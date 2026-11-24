//! MCP registry: соединения, discovery, назначения и политика доступа.
//!
//! MCP выключен по умолчанию. Соединение создаётся администратором для
//! конкретного Runtime, discovery выполняет сам Runtime по защищённому каналу,
//! а найденные tools проходят те же проверки RBAC, JSON Schema, risk и approval,
//! что и встроенные инструменты. Endpoint проверяется на SSRF: literal
//! loopback/private/link-local адреса запрещены без явного opt-in.

use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::ai::PendingRequest;
use crate::ai::models::WorkspaceRow;
use crate::ai::runtime::protocol::{McpConnectionConfig, McpDiscoveryResult, ToolDefinition, kind};
use crate::ai::runtime::{self, RuntimeReply};
use crate::ai::tools::{self, ToolPolicyRow, ToolSpec};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub const DEFAULT_MCP_TIMEOUT_SECONDS: i64 = 120;
pub const DEFAULT_MCP_MAX_OUTPUT_BYTES: i64 = 262_144;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct McpConnectionRow {
    pub id: Uuid,
    pub name: String,
    pub runtime_node_id: Uuid,
    pub transport: String,
    pub endpoint: String,
    pub secret_cipher: Option<String>,
    pub enabled: bool,
    pub allow_private_network: bool,
    pub status: String,
    pub last_error: Option<String>,
    pub last_discovered_at: Option<DateTime<Utc>>,
    pub discovered_count: i32,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl McpConnectionRow {
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "runtimeNodeId": self.runtime_node_id,
            "transport": self.transport,
            "endpoint": self.endpoint,
            "enabled": self.enabled,
            "allowPrivateNetwork": self.allow_private_network,
            "status": self.status,
            "lastError": self.last_error,
            "lastDiscoveredAt": self.last_discovered_at,
            "discoveredCount": self.discovered_count,
            "hasSecret": self.secret_cipher.is_some(),
            "createdBy": self.created_by,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct McpToolRow {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub tool_name: String,
    pub tool_id: String,
    pub model_name: String,
    pub description: String,
    pub input_schema: Value,
    pub risk: String,
    pub enabled: bool,
    pub discovered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl McpToolRow {
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "connectionId": self.connection_id,
            "name": self.tool_name,
            "toolId": self.tool_id,
            "modelName": self.model_name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "risk": self.risk,
            "enabled": self.enabled,
            "discoveredAt": self.discovered_at,
            "updatedAt": self.updated_at,
        })
    }
}

pub async fn load_connection(db: &PgPool, id: Uuid) -> ApiResult<McpConnectionRow> {
    sqlx::query_as::<_, McpConnectionRow>(
        "select id, name, runtime_node_id, transport, endpoint, secret_cipher, enabled,
                allow_private_network, status, last_error, last_discovered_at,
                discovered_count, created_by, created_at, updated_at
           from ai_mcp_connections where id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("MCP-соединение не найдено."))
}

pub async fn list_connections(db: &PgPool) -> ApiResult<Vec<McpConnectionRow>> {
    Ok(sqlx::query_as::<_, McpConnectionRow>(
        "select id, name, runtime_node_id, transport, endpoint, secret_cipher, enabled,
                allow_private_network, status, last_error, last_discovered_at,
                discovered_count, created_by, created_at, updated_at
           from ai_mcp_connections order by lower(name)",
    )
    .fetch_all(db)
    .await?)
}

pub async fn list_tools_for_connection(db: &PgPool, id: Uuid) -> ApiResult<Vec<McpToolRow>> {
    Ok(sqlx::query_as::<_, McpToolRow>(
        "select id, connection_id, tool_name, tool_id, model_name, description,
                input_schema, risk, enabled, discovered_at, updated_at
           from ai_mcp_tools where connection_id = $1 order by tool_name",
    )
    .bind(id)
    .fetch_all(db)
    .await?)
}

fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                // IPv4-mapped IPv6.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| {
                        v4.is_loopback()
                            || v4.is_private()
                            || v4.is_link_local()
                            || v4.is_unspecified()
                    })
                    .unwrap_or(false)
        }
    }
}

fn literal_ip(host: &str) -> Option<IpAddr> {
    host.trim_matches(['[', ']']).parse::<IpAddr>().ok()
}

/// Проверка endpoint до сохранения и повторно перед discovery.
pub fn validate_endpoint(value: &str, allow_private_network: bool) -> ApiResult<reqwest::Url> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return Err(ApiError::validation("Укажите корректный MCP endpoint."));
    }
    let url = reqwest::Url::parse(trimmed)
        .map_err(|_| ApiError::validation("MCP endpoint должен быть корректным URL."))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::validation(
            "MCP endpoint должен использовать http или https.",
        ));
    }
    if url.username() != "" || url.password().is_some() {
        return Err(ApiError::validation(
            "MCP endpoint не должен содержать учётные данные в URL.",
        ));
    }
    let Some(host) = url.host_str() else {
        return Err(ApiError::validation("MCP endpoint должен содержать host."));
    };
    if url.fragment().is_some() {
        return Err(ApiError::validation(
            "MCP endpoint не должен содержать fragment.",
        ));
    }
    let blocked_name = host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host.eq_ignore_ascii_case("metadata.google.internal");
    if let Some(ip) = literal_ip(host) {
        if !allow_private_network && is_forbidden_ip(ip) {
            return Err(ApiError::validation(
                "MCP endpoint указывает на loopback/private/link-local адрес. \
                 Включите allowPrivateNetwork только если это доверенный локальный MCP-сервер.",
            ));
        }
    } else if blocked_name && !allow_private_network {
        return Err(ApiError::validation(
            "MCP endpoint указывает на localhost. Включите allowPrivateNetwork \
             только если это доверенный локальный MCP-сервер.",
        ));
    }
    Ok(url)
}

#[derive(Debug, sqlx::FromRow)]
struct AssignmentCheck {
    connected: bool,
}

/// Проверяет, что MCP-соединение назначено пользователю/workspace (или роли).
pub async fn assigned(
    db: &PgPool,
    connection_id: Uuid,
    user_id: Uuid,
    workspace_id: Option<Uuid>,
) -> ApiResult<bool> {
    let row: AssignmentCheck = sqlx::query_as(
        "select exists(
            select 1 from ai_mcp_assignments a
             where a.connection_id = $1 and a.enabled = true
               and (
                   (a.scope = 'user' and a.user_id = $2)
                   or (a.scope = 'workspace' and a.workspace_id = $3)
                   or (a.scope = 'role' and a.role_id in (
                       select role_id from user_roles where user_id = $2
                   ))
               )
         ) as connected",
    )
    .bind(connection_id)
    .bind(user_id)
    .bind(workspace_id)
    .fetch_one(db)
    .await?;
    Ok(row.connected)
}

/// Динамически разрешённые MCP-tools для Agent Task.
pub async fn allowed_for_task(
    db: &PgPool,
    user_id: Uuid,
    workspace: &WorkspaceRow,
) -> ApiResult<Vec<(ToolSpec, ToolPolicyRow)>> {
    let allowed_ids: std::collections::HashSet<&str> =
        workspace.allowed_tools.iter().map(String::as_str).collect();
    if allowed_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, McpToolRow>(
        "select t.id, t.connection_id, t.tool_name, t.tool_id, t.model_name,
                t.description, t.input_schema, t.risk, t.enabled,
                t.discovered_at, t.updated_at
           from ai_mcp_tools t
           join ai_mcp_connections c on c.id = t.connection_id
          where c.enabled = true and c.runtime_node_id = $1
            and t.enabled = true
            and exists (
                select 1 from ai_mcp_assignments a
                 where a.connection_id = c.id and a.enabled = true
                   and (
                       (a.scope = 'user' and a.user_id = $2)
                       or (a.scope = 'workspace' and a.workspace_id = $3)
                       or (a.scope = 'role' and a.role_id in (
                           select role_id from user_roles where user_id = $2
                       ))
                   )
            )
          order by t.tool_name",
    )
    .bind(workspace.runtime_node_id)
    .bind(user_id)
    .bind(workspace.id)
    .fetch_all(db)
    .await?;

    let policies = tools::policies(db).await?;
    let mut result = Vec::new();
    for row in rows {
        if !allowed_ids.contains(row.tool_id.as_str()) {
            continue;
        }
        let policy = match policies.iter().find(|policy| policy.tool_id == row.tool_id) {
            Some(policy) => policy.clone(),
            None => continue,
        };
        if !policy.enabled {
            continue;
        }
        result.push((
            ToolSpec {
                id: row.tool_id.clone(),
                model_name: row.model_name.clone(),
                version: 1,
                name: row.tool_name.clone(),
                description: row.description.clone(),
                category: "mcp".to_owned(),
                risk: policy.risk.clone(),
                approval_policy: policy.approval_policy.clone(),
                permission: "ai.tools.use".to_owned(),
                idempotent: false,
                timeout_seconds: policy.max_timeout_seconds.max(1) as i64,
                max_output_bytes: policy.max_output_bytes,
                input_schema: row.input_schema.clone(),
                server_side: false,
                mcp: true,
            },
            policy,
        ));
    }
    Ok(result)
}

/// Повторная проверка доступа к конкретному MCP tool перед dispatch.
pub async fn authorize_tool(
    db: &PgPool,
    tool_id: &str,
    user_id: Uuid,
    workspace: &WorkspaceRow,
) -> ApiResult<McpToolRow> {
    let row = sqlx::query_as::<_, McpToolRow>(
        "select t.id, t.connection_id, t.tool_name, t.tool_id, t.model_name,
                t.description, t.input_schema, t.risk, t.enabled,
                t.discovered_at, t.updated_at
           from ai_mcp_tools t
           join ai_mcp_connections c on c.id = t.connection_id
          where t.tool_id = $1 and c.enabled = true and c.runtime_node_id = $2
            and t.enabled = true",
    )
    .bind(tool_id)
    .bind(workspace.runtime_node_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(crate::ai::errors::tool_not_allowed)?;
    if !workspace.allowed_tools.iter().any(|item| item == tool_id) {
        return Err(crate::ai::errors::tool_not_allowed());
    }
    if !assigned(db, row.connection_id, user_id, Some(workspace.id)).await? {
        return Err(crate::ai::errors::tool_not_allowed());
    }
    Ok(row)
}

/// Формирует безопасный tool_id и имя функции для модели.
pub fn names(connection_id: Uuid, tool_name: &str) -> (String, String) {
    let short = connection_id.simple().to_string();
    let short = &short[..8];
    let sanitized: String = tool_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    let mut suffix = if sanitized.is_empty() {
        "tool".to_owned()
    } else {
        sanitized.chars().take(40).collect::<String>()
    };
    // Детерминированный суффикс на случай обрезки/коллизий.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("{connection_id}:{tool_name}").as_bytes());
    let digest = hex::encode(hasher.finalize());
    suffix.push('_');
    suffix.push_str(&digest[..8]);
    let model_name = format!("mcp_{short}_{suffix}");
    let tool_id = format!("mcp.{connection_id}.{}", sanitize_tool_component(tool_name));
    (tool_id, model_name)
}

fn sanitize_tool_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .take(120)
        .collect()
}

/// Отправляет Runtime актуальную конфигурацию разрешённых MCP-соединений.
pub async fn send_configuration(state: &AppState, node_id: Uuid) {
    if !state.ai.runtime.is_online(node_id) {
        return;
    }
    let rows = sqlx::query_as::<_, McpConnectionRow>(
        "select id, name, runtime_node_id, transport, endpoint, secret_cipher, enabled,
                allow_private_network, status, last_error, last_discovered_at,
                discovered_count, created_by, created_at, updated_at
           from ai_mcp_connections
          where runtime_node_id = $1 and enabled = true
          order by lower(name)",
    )
    .bind(node_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut connections = Vec::new();
    let mut stored = Vec::new();
    for row in rows {
        let secret = match row.secret_cipher.as_deref() {
            Some(cipher) => match state.ai.secrets.decrypt(cipher) {
                Ok(secret) => Some(secret),
                Err(error) => {
                    tracing::warn!(%error, connection = %row.id, "не удалось расшифровать MCP secret");
                    continue;
                }
            },
            None => None,
        };
        connections.push(McpConnectionConfig {
            id: row.id,
            name: row.name.clone(),
            endpoint: row.endpoint.clone(),
            secret,
            allow_private_network: row.allow_private_network,
        });
        // Секрет передаётся по WSS, но не сохраняется в outbox БД.
        stored.push(json!({
            "id": row.id,
            "name": row.name,
            "endpoint": row.endpoint,
            "secret": Value::Null,
            "allowPrivateNetwork": row.allow_private_network,
        }));
    }
    let message_id = Uuid::new_v4();
    let _ = runtime::send_command(
        state,
        node_id,
        message_id,
        kind::MCP_CONFIGURE,
        None,
        None,
        json!({ "connections": connections }),
        Some(json!({ "connections": stored })),
    )
    .await;
}

/// Запускает discovery и сохраняет снимок tools.
pub async fn discover(
    state: &AppState,
    actor: &crate::auth::session::AuthUser,
    connection_id: Uuid,
) -> ApiResult<Value> {
    let connection = load_connection(&state.db, connection_id).await?;
    if !connection.enabled {
        return Err(ApiError::conflict(
            "AI_MCP_DISABLED",
            "MCP-соединение отключено. Включите его перед discovery.",
        ));
    }
    let _ = validate_endpoint(&connection.endpoint, connection.allow_private_network)?;
    if !state.ai.runtime.is_online(connection.runtime_node_id) {
        return Err(crate::ai::errors::runtime_offline());
    }
    // Гарантируем, что Runtime знает конфигурацию и секрет.
    send_configuration(state, connection.runtime_node_id).await;
    let message_id = Uuid::new_v4();
    let (tx, rx) = oneshot::channel();
    state.ai.runtime.pending_register(
        message_id,
        connection.runtime_node_id,
        PendingRequest::Reply(tx),
    );
    if let Err(error) = runtime::send_command(
        state,
        connection.runtime_node_id,
        message_id,
        kind::MCP_DISCOVER,
        Some(message_id),
        None,
        json!({ "connectionId": connection.id }),
        None,
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error);
    }
    let result = match tokio::time::timeout(std::time::Duration::from_secs(35), rx).await {
        Ok(Ok(RuntimeReply::McpDiscovery(result))) => result,
        Ok(Ok(_)) => {
            return Err(ApiError::unavailable(
                "AI_MCP_DISCOVERY_FAILED",
                "Runtime вернул неожиданный ответ discovery.",
                5,
            ));
        }
        Ok(Err(_)) => {
            return Err(ApiError::unavailable(
                "AI_MCP_DISCOVERY_FAILED",
                "Ожидание discovery прервано.",
                5,
            ));
        }
        Err(_) => {
            let _ = state.ai.runtime.pending_remove(message_id);
            return Err(ApiError::unavailable(
                "AI_MCP_DISCOVERY_TIMEOUT",
                "Runtime не ответил на discovery MCP.",
                5,
            ));
        }
    };
    store_discovery(&state.db, connection_id, &result).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.mcp.discovered",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_mcp_connection",
            target_id: &connection_id.to_string(),
            result: if result.ok { "success" } else { "failure" },
            ip: &actor.ip,
            details: json!({
                "tools": result.tools.len(),
                "error": result.error,
            }),
        },
    )
    .await;
    let connection = load_connection(&state.db, connection_id).await?;
    let tools = list_tools_for_connection(&state.db, connection_id).await?;
    Ok(json!({
        "connection": connection.summary(),
        "tools": tools.iter().map(McpToolRow::summary).collect::<Vec<_>>(),
        "error": result.error,
    }))
}

/// Сохраняет discovery-снимок, не теряя tool policies при повторном discovery.
pub async fn store_discovery(
    db: &PgPool,
    connection_id: Uuid,
    result: &McpDiscoveryResult,
) -> ApiResult<()> {
    let mut tx = db.begin().await?;
    if result.ok {
        let mut seen = std::collections::HashSet::new();
        for tool in &result.tools {
            let schema = if tool.input_schema.is_object() {
                tool.input_schema.clone()
            } else {
                json!({ "type": "object", "properties": {} })
            };
            let (tool_id, model_name) = names(connection_id, &tool.name);
            if !seen.insert(tool_id.clone()) {
                continue;
            }
            let risk =
                sqlx::query_scalar::<_, String>("select risk from ai_mcp_tools where tool_id = $1")
                    .bind(&tool_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .unwrap_or_else(|| "high".to_owned());
            sqlx::query(
                "insert into ai_mcp_tools
                    (connection_id, tool_name, tool_id, model_name, description,
                     input_schema, risk, enabled, discovered_at, updated_at)
                 values ($1, $2, $3, $4, $5, $6, $7, true, now(), now())
                 on conflict (tool_id) do update
                    set tool_name = excluded.tool_name,
                        model_name = excluded.model_name,
                        description = excluded.description,
                        input_schema = excluded.input_schema,
                        discovered_at = now(), updated_at = now()",
            )
            .bind(connection_id)
            .bind(&tool.name)
            .bind(&tool_id)
            .bind(&model_name)
            .bind(&tool.description)
            .bind(&schema)
            .bind(&risk)
            .execute(&mut *tx)
            .await?;
            // Политика создаётся один раз; администратор может её изменить.
            sqlx::query(
                "insert into ai_tool_policies
                    (tool_id, enabled, risk, approval_policy, max_timeout_seconds, max_output_bytes)
                 values ($1, true, $2, 'always', $3, $4)
                 on conflict (tool_id) do nothing",
            )
            .bind(&tool_id)
            .bind(&risk)
            .bind(DEFAULT_MCP_TIMEOUT_SECONDS as i32)
            .bind(DEFAULT_MCP_MAX_OUTPUT_BYTES)
            .execute(&mut *tx)
            .await?;
        }
        // Удаляем tools, которых больше нет у сервера, только если discovery
        // вернул непустой список (защита от ошибочного пустого ответа).
        if !result.tools.is_empty() {
            sqlx::query(
                "delete from ai_mcp_tools
                  where connection_id = $1 and tool_name <> all($2)",
            )
            .bind(connection_id)
            .bind(
                result
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>(),
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "update ai_mcp_connections
            set status = $2, last_error = $3, last_discovered_at = now(),
                discovered_count = (select count(*) from ai_mcp_tools where connection_id = $1),
                updated_at = now()
          where id = $1",
    )
    .bind(connection_id)
    .bind(if result.ok { "ok" } else { "error" })
    .bind(result.error.as_deref())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Tool definitions для модели по разрешённым MCP-tools.
pub fn definitions(specs: &[ToolSpec]) -> Vec<ToolDefinition> {
    tools::tool_definitions(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_endpoints_without_opt_in() {
        assert!(validate_endpoint("http://127.0.0.1:3000/mcp", false).is_err());
        assert!(validate_endpoint("http://10.0.0.5/mcp", false).is_err());
        assert!(validate_endpoint("http://169.254.169.254/latest", false).is_err());
        assert!(validate_endpoint("http://localhost:3000/mcp", false).is_err());
        assert!(validate_endpoint("http://127.0.0.1:3000/mcp", true).is_ok());
        assert!(validate_endpoint("https://mcp.example.com/mcp", false).is_ok());
    }

    #[test]
    fn rejects_unsafe_url_shapes() {
        assert!(validate_endpoint("file:///etc/passwd", false).is_err());
        assert!(validate_endpoint("http://user:pass@example.com/mcp", false).is_err());
        assert!(validate_endpoint("http://example.com/mcp#frag", false).is_err());
    }

    #[test]
    fn names_are_stable_and_safe() {
        let id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let (tool_id, model_name) = names(id, "read/file test");
        assert!(tool_id.starts_with("mcp.11111111-2222-3333-4444-555555555555."));
        assert!(model_name.starts_with("mcp_11111111_"));
        assert!(
            model_name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        );
    }
}
