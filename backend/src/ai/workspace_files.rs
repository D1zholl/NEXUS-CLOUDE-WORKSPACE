//! Файловый API AI Agent Workspace.
//!
//! Все операции выполняет существующий NEXUS AI Runtime через
//! `tool.execute`; backend не читает диск сервера и не создаёт второй
//! исполнитель. Пользовательские операции ограничены RBAC
//! (`ai.workspace.read` / `ai.workspace.write`), владением workspace и
//! состоянием workspace. Allowlist инструментов агента применяется только к
//! модели: владелец workspace управляет своими файлами напрямую.

use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ai::models::WorkspaceRow;
use crate::ai::runtime::protocol::{FileSnapshot, ToolExecuteRequest, ToolOutcome, kind};
use crate::ai::runtime::{RuntimeReply, send_command};
use crate::ai::{PendingRequest, tools, workspaces};
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::core::idempotency;
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

/// Больше этого файл не отдаётся в браузер и не принимается на загрузку:
/// JSON-конверт WebSocket и body-limit API ограничены.
const RAW_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Тайм-аут пользовательской операции поверх тайм-аута инструмента.
const TOOL_GRACE_SECONDS: u64 = 15;

fn parse_path<T>(path: Result<Path<T>, PathRejection>) -> ApiResult<T> {
    path.map(|Path(value)| value)
        .map_err(|_| ApiError::validation("Некорректный идентификатор в пути."))
}

fn parse_query<T>(query: Result<Query<T>, QueryRejection>) -> ApiResult<T> {
    query
        .map(|Query(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректные параметры запроса: {error}")))
}

fn parse_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректное тело запроса: {error}")))
}

/// Проверка относительного пути на backend (Runtime проверяет повторно).
fn relative_path(value: &str, allow_empty: bool) -> ApiResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        if allow_empty {
            return Ok(String::new());
        }
        return Err(ApiError::validation("Укажите путь внутри workspace."));
    }
    if trimmed.len() > 500 {
        return Err(ApiError::validation("Путь слишком длинный."));
    }
    if trimmed.contains('\0') {
        return Err(ApiError::validation("Путь содержит недопустимый символ."));
    }
    let normalized = trimmed.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(ApiError::validation("Абсолютные пути запрещены."));
    }
    if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        return Err(ApiError::validation("Пути с буквой диска запрещены."));
    }
    if normalized.split('/').any(|part| part == "..") {
        return Err(ApiError::validation("'..' запрещён в путях workspace."));
    }
    Ok(normalized)
}

fn join_relative(dir: &str, name: &str) -> String {
    if dir.trim().is_empty() {
        name.to_owned()
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

fn is_admin(actor: &AuthUser) -> bool {
    actor.can("ai.admin.manage")
}

/// Лимит чтения/загрузки одного файла. `max_file_bytes = 0` в workspace
/// означает «без лимита»; в этом случае действует технический лимит API.
fn raw_limit(workspace: &WorkspaceRow) -> i64 {
    if workspace.max_file_bytes > 0 {
        workspace.max_file_bytes.min(RAW_MAX_BYTES as i64)
    } else {
        RAW_MAX_BYTES as i64
    }
}

/// Доступ к workspace для пользовательской файловой операции.
async fn workspace_for(
    state: &AppState,
    actor: &AuthUser,
    workspace_id: Uuid,
    write: bool,
) -> ApiResult<WorkspaceRow> {
    actor.require(if write {
        "ai.workspace.write"
    } else {
        "ai.workspace.read"
    })?;
    workspaces::accessible(&state.db, workspace_id, actor.id, is_admin(actor), true).await
}

/// Вызов инструмента Runtime от имени пользователя с учётом квот.
///
/// Резерв создаётся до отправки команды, поэтому две параллельные операции не
/// могут одновременно опереться на одно и то же измеренное значение usage.
pub async fn run_tool(
    state: &AppState,
    workspace: &WorkspaceRow,
    tool_id: &str,
    arguments: Value,
) -> ApiResult<ToolOutcome> {
    let operation_id = format!("user-tool:{}", Uuid::new_v4());
    let reservation =
        crate::ai::quota::reserve_for_tool(state, workspace, tool_id, &arguments, &operation_id)
            .await?;
    match execute_tool(state, workspace, tool_id, arguments).await {
        Ok(outcome) => {
            if let Some(reservation) = reservation {
                crate::ai::quota::confirm(state, reservation, outcome.usage.as_ref()).await?;
            }
            Ok(outcome)
        }
        Err(error) => {
            if let Some(reservation) = reservation {
                crate::ai::quota::finish_error(state, reservation, &error, tool_id).await;
            }
            Err(error)
        }
    }
}

/// Низкоуровневый вызов инструмента без резервирования.
///
/// Используется `run_tool` (после резерва) и служебными операциями: preflight
/// `fs.stat` для оценки квоты, чтение текущего содержимого при restore.
pub async fn execute_tool(
    state: &AppState,
    workspace: &WorkspaceRow,
    tool_id: &str,
    arguments: Value,
) -> ApiResult<ToolOutcome> {
    let spec = tools::by_id(tool_id).ok_or_else(crate::ai::errors::tool_not_allowed)?;
    tools::validate_arguments(&spec.input_schema, &arguments)?;
    let policy = tools::policy(&state.db, tool_id).await?;
    if !policy.enabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AI_TOOL_DISABLED",
            "Инструмент отключён администратором.",
        ));
    }
    if !state.ai.runtime.is_online(workspace.runtime_node_id) {
        return Err(crate::ai::errors::runtime_offline());
    }
    let timeout = policy
        .max_timeout_seconds
        .max(1)
        .min(spec.timeout_seconds as i32)
        .min(600) as u64;
    let request = ToolExecuteRequest {
        tool_id: spec.id.clone(),
        version: spec.version,
        arguments,
        workspace_id: workspace.id,
        workspace_root: workspace.root_path.clone(),
        timeout_seconds: timeout as i64,
        max_output_bytes: policy.max_output_bytes.max(1024),
        workspace_quota_bytes: workspace.quota_bytes,
        workspace_max_files: workspace.max_files as i64,
        workspace_max_file_bytes: workspace.max_file_bytes,
        workspace_usage_bytes: workspace.usage_bytes,
        workspace_usage_files: workspace.usage_files,
        network_policy: "deny".to_owned(),
        max_memory_bytes: 0,
    };
    let wire = serde_json::to_value(&request)
        .map_err(|error| ApiError::internal(format!("tool request json: {error}")))?;
    let message_id = Uuid::new_v4();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.ai.runtime.pending_register(
        message_id,
        workspace.runtime_node_id,
        PendingRequest::Reply(tx),
    );
    if let Err(error) = send_command(
        state,
        workspace.runtime_node_id,
        message_id,
        kind::TOOL_EXECUTE,
        Some(message_id),
        None,
        wire,
        None,
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error);
    }
    let wait = timeout + TOOL_GRACE_SECONDS;
    let reply = match tokio::time::timeout(Duration::from_secs(wait), rx).await {
        Ok(reply) => reply,
        Err(_) => {
            let _ = state.ai.runtime.pending_remove(message_id);
            return Err(ApiError::unavailable(
                "AI_TOOL_TIMEOUT",
                "Runtime не завершил операцию за отведённое время.",
                5,
            ));
        }
    };
    match reply {
        Ok(RuntimeReply::Tool(Ok(outcome))) => Ok(outcome),
        Ok(RuntimeReply::Tool(Err(failure))) | Ok(RuntimeReply::Failed(failure)) => {
            Err(map_runtime_failure(failure.code, failure.message))
        }
        _ => Err(ApiError::unavailable(
            "AI_EXECUTION_UNKNOWN",
            "Результат операции неизвестен: Runtime не вернул подтверждение. \
             Проверьте файл вручную перед повторной попыткой.",
            5,
        )),
    }
}

fn map_runtime_failure(code: String, message: String) -> ApiError {
    let lower = message.to_lowercase();
    if message.contains("AI_FILE_CONFLICT") {
        return ApiError::conflict("AI_FILE_CONFLICT", "Файл изменён после открытия.")
            .with_details(json!({ "runtimeMessage": message }));
    }
    if lower.contains("уже существует") {
        return ApiError::conflict("AI_FILE_EXISTS", message);
    }
    if lower.contains("не найден") || lower.contains("не существует") {
        return ApiError::new(StatusCode::NOT_FOUND, "AI_FILE_NOT_FOUND", message);
    }
    if code == "AI_RUNTIME_OFFLINE" {
        return crate::ai::errors::runtime_offline();
    }
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

/// Квоты пользователя: сумма usage по его workspace + дельта операции.
async fn enforce_user_quota(
    state: &AppState,
    workspace: &WorkspaceRow,
    delta_bytes: i64,
    delta_files: i64,
    file_bytes: Option<i64>,
) -> ApiResult<()> {
    let quota = workspaces::ensure_quota(&state.db, workspace.owner_id).await?;
    if let (Some(size), true) = (file_bytes, quota.max_file_bytes > 0) {
        if size > quota.max_file_bytes {
            return Err(ApiError::new(
                StatusCode::INSUFFICIENT_STORAGE,
                "AI_USER_FILE_QUOTA_EXCEEDED",
                format!(
                    "Файл превышает пользовательский лимит {} байт.",
                    quota.max_file_bytes
                ),
            ));
        }
    }
    let (used_bytes, used_files): (i64, i64) = sqlx::query_as(
        "select coalesce(sum(usage_bytes), 0)::bigint,
                coalesce(sum(usage_files), 0)::bigint
           from ai_workspaces where owner_id = $1 and deleted_at is null",
    )
    .bind(workspace.owner_id)
    .fetch_one(&state.db)
    .await?;
    if quota.max_storage_bytes > 0 && used_bytes + delta_bytes > quota.max_storage_bytes {
        return Err(ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "AI_STORAGE_QUOTA_EXCEEDED",
            format!(
                "Операция превышает пользовательский лимит хранилища ({} байт).",
                quota.max_storage_bytes
            ),
        ));
    }
    if quota.max_files > 0 && used_files + delta_files > quota.max_files as i64 {
        return Err(ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "AI_FILES_QUOTA_EXCEEDED",
            format!("Достигнут лимит файлов пользователя ({}).", quota.max_files),
        ));
    }
    Ok(())
}

async fn audit_file(
    state: &AppState,
    actor: &AuthUser,
    event_type: &str,
    workspace_id: Uuid,
    details: Value,
) {
    record(
        &state.db,
        AuditEvent {
            event_type,
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &workspace_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details,
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// Дерево, чтение, запись
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeQuery {
    path: Option<String>,
    depth: Option<i32>,
}

pub async fn tree(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<TreeQuery>, QueryRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let relative = relative_path(query.path.as_deref().unwrap_or(""), true)?;
    let depth = query.depth.unwrap_or(1).clamp(1, 6);
    let mut arguments = json!({ "depth": depth });
    if !relative.is_empty() {
        arguments["path"] = json!(relative);
    }
    let outcome = run_tool(&state, &workspace, "fs.list", arguments).await?;
    Ok(crate::api::ApiOk(outcome.result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentQuery {
    path: String,
}

pub async fn content(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ContentQuery>, QueryRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let relative = relative_path(&query.path, false)?;
    let outcome = run_tool(
        &state,
        &workspace,
        "fs.read",
        json!({
            "path": relative,
            "maxBytes": raw_limit(&workspace),
        }),
    )
    .await?;
    let mut value = outcome.result;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "maxEditableBytes".to_owned(),
            json!(workspace.max_file_bytes),
        );
        object.insert(
            "runtimeTruncated".to_owned(),
            object.get("truncated").cloned().unwrap_or(json!(false)),
        );
    }
    Ok(crate::api::ApiOk(value))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveContentRequest {
    pub path: String,
    pub content: String,
    /// Хеш версии, которую открыл пользователь. Обязателен для существующего
    /// файла: без него запись не выполняется (никакой тихой перезаписи).
    pub base_sha256: Option<String>,
}

pub async fn save_content(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: axum::http::HeaderMap,
    body: Result<Json<SaveContentRequest>, JsonRejection>,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&json!({
        "workspaceId": workspace_id,
        "request": &request,
    }));
    let scope = format!("ai.file.save:{workspace_id}:{}", request.path);
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move { save_content_inner(&st, &actor, workspace_id, request).await },
    )
    .await
}

async fn save_content_inner(
    state: &AppState,
    actor: &AuthUser,
    workspace_id: Uuid,
    request: SaveContentRequest,
) -> ApiResult<Value> {
    let workspace = workspace_for(state, actor, workspace_id, true).await?;
    let relative = relative_path(&request.path, false)?;
    let size = request.content.len() as i64;
    if workspace.max_file_bytes > 0 && size > workspace.max_file_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "AI_FILE_TOO_LARGE",
            format!(
                "Файл превышает лимит workspace ({} байт).",
                workspace.max_file_bytes
            ),
        ));
    }
    enforce_user_quota(state, &workspace, 0, 0, Some(size)).await?;

    let stat = run_tool(
        state,
        &workspace,
        "fs.stat",
        json!({ "path": relative.clone() }),
    )
    .await;
    let existing = stat.ok().map(|outcome| outcome.result);
    let old_size = existing
        .as_ref()
        .and_then(|value| value.get("size"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let exists = existing.is_some();
    if exists
        && request
            .base_sha256
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "AI_FILE_PRECONDITION_REQUIRED",
            "Файл уже существует. Передайте baseSha256 текущей версии.",
        ));
    }
    enforce_user_quota(
        state,
        &workspace,
        size - old_size,
        if exists { 0 } else { 1 },
        None,
    )
    .await?;

    let mut arguments = json!({
        "path": relative.clone(),
        "content": request.content,
        "overwrite": exists,
    });
    if let Some(sha) = request
        .base_sha256
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        arguments["expectedSha256"] = json!(sha.trim());
    }
    match run_tool(state, &workspace, "fs.write", arguments).await {
        Ok(outcome) => {
            audit_file(
                state,
                actor,
                "ai.workspace.file_saved",
                workspace_id,
                json!({ "path": relative, "bytes": size, "created": !exists }),
            )
            .await;
            Ok(json!({
                "path": relative,
                "sha256": outcome.result.get("sha256").cloned().unwrap_or(Value::Null),
                "bytes": size,
                "savedAt": chrono::Utc::now(),
            }))
        }
        Err(error) if error.code == "AI_FILE_CONFLICT" => {
            // Отдаём актуальную версию для сравнения, не изменяя файл.
            let current = run_tool(
                state,
                &workspace,
                "fs.read",
                json!({
                    "path": relative.clone(),
                    "maxBytes": raw_limit(&workspace),
                }),
            )
            .await
            .ok()
            .map(|outcome| outcome.result);
            let current_sha = current
                .as_ref()
                .and_then(|value| value.get("sha256"))
                .cloned()
                .unwrap_or(Value::Null);
            Err(ApiError::conflict(
                "AI_FILE_CONFLICT",
                "Файл изменён после открытия. Сравните версии перед сохранением.",
            )
            .with_details(json!({
                "path": relative,
                "baseSha256": request.base_sha256,
                "currentSha256": current_sha,
                "currentContent": current
                    .as_ref()
                    .and_then(|value| value.get("content"))
                    .cloned()
                    .unwrap_or(Value::Null),
            })))
        }
        Err(error) => Err(error),
    }
}

// ---------------------------------------------------------------------------
// Операции с файлами
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRequest {
    path: String,
}

pub async fn create_directory(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: axum::http::HeaderMap,
    body: Result<Json<DirectoryRequest>, JsonRejection>,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&json!({ "dir": &request }));
    let scope = format!("ai.file.mkdir:{workspace_id}:{}", request.path);
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let relative = relative_path(&request.path, false)?;
            let outcome =
                run_tool(&st, &workspace, "fs.mkdir", json!({ "path": relative })).await?;
            audit_file(
                &st,
                &actor,
                "ai.workspace.directory_created",
                workspace_id,
                json!({ "path": relative }),
            )
            .await;
            Ok(outcome.result)
        },
    )
    .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    from: String,
    to: String,
    #[serde(default)]
    overwrite: bool,
}

pub async fn move_entry(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: axum::http::HeaderMap,
    body: Result<Json<TransferRequest>, JsonRejection>,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&json!({ "move": &request }));
    let scope = format!(
        "ai.file.move:{workspace_id}:{}:{}",
        request.from, request.to
    );
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let from = relative_path(&request.from, false)?;
            let to = relative_path(&request.to, false)?;
            let outcome = run_tool(
                &st,
                &workspace,
                "fs.move",
                json!({ "from": from, "to": to, "overwrite": request.overwrite }),
            )
            .await?;
            audit_file(
                &st,
                &actor,
                "ai.workspace.file_moved",
                workspace_id,
                json!({ "from": from, "to": to }),
            )
            .await;
            Ok(outcome.result)
        },
    )
    .await
}

pub async fn copy_entry(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: axum::http::HeaderMap,
    body: Result<Json<TransferRequest>, JsonRejection>,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&json!({ "copy": &request }));
    let scope = format!(
        "ai.file.copy:{workspace_id}:{}:{}",
        request.from, request.to
    );
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let from = relative_path(&request.from, false)?;
            let to = relative_path(&request.to, false)?;
            let outcome = run_tool(
                &st,
                &workspace,
                "fs.copy",
                json!({ "from": from, "to": to, "overwrite": request.overwrite }),
            )
            .await?;
            audit_file(
                &st,
                &actor,
                "ai.workspace.file_copied",
                workspace_id,
                json!({ "from": from, "to": to }),
            )
            .await;
            Ok(outcome.result)
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuery {
    path: String,
    #[serde(default)]
    recursive: bool,
}

pub async fn delete_entry(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let request_hash = idempotency::hash_request(&json!({
        "delete": &query.path,
        "recursive": query.recursive,
    }));
    let scope = format!("ai.file.delete:{workspace_id}:{}", query.path);
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let relative = relative_path(&query.path, false)?;
            let outcome = run_tool(
                &st,
                &workspace,
                "fs.delete",
                json!({ "path": relative, "recursive": query.recursive }),
            )
            .await?;
            audit_file(
                &st,
                &actor,
                "ai.workspace.file_deleted",
                workspace_id,
                json!({ "path": relative, "recursive": query.recursive }),
            )
            .await;
            Ok(outcome.result)
        },
    )
    .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    query: String,
    path: Option<String>,
    max_results: Option<i64>,
}

pub async fn search_files(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<SearchRequest>, JsonRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let workspace_id = parse_path(path)?;
    let request = parse_body(body)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    if request.query.trim().is_empty() {
        return Err(ApiError::validation(
            "Поисковый запрос не может быть пустым.",
        ));
    }
    if request.query.chars().count() > 500 {
        return Err(ApiError::validation("Поисковый запрос слишком длинный."));
    }
    let relative = relative_path(request.path.as_deref().unwrap_or(""), true)?;
    let mut arguments = json!({
        "query": request.query,
        "maxResults": request.max_results.unwrap_or(50).clamp(1, 200),
    });
    if !relative.is_empty() {
        arguments["path"] = json!(relative);
    }
    let outcome = run_tool(&state, &workspace, "fs.search", arguments).await?;
    Ok(crate::api::ApiOk(outcome.result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatQuery {
    path: String,
}

pub async fn stat_entry(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<StatQuery>, QueryRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let relative = relative_path(&query.path, false)?;
    let outcome = run_tool(&state, &workspace, "fs.stat", json!({ "path": relative })).await?;
    Ok(crate::api::ApiOk(outcome.result))
}

// ---------------------------------------------------------------------------
// Upload / download
// ---------------------------------------------------------------------------

fn sanitize_upload_name(raw: &str) -> ApiResult<String> {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if name.is_empty() || name == "." || name == ".." {
        return Err(ApiError::validation("Некорректное имя загружаемого файла."));
    }
    if name.chars().count() > 200 {
        return Err(ApiError::validation("Слишком длинное имя файла."));
    }
    if name
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(ApiError::validation(
            "Имя файла содержит недопустимые символы.",
        ));
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err(ApiError::validation(
            "Имя файла не может заканчиваться пробелом или точкой.",
        ));
    }
    Ok(name)
}

pub async fn upload(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    actor.require("ai.workspace.write")?;
    let workspace = workspace_for(&state, &actor, workspace_id, true).await?;
    let limit = raw_limit(&workspace) as usize;
    let mut directory = String::new();
    let mut file_name = String::new();
    let mut bytes: Option<Bytes> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::validation(format!("Некорректный multipart: {error}")))?
    {
        match field.name().unwrap_or("") {
            "path" => {
                directory = field.text().await.map_err(|error| {
                    ApiError::validation(format!("Ошибка чтения пути: {error}"))
                })?;
            }
            "file" => {
                file_name = sanitize_upload_name(field.file_name().unwrap_or("upload.bin"))?;
                let data = field.bytes().await.map_err(|error| {
                    ApiError::validation(format!("Ошибка чтения файла: {error}"))
                })?;
                if data.len() > limit {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "AI_FILE_TOO_LARGE",
                        format!("Файл превышает лимит {limit} байт."),
                    ));
                }
                bytes = Some(data);
            }
            _ => {}
        }
    }
    let Some(bytes) = bytes else {
        return Err(ApiError::validation(
            "Файл не найден в form-data (поле file).",
        ));
    };
    if file_name.is_empty() {
        return Err(ApiError::validation("Не указано имя файла."));
    }
    let directory = relative_path(&directory, true)?;
    let target = join_relative(&directory, &file_name);
    let request_hash = idempotency::hash_request(&json!({
        "target": target,
        "size": bytes.len(),
    }));
    let scope = format!("ai.file.upload:{workspace_id}:{target}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            enforce_user_quota(
                &st,
                &workspace,
                bytes.len() as i64,
                1,
                Some(bytes.len() as i64),
            )
            .await?;
            let outcome = run_tool(
                &st,
                &workspace,
                "fs.write_base64",
                json!({
                    "path": target,
                    "contentBase64": BASE64.encode(&bytes),
                    "overwrite": false,
                }),
            )
            .await?;
            audit_file(
                &st,
                &actor,
                "ai.workspace.file_uploaded",
                workspace_id,
                json!({ "path": target, "bytes": bytes.len() }),
            )
            .await;
            Ok(json!({
                "path": target,
                "bytes": bytes.len(),
                "sha256": outcome.result.get("sha256").cloned().unwrap_or(Value::Null),
            }))
        },
    )
    .await
}

fn mime_for(path: &str, bytes: &[u8]) -> String {
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type().to_owned();
    }
    let extension = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match extension.as_str() {
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" | "jsx" => "text/plain",
        "txt" | "log" | "toml" | "yaml" | "yml" | "sql" | "sh" | "ps1" | "rs" | "py" => {
            "text/plain"
        }
        _ => "application/octet-stream",
    }
    .to_owned()
}

fn inline_allowed(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif"
    )
}

pub async fn raw_file(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ContentQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let relative = relative_path(&query.path, false)?;
    let limit = raw_limit(&workspace);
    let outcome = run_tool(
        &state,
        &workspace,
        "fs.read_base64",
        json!({ "path": relative.clone(), "maxBytes": limit }),
    )
    .await?;
    let truncated = outcome
        .result
        .get("truncated")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if truncated {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "AI_FILE_TOO_LARGE",
            "Файл слишком большой для просмотра.",
        ));
    }
    let encoded = outcome
        .result
        .get("contentBase64")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ApiError::internal("Runtime не вернул содержимое файла."))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| ApiError::internal("Некорректные Base64-данные Runtime."))?;
    let mime = mime_for(&relative, &bytes);
    let inline = inline_allowed(&mime);
    let name = relative
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let disposition = format!(
        "{}; filename=\"{}\"",
        if inline { "inline" } else { "attachment" },
        if name.is_empty() { "file" } else { &name }
    );
    let headers = [
        (header::CONTENT_TYPE, mime),
        (header::CONTENT_DISPOSITION, disposition),
        (header::CACHE_CONTROL, "no-store".to_owned()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
    ];
    Ok((headers, bytes).into_response())
}

// ---------------------------------------------------------------------------
// Снимки изменений агента, diff и откат
// ---------------------------------------------------------------------------

fn decode_snapshot(value: Option<&str>) -> ApiResult<Option<(String, String)>> {
    let Some(encoded) = value else {
        return Ok(None);
    };
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| ApiError::internal("Некорректный снимок файла."))?;
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(Some((text, "utf8".to_owned()))),
        Ok(_) => Ok(Some((encoded.to_owned(), "base64".to_owned()))),
        Err(_) => Ok(Some((encoded.to_owned(), "base64".to_owned()))),
    }
}

/// Сохраняет снимки, полученные от Runtime, для diff и отката.
pub async fn capture_snapshots(
    db: &sqlx::PgPool,
    workspace_id: Uuid,
    owner_id: Uuid,
    task_id: Option<Uuid>,
    execution_id: Option<Uuid>,
    snapshots: &[FileSnapshot],
) {
    for snapshot in snapshots {
        let before = decode_snapshot(snapshot.before_base64.as_deref());
        let after = decode_snapshot(snapshot.after_base64.as_deref());
        let (Ok(before), Ok(after)) = (before, after) else {
            tracing::warn!(path = %snapshot.path, "снимок файла пропущен: некорректные данные");
            continue;
        };
        let (additions, deletions) = line_stats(before.as_ref(), after.as_ref());
        let result = sqlx::query(
            "insert into ai_file_snapshots
                (workspace_id, owner_id, task_id, execution_id, path, action,
                 before_sha256, after_sha256, before_content, after_content,
                 before_encoding, after_encoding, truncated, additions, deletions)
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
        )
        .bind(workspace_id)
        .bind(owner_id)
        .bind(task_id)
        .bind(execution_id)
        .bind(&snapshot.path)
        .bind(&snapshot.action)
        .bind(&snapshot.before_sha256)
        .bind(&snapshot.after_sha256)
        .bind(before.as_ref().map(|(text, _)| text.as_str()))
        .bind(after.as_ref().map(|(text, _)| text.as_str()))
        .bind(
            before
                .as_ref()
                .map(|(_, encoding)| encoding.as_str())
                .unwrap_or("utf8"),
        )
        .bind(
            after
                .as_ref()
                .map(|(_, encoding)| encoding.as_str())
                .unwrap_or("utf8"),
        )
        .bind(snapshot.truncated)
        .bind(additions)
        .bind(deletions)
        .execute(db)
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, path = %snapshot.path, "не удалось сохранить снимок файла");
        }
    }
}

/// Приблизительный подсчёт добавленных/удалённых строк для UI.
fn line_stats(before: Option<&(String, String)>, after: Option<&(String, String)>) -> (i32, i32) {
    let before_lines: Vec<&str> = before
        .filter(|(_, encoding)| encoding == "utf8")
        .map(|(text, _)| text.lines().collect())
        .unwrap_or_default();
    let after_lines: Vec<&str> = after
        .filter(|(_, encoding)| encoding == "utf8")
        .map(|(text, _)| text.lines().collect())
        .unwrap_or_default();
    if before_lines.is_empty() && after_lines.is_empty() {
        return (0, 0);
    }
    if before_lines.len() > 1200 || after_lines.len() > 1200 {
        return (after_lines.len() as i32, before_lines.len() as i32);
    }
    let columns = after_lines.len() + 1;
    let mut previous = vec![0i32; columns];
    let mut current = vec![0i32; columns];
    for left in &before_lines {
        for (index, right) in after_lines.iter().enumerate() {
            current[index + 1] = if left == right {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }
    let common = previous[after_lines.len()] as i32;
    (
        after_lines.len() as i32 - common,
        before_lines.len() as i32 - common,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesQuery {
    task_id: Option<Uuid>,
    path: Option<String>,
    limit: Option<i64>,
}

pub async fn list_changes(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ChangesQuery>, QueryRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let workspace_id = parse_path(path)?;
    let query = parse_query(query)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let filter_path = match query.path.as_deref() {
        Some(value) if !value.trim().is_empty() => Some(relative_path(value, false)?),
        _ => None,
    };
    let rows = sqlx::query_as::<_, SnapshotRow>(
        "select id, task_id, execution_id, path, action, before_sha256, after_sha256,
                truncated, additions, deletions, restored_at, created_at,
                (before_content is not null) as has_before,
                (after_content is not null) as has_after
           from ai_file_snapshots
          where workspace_id = $1
            and ($2::uuid is null or task_id = $2)
            and ($3::text is null or path = $3)
          order by created_at desc
          limit $4",
    )
    .bind(workspace.id)
    .bind(query.task_id)
    .bind(&filter_path)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows.iter().map(SnapshotRow::view).collect();
    Ok(crate::api::ApiOk(json!({ "items": items })))
}

#[derive(Debug, sqlx::FromRow)]
struct SnapshotRow {
    id: Uuid,
    task_id: Option<Uuid>,
    execution_id: Option<Uuid>,
    path: String,
    action: String,
    before_sha256: Option<String>,
    after_sha256: Option<String>,
    truncated: bool,
    additions: i32,
    deletions: i32,
    restored_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    has_before: bool,
    has_after: bool,
}

impl SnapshotRow {
    fn view(&self) -> Value {
        json!({
            "id": self.id,
            "taskId": self.task_id,
            "executionId": self.execution_id,
            "path": self.path,
            "action": self.action,
            "beforeSha256": self.before_sha256,
            "afterSha256": self.after_sha256,
            "truncated": self.truncated,
            "additions": self.additions,
            "deletions": self.deletions,
            "restoredAt": self.restored_at,
            "createdAt": self.created_at,
            "hasBefore": self.has_before,
            "hasAfter": self.has_after,
        })
    }
}

pub async fn snapshot_detail(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    let (workspace_id, snapshot_id) = parse_path(path)?;
    let workspace = workspace_for(&state, &actor, workspace_id, false).await?;
    let row = sqlx::query_as::<_, SnapshotContentRow>(
        "select id, task_id, execution_id, path, action, before_sha256, after_sha256,
                before_content, after_content, before_encoding, after_encoding,
                truncated, additions, deletions, restored_at, created_at
           from ai_file_snapshots where id = $1 and workspace_id = $2",
    )
    .bind(snapshot_id)
    .bind(workspace.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Снимок файла не найден."))?;
    Ok(crate::api::ApiOk(json!({
        "id": row.id,
        "taskId": row.task_id,
        "executionId": row.execution_id,
        "path": row.path,
        "action": row.action,
        "beforeSha256": row.before_sha256,
        "afterSha256": row.after_sha256,
        "before": content_view(row.before_content, &row.before_encoding),
        "after": content_view(row.after_content, &row.after_encoding),
        "truncated": row.truncated,
        "additions": row.additions,
        "deletions": row.deletions,
        "restoredAt": row.restored_at,
        "createdAt": row.created_at,
    })))
}

fn content_view(content: Option<String>, encoding: &str) -> Value {
    match content {
        Some(text) if encoding == "utf8" => json!({ "content": text, "encoding": "utf8" }),
        Some(text) => json!({ "contentBase64": text, "encoding": "base64" }),
        None => Value::Null,
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SnapshotContentRow {
    id: Uuid,
    task_id: Option<Uuid>,
    execution_id: Option<Uuid>,
    path: String,
    action: String,
    before_sha256: Option<String>,
    after_sha256: Option<String>,
    before_content: Option<String>,
    after_content: Option<String>,
    before_encoding: String,
    after_encoding: String,
    truncated: bool,
    additions: i32,
    deletions: i32,
    restored_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Текущее состояние файла: содержимое Base64 и sha256.
async fn current_file(
    state: &AppState,
    workspace: &WorkspaceRow,
    path: &str,
) -> ApiResult<Option<(Vec<u8>, String)>> {
    let limit = raw_limit(workspace);
    match run_tool(
        state,
        workspace,
        "fs.read_base64",
        json!({ "path": path, "maxBytes": limit }),
    )
    .await
    {
        Ok(outcome) => {
            let encoded = outcome
                .result
                .get("contentBase64")
                .and_then(|value| value.as_str())
                .ok_or_else(|| ApiError::internal("Runtime не вернул содержимое файла."))?;
            let sha = outcome
                .result
                .get("sha256")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_owned();
            let bytes = BASE64
                .decode(encoded)
                .map_err(|_| ApiError::internal("Некорректные данные Runtime."))?;
            Ok(Some((bytes, sha)))
        }
        Err(error) if error.code == "AI_FILE_NOT_FOUND" => Ok(None),
        Err(error) => Err(error),
    }
}

struct RestorePlan {
    workspace: WorkspaceRow,
    task_id: Option<Uuid>,
    files: Vec<RestoreFile>,
}

struct RestoreFile {
    snapshot_ids: Vec<Uuid>,
    path: String,
    expected_after: Option<String>,
    before_content: Option<(String, String)>,
    truncated: bool,
}

enum RestoreStep {
    Restore(Vec<u8>),
    Delete,
}

fn decode_stored_content(content: Option<&str>, encoding: &str) -> ApiResult<Option<Vec<u8>>> {
    match content {
        None => Ok(None),
        Some(text) if encoding == "utf8" => Ok(Some(text.as_bytes().to_vec())),
        Some(text) => BASE64
            .decode(text)
            .map(Some)
            .map_err(|_| ApiError::internal("Некорректный снимок файла.")),
    }
}

/// Проверяет конфликты всех файлов до любых изменений, затем восстанавливает.
async fn apply_restore(state: &AppState, actor: &AuthUser, plan: &RestorePlan) -> ApiResult<Value> {
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    for file in &plan.files {
        if file.truncated {
            conflicts.push(json!({
                "path": file.path,
                "reason": "snapshot_truncated",
                "message": "Снимок файла усечён: безопасный откат невозможен.",
            }));
            continue;
        }
        let current = current_file(state, &plan.workspace, &file.path).await?;
        let current_sha = current.as_ref().map(|(_, sha)| sha.clone());
        let step = match &file.before_content {
            Some((text, encoding)) => RestoreStep::Restore(
                decode_stored_content(Some(text), encoding)?
                    .ok_or_else(|| ApiError::internal("Пустой снимок before."))?,
            ),
            None => RestoreStep::Delete,
        };
        // Повторный откат идемпотентен: если файл уже в целевом состоянии,
        // конфликт не создаётся и файл не трогается.
        let already = match (&step, &current) {
            (RestoreStep::Restore(bytes), Some((content, _))) => {
                content.as_slice() == bytes.as_slice()
            }
            (RestoreStep::Delete, None) => true,
            _ => false,
        };
        if already {
            continue;
        }
        if current_sha != file.expected_after {
            conflicts.push(json!({
                "path": file.path,
                "reason": "modified_after_task",
                "currentSha256": current_sha,
                "expectedSha256": file.expected_after,
            }));
            continue;
        }
        steps.push((file, step, current));
    }
    if !conflicts.is_empty() {
        return Err(ApiError::conflict(
            "AI_RESTORE_CONFLICT",
            "Файлы изменены после задачи. Откат не выполнен, чтобы не потерять новые правки.",
        )
        .with_details(json!({ "conflicts": conflicts })));
    }
    let mut restored = Vec::new();
    for (file, step, current) in steps {
        match step {
            RestoreStep::Restore(bytes) => {
                if current.as_ref().map(|(content, _)| content.as_slice()) == Some(bytes.as_slice())
                {
                    // Уже в целевом состоянии: не трогаем файл повторно.
                    continue;
                }
                let mut arguments = json!({
                    "path": file.path,
                    "contentBase64": BASE64.encode(&bytes),
                    "overwrite": true,
                });
                if let Some(expected) = file.expected_after.as_deref() {
                    arguments["expectedSha256"] = json!(expected);
                }
                run_tool(state, &plan.workspace, "fs.write_base64", arguments).await?;
                restored.push(json!({ "path": file.path, "action": "restore" }));
            }
            RestoreStep::Delete => {
                if current.is_none() {
                    continue;
                }
                run_tool(
                    state,
                    &plan.workspace,
                    "fs.delete",
                    json!({ "path": file.path }),
                )
                .await?;
                restored.push(json!({ "path": file.path, "action": "delete" }));
            }
        }
        sqlx::query("update ai_file_snapshots set restored_at = now() where id = any($1::uuid[])")
            .bind(&file.snapshot_ids)
            .execute(&state.db)
            .await?;
    }
    audit_file(
        state,
        actor,
        "ai.workspace.snapshot_restored",
        plan.workspace.id,
        json!({
            "taskId": plan.task_id,
            "files": restored.len(),
        }),
    )
    .await;
    Ok(json!({ "restored": restored, "taskId": plan.task_id }))
}

pub async fn restore_snapshot(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let (workspace_id, snapshot_id) = parse_path(path)?;
    let request_hash = idempotency::hash_request(&json!({ "restoreSnapshot": snapshot_id }));
    let scope = format!("ai.snapshot.restore:{snapshot_id}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let row = sqlx::query_as::<_, SnapshotContentRow>(
                "select id, task_id, execution_id, path, action, before_sha256, after_sha256,
                        before_content, after_content, before_encoding, after_encoding,
                        truncated, additions, deletions, restored_at, created_at
                   from ai_file_snapshots where id = $1 and workspace_id = $2",
            )
            .bind(snapshot_id)
            .bind(workspace.id)
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Снимок файла не найден."))?;
            let plan = RestorePlan {
                workspace,
                task_id: row.task_id,
                files: vec![RestoreFile {
                    snapshot_ids: vec![row.id],
                    path: row.path,
                    expected_after: row.after_sha256,
                    before_content: match row.before_content {
                        Some(content) => Some((content, row.before_encoding)),
                        None => None,
                    },
                    truncated: row.truncated,
                }],
            };
            apply_restore(&st, &actor, &plan).await
        },
    )
    .await
}

/// Откат всех файлов задачи к состоянию до её первого изменения.
pub async fn restore_task(
    State(state): State<std::sync::Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let (workspace_id, task_id) = parse_path(path)?;
    let request_hash = idempotency::hash_request(&json!({ "restoreTask": task_id }));
    let scope = format!("ai.task.restore:{task_id}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let workspace = workspace_for(&st, &actor, workspace_id, true).await?;
            let rows = sqlx::query_as::<_, SnapshotContentRow>(
                "select id, task_id, execution_id, path, action, before_sha256, after_sha256,
                        before_content, after_content, before_encoding, after_encoding,
                        truncated, additions, deletions, restored_at, created_at
                   from ai_file_snapshots
                  where workspace_id = $1 and task_id = $2
                  order by created_at asc",
            )
            .bind(workspace.id)
            .bind(task_id)
            .fetch_all(&st.db)
            .await?;
            if rows.is_empty() {
                return Err(ApiError::not_found(
                    "Для этой задачи нет сохранённых изменений файлов.",
                ));
            }
            // Для каждого файла: before — из первого снимка, ожидаемое
            // состояние — из последнего.
            let mut order: Vec<String> = Vec::new();
            let mut files: std::collections::HashMap<String, RestoreFile> =
                std::collections::HashMap::new();
            for row in rows {
                let entry = files.entry(row.path.clone()).or_insert_with(|| {
                    order.push(row.path.clone());
                    RestoreFile {
                        snapshot_ids: Vec::new(),
                        path: row.path.clone(),
                        expected_after: None,
                        before_content: None,
                        truncated: false,
                    }
                });
                if entry.snapshot_ids.is_empty() {
                    entry.before_content = row
                        .before_content
                        .clone()
                        .map(|content| (content, row.before_encoding.clone()));
                }
                entry.snapshot_ids.push(row.id);
                entry.expected_after = row.after_sha256.clone();
                if row.truncated {
                    entry.truncated = true;
                }
            }
            let plan = RestorePlan {
                workspace,
                task_id: Some(task_id),
                files: order
                    .into_iter()
                    .filter_map(|path| files.remove(&path))
                    .collect(),
            };
            apply_restore(&st, &actor, &plan).await
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_rejects_traversal_and_absolute_paths() {
        assert!(relative_path("../secret", false).is_err());
        assert!(relative_path("a/../../b", false).is_err());
        assert!(relative_path("C:/Windows", false).is_err());
        assert!(relative_path("/etc/passwd", false).is_err());
        assert!(relative_path(r"\\server\share", false).is_err());
        assert!(relative_path("", false).is_err());
        assert_eq!(relative_path("src/main.rs", false).unwrap(), "src/main.rs");
        assert_eq!(relative_path(r"src\main.rs", false).unwrap(), "src/main.rs");
        assert_eq!(relative_path("", true).unwrap(), "");
    }

    #[test]
    fn join_relative_never_doubles_slashes() {
        assert_eq!(join_relative("", "a.txt"), "a.txt");
        assert_eq!(join_relative("src/", "a.txt"), "src/a.txt");
        assert_eq!(join_relative("src", "a.txt"), "src/a.txt");
    }

    #[test]
    fn upload_name_is_stripped_to_file_name() {
        assert_eq!(
            sanitize_upload_name("C:\\temp\\logo.png").unwrap(),
            "logo.png"
        );
        assert_eq!(sanitize_upload_name("dir/photo.jpg").unwrap(), "photo.jpg");
        assert!(sanitize_upload_name("..").is_err());
        assert!(sanitize_upload_name("bad:name.txt").is_err());
        assert!(sanitize_upload_name("trailing.").is_err());
    }

    #[test]
    fn line_stats_counts_changes() {
        let before = Some(("a\nb\nc".to_owned(), "utf8".to_owned()));
        let after = Some(("a\nB\nc\nd".to_owned(), "utf8".to_owned()));
        let (additions, deletions) = line_stats(before.as_ref(), after.as_ref());
        assert_eq!(deletions, 1);
        assert_eq!(additions, 2);
        let (additions, deletions) = line_stats(None, after.as_ref());
        assert_eq!(additions, 4);
        assert_eq!(deletions, 0);
    }

    #[test]
    fn snapshot_encoding_keeps_text_readable() {
        let text = decode_snapshot(Some(&BASE64.encode(b"hello\nworld")))
            .unwrap()
            .unwrap();
        assert_eq!(text.1, "utf8");
        assert_eq!(text.0, "hello\nworld");
        let binary = decode_snapshot(Some(&BASE64.encode([0u8, 159, 146, 150, 0])))
            .unwrap()
            .unwrap();
        assert_eq!(binary.1, "base64");
    }
}
