//! Серверный каталог инструментов, JSON Schema, политики риска и approvals.
//!
//! Каталог — единственный источник имён инструментов для модели. Runtime
//! независимо проверяет те же tool_id и аргументы; неизвестный инструмент
//! отклоняется и здесь, и на границе исполнения.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::ai::models::{WorkspaceRow, redact_arguments};
use crate::ai::runtime::protocol::ToolDefinition;
use crate::ai::{errors, models};
use crate::core::error::{ApiError, ApiResult};

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub id: String,
    pub model_name: String,
    pub version: i32,
    pub name: String,
    pub description: String,
    pub category: String,
    pub risk: String,
    pub approval_policy: String,
    pub permission: String,
    pub idempotent: bool,
    pub timeout_seconds: i64,
    pub max_output_bytes: i64,
    pub input_schema: Value,
    /// true — инструмент исполняется backend (skill.load, mcp discovery), не Runtime.
    pub server_side: bool,
    /// true — инструмент доступен только файловому API пользователя, но не модели.
    pub ui_only: bool,
    /// true — инструмент является внешним MCP-tool и исполняется Runtime через MCP.
    pub mcp: bool,
}

/// Статическое описание встроенного инструмента.
#[derive(Clone, Debug)]
struct StaticToolSpec {
    id: &'static str,
    model_name: &'static str,
    version: i32,
    name: &'static str,
    description: &'static str,
    category: &'static str,
    risk: &'static str,
    approval_policy: &'static str,
    permission: &'static str,
    idempotent: bool,
    timeout_seconds: i64,
    max_output_bytes: i64,
    input_schema: Value,
    server_side: bool,
}

impl From<StaticToolSpec> for ToolSpec {
    fn from(value: StaticToolSpec) -> Self {
        Self {
            id: value.id.to_owned(),
            model_name: value.model_name.to_owned(),
            version: value.version,
            name: value.name.to_owned(),
            description: value.description.to_owned(),
            category: value.category.to_owned(),
            risk: value.risk.to_owned(),
            approval_policy: value.approval_policy.to_owned(),
            permission: value.permission.to_owned(),
            idempotent: value.idempotent,
            timeout_seconds: value.timeout_seconds,
            max_output_bytes: value.max_output_bytes,
            input_schema: value.input_schema,
            server_side: value.server_side,
            ui_only: is_ui_only(value.id),
            mcp: false,
        }
    }
}

/// Служебные инструменты файлового API: доступны только владельцу workspace,
/// никогда не передаются модели как tools.
pub fn is_ui_only(id: &str) -> bool {
    matches!(id, "fs.read_base64" | "fs.write_base64")
}

const RISK_LOW: &str = "low";
const RISK_MEDIUM: &str = "medium";
const RISK_HIGH: &str = "high";

fn schema_object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

fn path_property(description: &str) -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": 500, "description": description })
}

pub fn catalog() -> Vec<ToolSpec> {
    static_catalog().into_iter().map(Into::into).collect()
}

fn static_catalog() -> Vec<StaticToolSpec> {
    vec![
        StaticToolSpec {
            id: "fs.list",
            model_name: "fs_list",
            version: 1,
            name: "Список файлов",
            description: "Показывает файлы и каталоги внутри рабочего пространства.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.read",
            idempotent: true,
            timeout_seconds: 30,
            max_output_bytes: 131_072,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь; пусто — корень workspace."),
                    "depth": { "type": "integer", "minimum": 1, "maximum": 6, "default": 1 }
                }),
                &[],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.read",
            model_name: "fs_read",
            version: 1,
            name: "Чтение файла",
            description: "Читает текстовый файл из рабочего пространства.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.read",
            idempotent: true,
            timeout_seconds: 30,
            max_output_bytes: 262_144,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь к файлу."),
                    "maxBytes": { "type": "integer", "minimum": 1, "maximum": 8388608, "default": 262144 }
                }),
                &["path"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.read_base64",
            model_name: "fs_read_base64",
            version: 1,
            name: "Чтение бинарного файла",
            description: "Читает файл как Base64 для просмотра изображений и скачивания.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.read",
            idempotent: true,
            timeout_seconds: 30,
            max_output_bytes: 11_500_000,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь к файлу."),
                    "maxBytes": { "type": "integer", "minimum": 1, "maximum": 8388608, "default": 8388608 }
                }),
                &["path"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.write",
            model_name: "fs_write",
            version: 1,
            name: "Запись файла",
            description: "Создаёт или перезаписывает текстовый файл в рабочем пространстве.",
            category: "filesystem",
            risk: RISK_MEDIUM,
            approval_policy: "never",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 30,
            max_output_bytes: 65_536,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь к файлу."),
                    "content": { "type": "string", "maxLength": 1048576 },
                    "overwrite": { "type": "boolean", "default": false },
                    "expectedSha256": { "type": "string", "minLength": 64, "maxLength": 64 }
                }),
                &["path", "content"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.write_base64",
            model_name: "fs_write_base64",
            version: 1,
            name: "Запись бинарного файла",
            description: "Создаёт бинарный файл из Base64 (загрузка в workspace).",
            category: "filesystem",
            risk: RISK_MEDIUM,
            approval_policy: "never",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 60,
            max_output_bytes: 65_536,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь к файлу."),
                    "contentBase64": { "type": "string", "maxLength": 11534336 },
                    "overwrite": { "type": "boolean", "default": false },
                    "expectedSha256": { "type": "string", "minLength": 64, "maxLength": 64 }
                }),
                &["path", "contentBase64"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.search",
            model_name: "fs_search",
            version: 1,
            name: "Поиск по файлам",
            description: "Ищет строку в текстовых файлах рабочего пространства.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.read",
            idempotent: true,
            timeout_seconds: 60,
            max_output_bytes: 262_144,
            input_schema: schema_object(
                json!({
                    "query": { "type": "string", "minLength": 1, "maxLength": 500 },
                    "path": path_property("Каталог поиска; пусто — корень."),
                    "maxResults": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
                }),
                &["query"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.mkdir",
            model_name: "fs_mkdir",
            version: 1,
            name: "Создание каталога",
            description: "Создаёт каталог внутри рабочего пространства.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.write",
            idempotent: true,
            timeout_seconds: 15,
            max_output_bytes: 16_384,
            input_schema: schema_object(
                json!({ "path": path_property("Относительный путь каталога.") }),
                &["path"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.move",
            model_name: "fs_move",
            version: 1,
            name: "Перемещение файла",
            description: "Перемещает или переименовывает файл/каталог в рабочем пространстве.",
            category: "filesystem",
            risk: RISK_HIGH,
            approval_policy: "risk",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 30,
            max_output_bytes: 16_384,
            input_schema: schema_object(
                json!({
                    "from": path_property("Исходный относительный путь."),
                    "to": path_property("Целевой относительный путь."),
                    "overwrite": { "type": "boolean", "default": false }
                }),
                &["from", "to"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.copy",
            model_name: "fs_copy",
            version: 1,
            name: "Копирование файла",
            description: "Копирует файл внутри рабочего пространства.",
            category: "filesystem",
            risk: RISK_MEDIUM,
            approval_policy: "never",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 30,
            max_output_bytes: 16_384,
            input_schema: schema_object(
                json!({
                    "from": path_property("Исходный относительный путь."),
                    "to": path_property("Целевой относительный путь."),
                    "overwrite": { "type": "boolean", "default": false }
                }),
                &["from", "to"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.delete",
            model_name: "fs_delete",
            version: 1,
            name: "Удаление",
            description: "Удаляет файл или каталог. Опасное необратимое действие.",
            category: "filesystem",
            risk: RISK_HIGH,
            approval_policy: "risk",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 30,
            max_output_bytes: 16_384,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь."),
                    "recursive": { "type": "boolean", "default": false }
                }),
                &["path"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.apply_patch",
            model_name: "fs_apply_patch",
            version: 1,
            name: "Применение патча",
            description: "Применяет unified diff к файлам рабочего пространства.",
            category: "filesystem",
            risk: RISK_MEDIUM,
            approval_policy: "risk",
            permission: "ai.workspace.write",
            idempotent: false,
            timeout_seconds: 60,
            max_output_bytes: 262_144,
            input_schema: schema_object(
                json!({
                    "patch": { "type": "string", "minLength": 1, "maxLength": 1048576 },
                    "strip": { "type": "integer", "minimum": 0, "maximum": 3, "default": 1 }
                }),
                &["patch"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "fs.stat",
            model_name: "fs_stat",
            version: 1,
            name: "Метаданные файла",
            description: "Возвращает размер, тип и время изменения файла или каталога в workspace.",
            category: "filesystem",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.workspace.read",
            idempotent: true,
            timeout_seconds: 15,
            max_output_bytes: 16_384,
            input_schema: schema_object(
                json!({
                    "path": path_property("Относительный путь к файлу или каталогу."),
                }),
                &["path"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "program.run",
            model_name: "program_run",
            version: 1,
            name: "Запуск программы",
            description: "Запускает зарегистрированную администратором программу с аргументами.",
            category: "process",
            risk: RISK_HIGH,
            approval_policy: "always",
            permission: "ai.tools.use",
            idempotent: false,
            timeout_seconds: 300,
            max_output_bytes: 262_144,
            input_schema: schema_object(
                json!({
                    "programId": { "type": "string", "minLength": 1, "maxLength": 80 },
                    "args": {
                        "type": "array",
                        "maxItems": 32,
                        "items": { "type": "string", "maxLength": 500 }
                    },
                    "cwd": path_property("Рабочий каталог внутри workspace.")
                }),
                &["programId"],
            ),
            server_side: false,
        },
        StaticToolSpec {
            id: "skill.load",
            model_name: "skill_load",
            version: 1,
            name: "Загрузка Skill",
            description: "Загружает назначенный навык (SKILL.md или дополнительный файл) по требованию.",
            category: "skills",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.skills.use",
            idempotent: true,
            timeout_seconds: 15,
            max_output_bytes: 262_144,
            input_schema: schema_object(
                json!({
                    "skillId": { "type": "string", "minLength": 1, "maxLength": 80 },
                    "file": { "type": "string", "maxLength": 300, "description": "Путь внутри Skill; пусто — SKILL.md." }
                }),
                &["skillId"],
            ),
            server_side: true,
        },
        StaticToolSpec {
            id: "skills.list",
            model_name: "skills_list",
            version: 1,
            name: "Список Skills",
            description: "Возвращает метаданные Skills, назначенных пользователю для текущего workspace.",
            category: "skills",
            risk: RISK_LOW,
            approval_policy: "never",
            permission: "ai.skills.use",
            idempotent: true,
            timeout_seconds: 15,
            max_output_bytes: 65_536,
            input_schema: schema_object(json!({}), &[]),
            server_side: true,
        },
    ]
}

pub fn by_id(id: &str) -> Option<ToolSpec> {
    catalog().into_iter().find(|tool| tool.id == id)
}

pub fn by_model_name(name: &str) -> Option<ToolSpec> {
    catalog().into_iter().find(|tool| tool.model_name == name)
}

pub fn tool_definitions(specs: &[ToolSpec]) -> Vec<ToolDefinition> {
    specs
        .iter()
        .map(|spec| ToolDefinition {
            kind: "function".to_owned(),
            function: crate::ai::runtime::protocol::ToolDefinitionFunction {
                name: spec.model_name.to_owned(),
                description: spec.description.to_owned(),
                parameters: spec.input_schema.clone(),
            },
        })
        .collect()
}

#[derive(Clone, Debug, sqlx::FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyRow {
    pub tool_id: String,
    pub enabled: bool,
    pub risk: String,
    pub approval_policy: String,
    pub max_timeout_seconds: i32,
    pub max_output_bytes: i64,
}

pub async fn ensure_policies(db: &PgPool) -> Result<(), sqlx::Error> {
    for spec in catalog() {
        sqlx::query(
            "insert into ai_tool_policies
                (tool_id, enabled, risk, approval_policy, max_timeout_seconds, max_output_bytes)
             values ($1, true, $2, $3, $4, $5)
             on conflict (tool_id) do nothing",
        )
        .bind(spec.id)
        .bind(spec.risk)
        .bind(spec.approval_policy)
        .bind(spec.timeout_seconds as i32)
        .bind(spec.max_output_bytes)
        .execute(db)
        .await?;
    }
    Ok(())
}

pub async fn policies(db: &PgPool) -> ApiResult<Vec<ToolPolicyRow>> {
    Ok(sqlx::query_as::<_, ToolPolicyRow>(
        "select tool_id, enabled, risk, approval_policy, max_timeout_seconds, max_output_bytes
           from ai_tool_policies order by tool_id",
    )
    .fetch_all(db)
    .await?)
}

pub async fn policy(db: &PgPool, tool_id: &str) -> ApiResult<ToolPolicyRow> {
    sqlx::query_as::<_, ToolPolicyRow>(
        "select tool_id, enabled, risk, approval_policy, max_timeout_seconds, max_output_bytes
           from ai_tool_policies where tool_id = $1",
    )
    .bind(tool_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(errors::tool_not_allowed)
}

fn risk_rank(value: &str) -> u8 {
    match value {
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        "critical" => 4,
        _ => 4,
    }
}

pub fn approval_required(policy: &ToolPolicyRow) -> bool {
    match policy.approval_policy.as_str() {
        "always" => true,
        "risk" => risk_rank(&policy.risk) >= risk_rank("medium"),
        _ => false,
    }
}

pub fn validate_arguments(schema: &Value, arguments: &Value) -> ApiResult<()> {
    if !arguments.is_object() {
        return Err(errors::invalid_arguments(
            "Аргументы инструмента должны быть JSON-объектом.",
        ));
    }
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| ApiError::internal(format!("AI tool schema: {error}")))?;
    let mut errors = validator.iter_errors(arguments);
    if let Some(first) = errors.next() {
        return Err(errors::invalid_arguments(format!(
            "Аргументы не соответствуют схеме: {first}"
        )));
    }
    Ok(())
}

pub fn arguments_hash(arguments: &Value) -> String {
    let canonical = serde_json::to_string(arguments).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// Проверяет, что инструмент разрешён для workspace и пользователя.
pub fn authorize_workspace(
    spec: &ToolSpec,
    policy: &ToolPolicyRow,
    workspace: &WorkspaceRow,
) -> ApiResult<()> {
    if !policy.enabled {
        return Err(errors::tool_not_allowed());
    }
    if workspace.status != "active" {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "AI_WORKSPACE_SUSPENDED",
            "Рабочее пространство приостановлено администратором.",
        ));
    }
    let allowed: HashSet<&str> = workspace.allowed_tools.iter().map(String::as_str).collect();
    if !allowed.contains(spec.id.as_str()) {
        return Err(errors::tool_not_allowed());
    }
    Ok(())
}

/// Набор инструментов по умолчанию для нового workspace: безопасные операции,
/// явно требующие подтверждения действия администратор добавляет отдельно.
pub fn default_workspace_tools() -> Vec<String> {
    let mut tools: Vec<String> = catalog()
        .into_iter()
        .filter(|tool| !tool.ui_only && matches!(tool.risk.as_str(), RISK_LOW | RISK_MEDIUM))
        .map(|tool| tool.id.to_owned())
        .collect();
    if !tools.iter().any(|id| id == "skill.load") {
        tools.push("skill.load".to_owned());
    }
    tools
}

/// Инструменты из набора по умолчанию, которых ещё нет в allowlist workspace.
///
/// Используется административным «обновлением совместимости»: старые
/// workspace создавались до появления `fs.stat`/`skills.list` и не содержат их
/// в `allowed_tools`. Автоматического расширения прав нет — администратор
/// явно запускает операцию, и добавляются только безопасные инструменты.
pub fn missing_safe_tools(workspace: &WorkspaceRow) -> Vec<String> {
    missing_safe_tools_from(&workspace.allowed_tools)
}

pub(crate) fn missing_safe_tools_from(allowed_tools: &[String]) -> Vec<String> {
    let allowed: HashSet<&str> = allowed_tools.iter().map(String::as_str).collect();
    default_workspace_tools()
        .into_iter()
        .filter(|id| !allowed.contains(id.as_str()))
        .collect()
}

/// Инструменты, доступные агенту в задаче.
pub async fn allowed_for_task(
    db: &PgPool,
    workspace: &WorkspaceRow,
    include_skills: bool,
) -> ApiResult<Vec<(ToolSpec, ToolPolicyRow)>> {
    let allowed: HashSet<&str> = workspace.allowed_tools.iter().map(String::as_str).collect();
    let rows = policies(db).await?;
    let mut result = Vec::new();
    for spec in catalog() {
        if spec.ui_only {
            continue;
        }
        if spec.server_side {
            let gated = spec.id.starts_with("skill");
            if allowed.contains(spec.id.as_str()) && (!gated || include_skills) {
                if let Some(policy) = rows.iter().find(|row| row.tool_id == spec.id) {
                    result.push((spec, policy.clone()));
                }
            }
            continue;
        }
        if !allowed.contains(spec.id.as_str()) {
            continue;
        }
        if let Some(policy) = rows.iter().find(|row| row.tool_id == spec.id) {
            result.push((spec, policy.clone()));
        }
    }
    Ok(result)
}

/// Публичный каталог: без внутренних деталей и абсолютных путей.
pub async fn public_catalog(db: &PgPool) -> ApiResult<Value> {
    let rows = policies(db).await?;
    let items: Vec<Value> = catalog()
        .into_iter()
        .filter(|spec| !spec.server_side && !spec.ui_only)
        .map(|spec| {
            let policy = rows.iter().find(|row| row.tool_id == spec.id);
            json!({
                "id": spec.id,
                "name": spec.name,
                "description": spec.description,
                "category": spec.category,
                "risk": policy.map(|p| p.risk.clone()).unwrap_or_else(|| spec.risk.to_owned()),
                "requiresApproval": policy.map(approval_required).unwrap_or(false),
                "enabled": policy.map(|p| p.enabled).unwrap_or(true),
                "inputSchema": spec.input_schema,
                "timeoutSeconds": spec.timeout_seconds,
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}

pub fn arguments_for_ui(arguments: &Value) -> Value {
    redact_arguments(arguments)
}

pub async fn log_tool_event(
    db: &PgPool,
    task_id: uuid::Uuid,
    kind: &str,
    message: &str,
    data: Value,
) {
    let _ = sqlx::query(
        "insert into ai_task_events (task_id, kind, stage, message, data)
         values ($1, $2, 'tool', $3, $4)",
    )
    .bind(task_id)
    .bind(kind)
    .bind(message)
    .bind(data)
    .execute(db)
    .await;
    let _ = models::now();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_workspace_tools_include_new_safe_tools() {
        let tools = default_workspace_tools();
        assert!(tools.iter().any(|id| id == "fs.stat"));
        assert!(tools.iter().any(|id| id == "skills.list"));
        assert!(tools.iter().any(|id| id == "skill.load"));
        assert!(!tools.iter().any(|id| id == "fs.delete"));
        assert!(!tools.iter().any(|id| id == "program.run"));
    }

    #[test]
    fn missing_safe_tools_is_additive_and_preserves_explicit_choices() {
        // Старый workspace: без fs.stat и skills.list, но с явно выданным fs.delete.
        let allowed = vec![
            "fs.read".to_owned(),
            "fs.write".to_owned(),
            "fs.delete".to_owned(),
            "skill.load".to_owned(),
        ];
        let missing = missing_safe_tools_from(&allowed);
        assert!(missing.iter().any(|id| id == "fs.stat"));
        assert!(missing.iter().any(|id| id == "skills.list"));
        assert!(!missing.iter().any(|id| id == "fs.read"));
        assert!(!missing.iter().any(|id| id == "fs.delete"));
        // Опасные инструменты не попадают в «безопасное» обновление.
        assert!(!missing.iter().any(|id| id == "program.run"));
    }

    #[test]
    fn binary_tools_are_ui_only_and_never_agent_defaults() {
        let read = by_id("fs.read_base64").expect("fs.read_base64");
        let write = by_id("fs.write_base64").expect("fs.write_base64");
        assert!(read.ui_only);
        assert!(write.ui_only);
        let defaults = default_workspace_tools();
        assert!(!defaults.iter().any(|id| id == "fs.read_base64"));
        assert!(!defaults.iter().any(|id| id == "fs.write_base64"));
    }

    #[test]
    fn write_schema_accepts_expected_sha256() {
        let spec = by_id("fs.write").expect("fs.write");
        validate_arguments(
            &spec.input_schema,
            &json!({
                "path": "a.txt",
                "content": "x",
                "overwrite": true,
                "expectedSha256": "a".repeat(64),
            }),
        )
        .expect("schema accepts expectedSha256");
    }
}
