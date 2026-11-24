//! Runtime nodes: enrollment, device authentication, исходящий WebSocket,
//! heartbeat, состояние и надёжная доставка команд.
//!
//! Пользовательская cookie-сессия здесь не используется: устройство имеет
//! отдельный отзывной токен, в БД хранится только его SHA-256.

pub mod protocol;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::ai::runtime::protocol::{
    Envelope, ExecutionStatusResult, McpDiscoveryResult, ProgramDescriptor, ProviderChatCompleted,
    ProviderResponse, RuntimeCapabilities, ToolOutcome, WorkspaceUsage, kind,
};
use crate::ai::{ChatChunk, PROTOCOL_VERSION, PendingRequest, errors};
use crate::auth::session::generate_token;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const OUTBOUND_CAPACITY: usize = 256;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);
const HEARTBEAT_INTERVAL: i64 = 20;

#[derive(Debug)]
pub enum RuntimeReply {
    Provider(ProviderChatCompleted),
    ProviderOp(ProviderResponse),
    Tool(Result<ToolOutcome, RuntimeFailure>),
    Failed(RuntimeFailure),
    McpDiscovery(McpDiscoveryResult),
    Usage(WorkspaceUsage),
    ExecutionStatus(ExecutionStatusResult),
}

#[derive(Debug, Clone)]
pub struct RuntimeFailure {
    pub code: String,
    pub message: String,
}

struct Pending {
    node_id: Uuid,
    request: PendingRequest,
}

struct Connection {
    tx: mpsc::Sender<String>,
    close: watch::Sender<bool>,
    connected_at: Instant,
}

/// In-memory реестр живых Runtime-соединений и ожидающих ответов.
pub struct RuntimeHub {
    connections: DashMap<Uuid, Connection>,
    pending: DashMap<Uuid, Pending>,
}

impl RuntimeHub {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            pending: DashMap::new(),
        }
    }

    pub fn is_online(&self, node_id: Uuid) -> bool {
        self.connections.contains_key(&node_id)
    }

    pub fn online_count(&self) -> usize {
        self.connections.len()
    }

    pub fn register(&self, node_id: Uuid) -> (mpsc::Receiver<String>, watch::Receiver<bool>) {
        // Новое соединение вытесняет старое: у устройства один управляющий канал.
        if let Some((_, old)) = self.connections.remove(&node_id) {
            let _ = old.close.send(true);
        }
        let (tx, rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (close_tx, close_rx) = watch::channel(false);
        self.connections.insert(
            node_id,
            Connection {
                tx,
                close: close_tx,
                connected_at: Instant::now(),
            },
        );
        (rx, close_rx)
    }

    pub fn unregister(&self, node_id: Uuid) {
        self.connections.remove(&node_id);
    }

    pub fn send(&self, node_id: Uuid, envelope: &Envelope) -> bool {
        let Some(connection) = self.connections.get(&node_id) else {
            return false;
        };
        match serde_json::to_string(envelope) {
            Ok(payload) => connection.tx.try_send(payload).is_ok(),
            Err(_) => false,
        }
    }

    pub fn pending_register(&self, message_id: Uuid, node_id: Uuid, request: PendingRequest) {
        self.pending
            .insert(message_id, Pending { node_id, request });
    }

    pub fn pending_remove(&self, message_id: Uuid) -> Option<PendingRequest> {
        self.pending
            .remove(&message_id)
            .map(|(_, pending)| pending.request)
    }

    /// При разрыве связи ждём подтверждения выполнения. Неидемпотентные
    /// операции не перезапускаются: вызывающая сторона получает контролируемую
    /// ошибку `AI_EXECUTION_UNKNOWN`.
    pub fn fail_pending_for_node(&self, node_id: Uuid) {
        let ids: Vec<Uuid> = self
            .pending
            .iter()
            .filter(|entry| entry.value().node_id == node_id)
            .map(|entry| *entry.key())
            .collect();
        for id in ids {
            if let Some(pending) = self.pending_remove(id) {
                match pending {
                    PendingRequest::Chat(tx) => {
                        let _ = tx.try_send(ChatChunk::Failed {
                            code: "AI_RUNTIME_DISCONNECTED".to_owned(),
                            message: "Соединение с Runtime прервано.".to_owned(),
                        });
                    }
                    PendingRequest::Reply(tx) => {
                        let _ = tx.send(RuntimeReply::Failed(RuntimeFailure {
                            code: "AI_EXECUTION_UNKNOWN".to_owned(),
                            message:
                                "Результат операции неизвестен: соединение с Runtime прервано. \
                                      Повторное выполнение не запускается автоматически."
                                    .to_owned(),
                        }));
                    }
                }
            }
        }
    }

    pub fn close(&self, node_id: Uuid) {
        if let Some((_, connection)) = self.connections.remove(&node_id) {
            let _ = connection.close.send(true);
        }
        self.fail_pending_for_node(node_id);
    }

    pub fn connected_since(&self, node_id: Uuid) -> Option<Instant> {
        self.connections.get(&node_id).map(|c| c.connected_at)
    }
}

impl Default for RuntimeHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Хэш device token. В БД хранится только это значение.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn new_device_token() -> String {
    format!("nxa_{}", generate_token())
}

pub fn protocol_version_supported(version: i32) -> bool {
    version == PROTOCOL_VERSION
}

#[derive(sqlx::FromRow)]
pub struct RuntimeNodeRow {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub runtime_version: String,
    pub protocol_version: i32,
    pub status: String,
    pub capabilities: Value,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub registered_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub token_rotated_at: Option<DateTime<Utc>>,
}

pub async fn load_node(db: &PgPool, node_id: Uuid) -> ApiResult<RuntimeNodeRow> {
    sqlx::query_as::<_, RuntimeNodeRow>(
        "select id, name, platform, runtime_version, protocol_version, status,
                capabilities, last_seen_at, last_heartbeat_at, registered_at,
                revoked_at, created_by, token_rotated_at
           from ai_runtime_nodes where id = $1",
    )
    .bind(node_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Runtime не найден."))
}

pub async fn node_is_usable(db: &PgPool, node_id: Uuid) -> ApiResult<bool> {
    let status: Option<String> =
        sqlx::query_scalar("select status from ai_runtime_nodes where id = $1")
            .bind(node_id)
            .fetch_optional(db)
            .await?;
    Ok(matches!(status.as_deref(), Some("online" | "degraded")))
}

/// Создаёт одноразовый enrollment code. Открытый код возвращается только здесь.
pub async fn create_enrollment_code(
    state: &AppState,
    actor: &crate::auth::session::AuthUser,
    label: &str,
    ttl_seconds: i64,
    scopes: &[String],
) -> ApiResult<(Uuid, String, DateTime<Utc>)> {
    let code = format!("nxe_{}", generate_token());
    let code_hash = hash_token(&code);
    let ttl = ttl_seconds.clamp(60, 3600);
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl);
    let id: Uuid = sqlx::query_scalar(
        "insert into ai_runtime_enrollment_codes (code_hash, label, scopes, expires_at, created_by)
         values ($1, $2, $3, $4, $5) returning id",
    )
    .bind(&code_hash)
    .bind(label.trim())
    .bind(scopes)
    .bind(expires_at)
    .bind(actor.id)
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.enrollment_created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_runtime_enrollment",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "label": label, "expiresAt": expires_at }),
        },
    )
    .await;
    Ok((id, code, expires_at))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollRequest {
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub runtime_version: String,
    #[serde(default)]
    pub protocol_version: i32,
}

pub async fn enroll(state: &AppState, request: EnrollRequest, ip: &str) -> ApiResult<Value> {
    if !state.module_enabled(crate::ai::MODULE_KEY).await {
        return Err(errors::module_disabled());
    }
    if !protocol_version_supported(request.protocol_version.max(1)) {
        return Err(errors::protocol_unsupported());
    }
    let code = request.code.trim();
    if code.is_empty() || request.name.trim().is_empty() {
        return Err(ApiError::validation(
            "Укажите код регистрации и имя устройства.",
        ));
    }
    let code_hash = hash_token(code);
    let mut tx = state.db.begin().await?;
    let row: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "select id, expires_at from ai_runtime_enrollment_codes
          where code_hash = $1 and used_at is null and revoked_at is null
          for update",
    )
    .bind(&code_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((enrollment_id, expires_at)) = row else {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "AI_ENROLLMENT_INVALID",
            "Код регистрации недействителен, уже использован или отозван.",
        ));
    };
    if expires_at <= Utc::now() {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "AI_ENROLLMENT_EXPIRED",
            "Срок действия кода регистрации истёк.",
        ));
    }

    let token = new_device_token();
    let token_hash = hash_token(&token);
    let node_id: Uuid = sqlx::query_scalar(
        "insert into ai_runtime_nodes
            (name, platform, runtime_version, protocol_version, status, token_hash, created_by)
         values ($1, $2, $3, $4, 'offline', $5,
                 (select created_by from ai_runtime_enrollment_codes where id = $6))
         returning id",
    )
    .bind(request.name.trim().chars().take(120).collect::<String>())
    .bind(request.platform.chars().take(120).collect::<String>())
    .bind(request.runtime_version.chars().take(80).collect::<String>())
    .bind(request.protocol_version)
    .bind(&token_hash)
    .bind(enrollment_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "update ai_runtime_enrollment_codes
            set used_at = now(), used_by_node = $2
          where id = $1",
    )
    .bind(enrollment_id)
    .bind(node_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.enrolled",
            actor_id: None,
            actor_username: "runtime",
            target_type: "ai_runtime_node",
            target_id: &node_id.to_string(),
            result: "success",
            ip,
            details: json!({ "name": request.name, "platform": request.platform }),
        },
    )
    .await;

    Ok(json!({
        "nodeId": node_id,
        "deviceToken": token,
        "protocolVersion": PROTOCOL_VERSION,
        "heartbeatSeconds": HEARTBEAT_INTERVAL,
    }))
}

pub async fn revoke_node(
    state: &AppState,
    actor: &crate::auth::session::AuthUser,
    node_id: Uuid,
) -> ApiResult<()> {
    let result = sqlx::query(
        "update ai_runtime_nodes
            set status = 'revoked', revoked_at = now(), token_hash = concat('revoked:', token_hash)
          where id = $1 and status <> 'revoked'",
    )
    .bind(node_id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        // Идемпотентно: повторный revoke не ошибка, если узел существует.
        let _ = load_node(&state.db, node_id).await?;
    }
    state.ai.runtime.close(node_id);
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.revoked",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_runtime_node",
            target_id: &node_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(())
}

pub async fn set_status(
    state: &AppState,
    actor: &crate::auth::session::AuthUser,
    node_id: Uuid,
    status: &str,
) -> ApiResult<()> {
    if !matches!(status, "online" | "offline" | "disabled") {
        return Err(ApiError::validation("Недопустимый статус Runtime."));
    }
    let stored = if status == "online" {
        "offline"
    } else {
        status
    };
    sqlx::query("update ai_runtime_nodes set status = $2 where id = $1")
        .bind(node_id)
        .bind(stored)
        .execute(&state.db)
        .await?;
    if status != "online" {
        state.ai.runtime.close(node_id);
    }
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.status_changed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_runtime_node",
            target_id: &node_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "status": stored }),
        },
    )
    .await;
    Ok(())
}

pub async fn rotate_token(
    state: &AppState,
    actor: &crate::auth::session::AuthUser,
    node_id: Uuid,
) -> ApiResult<String> {
    let token = new_device_token();
    let hash = hash_token(&token);
    let updated = sqlx::query(
        "update ai_runtime_nodes
            set token_hash = $2, token_rotated_at = now(), status = 'offline'
          where id = $1 and status not in ('revoked')",
    )
    .bind(node_id)
    .bind(&hash)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(errors::runtime_revoked());
    }
    state.ai.runtime.close(node_id);
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.token_rotated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_runtime_node",
            target_id: &node_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(token)
}

/// Отправляет команду Runtime и сохраняет её в outbox.
///
/// `persist_payload` позволяет не записывать секреты (API key) в БД: в outbox
/// остаётся безопасная копия, а ключ подставляется при повторной отправке.
#[allow(clippy::too_many_arguments)]
pub async fn send_command(
    state: &AppState,
    node_id: Uuid,
    message_id: Uuid,
    command_kind: &str,
    correlation_id: Option<Uuid>,
    task_id: Option<Uuid>,
    payload: Value,
    persist_payload: Option<Value>,
) -> ApiResult<Uuid> {
    let node = load_node(&state.db, node_id).await?;
    match node.status.as_str() {
        "revoked" => return Err(errors::runtime_revoked()),
        "disabled" => {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "AI_RUNTIME_DISABLED",
                "Runtime отключён администратором.",
            ));
        }
        _ => {}
    }
    if !state.ai.runtime.is_online(node_id) {
        return Err(errors::runtime_offline());
    }

    let sequence: i64 = sqlx::query_scalar(
        "update ai_runtime_nodes
            set last_server_sequence = last_server_sequence + 1
          where id = $1 and status not in ('revoked', 'disabled')
          returning last_server_sequence",
    )
    .bind(node_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(errors::runtime_offline)?;

    let stored_payload = persist_payload.unwrap_or_else(|| payload.clone());
    sqlx::query(
        "insert into ai_runtime_commands
            (node_id, message_id, sequence, kind, correlation_id, task_id, payload, status, attempts)
         values ($1, $2, $3, $4, $5, $6, $7, 'pending', 0)",
    )
    .bind(node_id)
    .bind(message_id)
    .bind(sequence)
    .bind(command_kind)
    .bind(correlation_id)
    .bind(task_id)
    .bind(&stored_payload)
    .execute(&state.db)
    .await?;

    let envelope = Envelope::server(
        message_id,
        command_kind,
        node_id,
        correlation_id,
        task_id,
        Some(sequence),
        payload,
    );
    if !state.ai.runtime.send(node_id, &envelope) {
        let _ = sqlx::query("delete from ai_runtime_commands where message_id = $1")
            .bind(message_id)
            .execute(&state.db)
            .await;
        return Err(errors::runtime_offline());
    }
    let _ = sqlx::query(
        "update ai_runtime_commands
            set status = 'sent', attempts = attempts + 1, sent_at = now()
          where message_id = $1 and status = 'pending'",
    )
    .bind(message_id)
    .execute(&state.db)
    .await;
    Ok(message_id)
}

pub async fn complete_command(db: &PgPool, message_id: Uuid, ok: bool, error: Option<&str>) {
    let _ = sqlx::query(
        "update ai_runtime_commands
            set status = $2, completed_at = now(), error = $3
          where message_id = $1 and status in ('pending', 'sent')",
    )
    .bind(message_id)
    .bind(if ok { "completed" } else { "failed" })
    .bind(error)
    .execute(db)
    .await;
}

/// Повторная доставка команд, которые не были отправлены до разрыва.
/// Секреты в outbox не хранятся: для provider.chat.start ключ подставляется
/// из актуальной конфигурации провайдера.
async fn resend_pending(state: &AppState, node_id: Uuid) {
    #[derive(sqlx::FromRow)]
    struct CommandRow {
        message_id: Uuid,
        sequence: i64,
        kind: String,
        correlation_id: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: Value,
    }
    let rows = sqlx::query_as::<_, CommandRow>(
        "select message_id, sequence, kind, correlation_id, task_id, payload
           from ai_runtime_commands
          where node_id = $1 and status = 'pending'
          order by sequence",
    )
    .bind(node_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for mut row in rows {
        if row.kind == kind::MCP_CONFIGURE {
            // Конфигурация MCP всегда формируется заново из актуальных
            // зашифрованных секретов; сохранённая копия секретов не содержит.
            complete_command(&state.db, row.message_id, false, Some("reconfigured")).await;
            continue;
        }
        if row.kind == kind::PROVIDER_START {
            if let Some(provider_id) = row
                .payload
                .get("providerId")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            {
                if let Ok(Some(key)) = crate::ai::providers::decrypted_key(state, provider_id).await
                {
                    row.payload["apiKey"] = Value::String(key);
                }
            }
        }
        let envelope = Envelope::server(
            row.message_id,
            &row.kind,
            node_id,
            row.correlation_id,
            row.task_id,
            Some(row.sequence),
            row.payload,
        );
        if state.ai.runtime.send(node_id, &envelope) {
            let _ = sqlx::query(
                "update ai_runtime_commands
                    set status = 'sent', attempts = attempts + 1, sent_at = now()
                  where message_id = $1",
            )
            .bind(row.message_id)
            .execute(&state.db)
            .await;
        }
    }
}

/// `GET /api/v1/ai/runtime/connect` — единственный управляющий канал.
pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    if !state.module_enabled(crate::ai::MODULE_KEY).await {
        return Err(errors::module_disabled());
    }
    let token = bearer_token(&headers).ok_or_else(|| {
        ApiError::unauthorized(
            "AI_RUNTIME_TOKEN_REQUIRED",
            "Требуется device token Runtime.",
        )
    })?;
    let token_hash = hash_token(&token);
    let node = sqlx::query_as::<_, RuntimeNodeRow>(
        "select id, name, platform, runtime_version, protocol_version, status,
                capabilities, last_seen_at, last_heartbeat_at, registered_at,
                revoked_at, created_by, token_rotated_at
           from ai_runtime_nodes where token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await?;
    let Some(node) = node else {
        return Err(ApiError::unauthorized(
            "AI_RUNTIME_TOKEN_INVALID",
            "Device token Runtime недействителен.",
        ));
    };
    if node.status == "revoked" {
        return Err(errors::runtime_revoked());
    }
    if node.status == "disabled" {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "AI_RUNTIME_DISABLED",
            "Runtime отключён администратором.",
        ));
    }
    let node_id = node.id;
    Ok(upgrade
        .on_upgrade(move |socket| async move {
            run_socket(state, socket, node_id).await;
        })
        .into_response())
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn run_socket(state: Arc<AppState>, mut socket: WebSocket, node_id: Uuid) {
    let (mut outbound, mut close_rx) = state.ai.runtime.register(node_id);
    let mut last_message = Instant::now();
    let mut last_client_sequence = 0i64;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;

    if let Err(error) = touch_node(&state, node_id, "online").await {
        tracing::warn!(%error, %node_id, "не удалось обновить состояние Runtime");
    }
    record_runtime_event(
        &state,
        node_id,
        "connected",
        "Runtime подключился",
        json!({}),
    )
    .await;
    let payload = json!({ "nodeId": node_id, "at": Utc::now() });
    state
        .realtime
        .publish_system("ai.runtime.connected", payload.clone());
    state
        .realtime
        .publish_user("ai.runtime.connected", node_id, json!({}));

    loop {
        tokio::select! {
            out = outbound.recv() => {
                match out {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            changed = close_rx.changed() => {
                if changed.is_err() || *close_rx.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if last_message.elapsed() > HEARTBEAT_TIMEOUT {
                    tracing::warn!(%node_id, "Runtime не присылает heartbeat, соединение закрыто");
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        last_message = Instant::now();
                        if let Err(error) = handle_message(state.clone(), node_id, &text, &mut last_client_sequence).await {
                            tracing::warn!(%error, %node_id, "ошибка сообщения Runtime");
                            let error_envelope = Envelope::server(
                                Uuid::new_v4(),
                                kind::RUNTIME_ERROR,
                                node_id,
                                None,
                                None,
                                None,
                                json!({ "code": error.code, "message": error.message }),
                            );
                            let _ = state.ai.runtime.send(node_id, &error_envelope);
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        tracing::info!(%error, %node_id, "WebSocket Runtime закрыт");
                        break;
                    }
                }
            }
        }
    }

    state.ai.runtime.unregister(node_id);
    state.ai.runtime.fail_pending_for_node(node_id);
    let _ = mark_offline(&state, node_id).await;
    record_runtime_event(
        &state,
        node_id,
        "disconnected",
        "Runtime отключился",
        json!({}),
    )
    .await;
    state
        .realtime
        .publish_system("ai.runtime.disconnected", json!({ "nodeId": node_id }));
    let _ = socket.send(Message::Close(None)).await;
}

async fn touch_node(state: &AppState, node_id: Uuid, status: &str) -> ApiResult<()> {
    sqlx::query(
        "update ai_runtime_nodes
            set status = case when status = 'disabled' then status else $2 end,
                last_seen_at = now(), last_heartbeat_at = now()
          where id = $1",
    )
    .bind(node_id)
    .bind(status)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn mark_offline(state: &AppState, node_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "update ai_runtime_nodes
            set status = case when status in ('disabled', 'revoked') then status else 'offline' end,
                last_seen_at = now()
          where id = $1",
    )
    .bind(node_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn record_runtime_event(
    state: &AppState,
    node_id: Uuid,
    event_kind: &str,
    message: &str,
    data: Value,
) {
    let _ = sqlx::query(
        "insert into ai_runtime_events (node_id, kind, message, data)
         values ($1, $2, $3, $4)",
    )
    .bind(node_id)
    .bind(event_kind)
    .bind(message)
    .bind(data)
    .execute(&state.db)
    .await;
}

async fn handle_message(
    state: Arc<AppState>,
    node_id: Uuid,
    text: &str,
    last_client_sequence: &mut i64,
) -> ApiResult<()> {
    let envelope: Envelope = serde_json::from_str(text)
        .map_err(|error| ApiError::validation(format!("Некорректный кадр Runtime: {error}")))?;
    if envelope.node_id != node_id {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "AI_NODE_MISMATCH",
            "node_id не соответствует аутентифицированному устройству.",
        ));
    }
    if !protocol_version_supported(envelope.protocol_version) {
        return Err(errors::protocol_unsupported());
    }
    if let Some(sequence) = envelope.sequence {
        if sequence <= *last_client_sequence {
            tracing::warn!(
                %node_id,
                sequence,
                last = *last_client_sequence,
                kind = %envelope.kind,
                "повторный/устаревший кадр Runtime отклонён"
            );
            return Ok(());
        }
        *last_client_sequence = sequence;
    }
    let age = Utc::now().signed_duration_since(envelope.timestamp);
    if age.num_seconds() > 15 * 60 || age.num_seconds() < -15 * 60 {
        tracing::warn!(
            %node_id,
            age_seconds = age.num_seconds(),
            kind = %envelope.kind,
            "кадр Runtime с недопустимым timestamp отклонён"
        );
        return Ok(());
    }
    let ctx: &AppState = &state;

    match envelope.kind.as_str() {
        kind::HELLO => {
            let caps: RuntimeCapabilities =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            handle_hello(&state, node_id, caps).await?;
        }
        kind::HEARTBEAT => {
            let status = envelope
                .payload
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("ok");
            let runtime_status = if status == "degraded" {
                "degraded"
            } else {
                "online"
            };
            touch_node(ctx, node_id, runtime_status).await?;
            let ack = Envelope::server(
                Uuid::new_v4(),
                kind::HEARTBEAT_ACK,
                node_id,
                Some(envelope.message_id),
                None,
                None,
                json!({ "serverTime": Utc::now() }),
            );
            state.ai.runtime.send(node_id, &ack);
        }
        kind::CAPABILITIES => {
            let caps: RuntimeCapabilities =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            store_capabilities(ctx, node_id, &caps).await?;
            let ack = Envelope::server(
                Uuid::new_v4(),
                kind::CAPABILITIES_ACK,
                node_id,
                Some(envelope.message_id),
                None,
                None,
                json!({ "serverTime": Utc::now() }),
            );
            state.ai.runtime.send(node_id, &ack);
        }
        kind::PROVIDER_DELTA => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let delta = envelope
                .payload
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if let Some(entry) = state.ai.runtime.pending.get(&correlation) {
                if let PendingRequest::Chat(tx) = &entry.request {
                    let _ = tx.try_send(ChatChunk::Delta(delta));
                }
            }
        }
        kind::PROVIDER_COMPLETED => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let completed: ProviderChatCompleted =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            if let Some(request) = state.ai.runtime.pending_remove(correlation) {
                match request {
                    PendingRequest::Chat(tx) => {
                        let _ = tx.try_send(ChatChunk::Done {
                            content: completed.content.clone(),
                            usage: completed.usage.clone(),
                            finish_reason: completed.finish_reason.clone(),
                        });
                    }
                    PendingRequest::Reply(tx) => {
                        let _ = tx.send(RuntimeReply::Provider(completed));
                    }
                }
            }
            complete_command(&state.db, correlation, true, None).await;
            if let Some(task_id) = envelope.task_id {
                publish_task_event(
                    ctx,
                    task_id,
                    "ai.task.progress",
                    "provider.completed",
                    json!({}),
                )
                .await;
            }
        }
        kind::PROVIDER_FAILED => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let code = envelope
                .payload
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("AI_PROVIDER_ERROR")
                .to_owned();
            let message = envelope
                .payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Провайдер вернул ошибку.")
                .to_owned();
            if let Some(request) = state.ai.runtime.pending_remove(correlation) {
                match request {
                    PendingRequest::Chat(tx) => {
                        let _ = tx.try_send(ChatChunk::Failed {
                            code: code.clone(),
                            message: message.clone(),
                        });
                    }
                    PendingRequest::Reply(tx) => {
                        let _ = tx.send(RuntimeReply::Failed(RuntimeFailure {
                            code: code.clone(),
                            message: message.clone(),
                        }));
                    }
                }
            }
            complete_command(&state.db, correlation, false, Some(&message)).await;
        }
        kind::PROVIDER_RESPONSE => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let response: ProviderResponse = serde_json::from_value(envelope.payload.clone())
                .unwrap_or(ProviderResponse {
                    operation: String::new(),
                    ok: false,
                    models: Vec::new(),
                    data: Value::Null,
                    error: Some("Некорректный ответ Runtime.".to_owned()),
                });
            if let Some(PendingRequest::Reply(tx)) = state.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::ProviderOp(response));
            }
            complete_command(&state.db, correlation, true, None).await;
        }
        kind::TOOL_PROGRESS => {
            if let Some(task_id) = envelope.task_id {
                let message = envelope
                    .payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Выполнение инструмента");
                publish_task_event(
                    ctx,
                    task_id,
                    "ai.tool.progress",
                    message,
                    envelope.payload.clone(),
                )
                .await;
            }
        }
        kind::TOOL_COMPLETED => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let outcome: ToolOutcome =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            if let Some(PendingRequest::Reply(tx)) = state.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::Tool(Ok(outcome)));
            }
            complete_command(&state.db, correlation, true, None).await;
        }
        kind::TOOL_FAILED => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let code = envelope
                .payload
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("AI_TOOL_FAILED")
                .to_owned();
            let message = envelope
                .payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Инструмент завершился с ошибкой.")
                .to_owned();
            if let Some(PendingRequest::Reply(tx)) = state.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::Tool(Err(RuntimeFailure {
                    code: code.clone(),
                    message: message.clone(),
                })));
            }
            complete_command(&state.db, correlation, false, Some(&message)).await;
        }
        kind::TASK_CANCEL_ACK => {
            let cancel_message_id = envelope
                .payload
                .get("cancelRequestMessageId")
                .and_then(|value| value.as_str())
                .and_then(|value| Uuid::parse_str(value).ok());
            let accepted = envelope
                .payload
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if let Some(message_id) = cancel_message_id {
                let _ = sqlx::query(
                    "update ai_runtime_commands
                        set acknowledged_at = now()
                      where message_id = $1 and status in ('pending', 'sent')",
                )
                .bind(message_id)
                .execute(&ctx.db)
                .await;
            }
            if let Some(task_id) = envelope.task_id {
                if !accepted {
                    let message = envelope
                        .payload
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("Runtime не принял запрос отмены.");
                    let _ = sqlx::query(
                        "update ai_tasks
                            set cancel_status = 'failed', status_text = $2, updated_at = now()
                          where id = $1 and cancel_status = 'requested'",
                    )
                    .bind(task_id)
                    .bind(message.chars().take(500).collect::<String>())
                    .execute(&ctx.db)
                    .await;
                    record_runtime_event(
                        ctx,
                        node_id,
                        "cancel_rejected",
                        message,
                        json!({ "taskId": task_id }),
                    )
                    .await;
                }
            }
        }
        kind::TASK_CANCELLED => {
            let Some(task_id) = envelope.task_id else {
                return Ok(());
            };
            let confirmed = envelope
                .payload
                .get("confirmed")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let cancel_message_id = envelope
                .payload
                .get("cancelRequestMessageId")
                .and_then(|value| value.as_str())
                .and_then(|value| Uuid::parse_str(value).ok());
            let reason = envelope
                .payload
                .get("reason")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_owned();
            if confirmed {
                let updated = sqlx::query(
                    "update ai_tasks
                        set status = 'cancelled', stage = 'cancelled',
                            status_text = 'Задача отменена и подтверждена Runtime',
                            cancel_status = 'confirmed', cancel_confirmed_at = now(),
                            cancel_message_id = coalesce($2, cancel_message_id),
                            finished_at = coalesce(finished_at, now()), updated_at = now()
                      where id = $1 and cancel_status = 'requested'
                        and status in ('queued', 'running', 'waiting_approval')",
                )
                .bind(task_id)
                .bind(cancel_message_id)
                .execute(&ctx.db)
                .await?;
                if updated.rows_affected() > 0 {
                    if let Some(message_id) = cancel_message_id {
                        complete_command(&ctx.db, message_id, true, None).await;
                    }
                    let _ = sqlx::query(
                        "update ai_tool_executions
                            set status = 'cancelled', error_code = 'AI_TASK_CANCELLED',
                                error_message = 'Задача отменена; Runtime подтвердил остановку.',
                                finished_at = coalesce(finished_at, now()), updated_at = now()
                          where task_id = $1 and status in ('pending', 'running', 'waiting_approval')",
                    )
                    .bind(task_id)
                    .execute(&ctx.db)
                    .await;
                    publish_task_event(
                        ctx,
                        task_id,
                        "ai.task.cancelled",
                        "Задача отменена и подтверждена Runtime.",
                        json!({
                            "cancelConfirmed": true,
                            "terminatedProcesses": envelope
                                .payload
                                .get("terminatedProcesses")
                                .cloned()
                                .unwrap_or(json!(0)),
                        }),
                    )
                    .await;
                    record_runtime_event(
                        ctx,
                        node_id,
                        "task_cancelled",
                        "Runtime подтвердил отмену задачи",
                        json!({ "taskId": task_id, "reason": reason }),
                    )
                    .await;
                }
            } else {
                let _ = sqlx::query(
                    "update ai_tasks
                        set cancel_status = 'failed',
                            status_text = 'Runtime не подтвердил остановку задачи',
                            updated_at = now()
                      where id = $1 and cancel_status = 'requested'",
                )
                .bind(task_id)
                .execute(&ctx.db)
                .await;
                publish_task_event(
                    ctx,
                    task_id,
                    "ai.task.progress",
                    "Runtime не подтвердил остановку задачи",
                    json!({ "cancelConfirmed": false, "reason": reason }),
                )
                .await;
            }
        }
        kind::WORKSPACE_USAGE_RESULT => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let usage: WorkspaceUsage =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            if let Some(PendingRequest::Reply(tx)) = ctx.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::Usage(usage));
            }
            complete_command(&ctx.db, correlation, true, None).await;
        }
        kind::EXECUTION_STATUS_RESULT => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let result: ExecutionStatusResult =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            if let Some(PendingRequest::Reply(tx)) = ctx.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::ExecutionStatus(result));
            }
            complete_command(&ctx.db, correlation, true, None).await;
        }
        kind::MCP_TOOLS => {
            let Some(correlation) = envelope.correlation_id else {
                return Ok(());
            };
            let result: McpDiscoveryResult =
                serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
            if let Some(PendingRequest::Reply(tx)) = ctx.ai.runtime.pending_remove(correlation) {
                let _ = tx.send(RuntimeReply::McpDiscovery(result));
            }
            complete_command(&ctx.db, correlation, true, None).await;
        }
        kind::MCP_CONFIGURE_ACK => {}
        kind::RUNTIME_ERROR => {
            let code = envelope
                .payload
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("AI_RUNTIME_ERROR");
            let message = envelope
                .payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Ошибка Runtime.");
            tracing::warn!(%node_id, code, message, "Runtime сообщил об ошибке");
            record_runtime_event(ctx, node_id, "error", message, envelope.payload.clone()).await;
        }
        _ => {
            tracing::debug!(%node_id, kind = %envelope.kind, "неизвестный тип сообщения Runtime");
        }
    }
    Ok(())
}

async fn handle_hello(
    state: &Arc<AppState>,
    node_id: Uuid,
    caps: RuntimeCapabilities,
) -> ApiResult<()> {
    if !protocol_version_supported(caps.protocol_version.max(1)) {
        return Err(errors::protocol_unsupported());
    }
    sqlx::query(
        "update ai_runtime_nodes
            set status = 'online', runtime_version = $2, platform = $3,
                protocol_version = $4, capabilities = $5,
                last_seen_at = now(), last_heartbeat_at = now()
          where id = $1",
    )
    .bind(node_id)
    .bind(caps.runtime_version.chars().take(80).collect::<String>())
    .bind(caps.platform.chars().take(120).collect::<String>())
    .bind(caps.protocol_version.max(1))
    .bind(serde_json::to_value(&caps).unwrap_or(Value::Null))
    .execute(&state.db)
    .await?;
    store_programs(state, node_id, &caps.programs).await?;
    record_runtime_event(
        state,
        node_id,
        "hello",
        "Runtime установил соединение",
        json!({
            "runtimeVersion": caps.runtime_version,
            "platform": caps.platform,
            "models": caps.models.len(),
            "tools": caps.tools.len(),
            "programs": caps.programs.len(),
        }),
    )
    .await;

    let ack = Envelope::server(
        Uuid::new_v4(),
        kind::HELLO_ACK,
        node_id,
        None,
        None,
        None,
        json!({
            "serverTime": Utc::now(),
            "heartbeatSeconds": HEARTBEAT_INTERVAL,
            "protocolVersion": PROTOCOL_VERSION,
        }),
    );
    state.ai.runtime.send(node_id, &ack);
    resend_pending(state, node_id).await;
    crate::ai::mcp::send_configuration(state, node_id).await;
    crate::ai::orchestrator::reconcile_node(state, node_id).await;
    crate::ai::orchestrator::resume_pending_for_node(state, node_id).await;
    Ok(())
}

async fn store_capabilities(
    state: &AppState,
    node_id: Uuid,
    caps: &RuntimeCapabilities,
) -> ApiResult<()> {
    sqlx::query(
        "update ai_runtime_nodes set capabilities = $2, last_seen_at = now() where id = $1",
    )
    .bind(node_id)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .execute(&state.db)
    .await?;
    store_programs(state, node_id, &caps.programs).await
}

async fn store_programs(
    state: &AppState,
    node_id: Uuid,
    programs: &[ProgramDescriptor],
) -> ApiResult<()> {
    for program in programs {
        sqlx::query(
            "insert into ai_programs
                (runtime_node_id, program_id, name, executable, version, available,
                 allowed_args, risk, network_policy, enabled, updated_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, now())
             on conflict (runtime_node_id, program_id) do update
                set name = excluded.name, executable = excluded.executable,
                    version = excluded.version, available = excluded.available,
                    allowed_args = excluded.allowed_args, risk = excluded.risk,
                    updated_at = now()",
        )
        .bind(node_id)
        .bind(&program.program_id)
        .bind(if program.name.is_empty() {
            &program.program_id
        } else {
            &program.name
        })
        .bind(&program.executable)
        .bind(&program.version)
        .bind(program.available)
        .bind(&program.allowed_args)
        .bind(&program.risk)
        .bind(if program.network_policy == "allow" {
            "allow"
        } else {
            "deny"
        })
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

pub async fn publish_task_event(
    state: &AppState,
    task_id: Uuid,
    kind: &str,
    message: &str,
    data: Value,
) {
    let owner: Option<Uuid> = sqlx::query_scalar("select owner_id from ai_tasks where id = $1")
        .bind(task_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if let Some(owner_id) = owner {
        state.realtime.publish_user(
            kind,
            owner_id,
            json!({
                "taskId": task_id,
                "message": message,
                "data": data,
                "at": Utc::now(),
            }),
        );
    }
}

/// Проверяет свежесть heartbeat в БД (для admin overview).
pub fn effective_status(status: &str, last_seen: Option<DateTime<Utc>>) -> &'static str {
    match status {
        "revoked" => "revoked",
        "disabled" => "disabled",
        "online" | "degraded" => {
            let fresh = last_seen
                .map(|at| Utc::now() - at < chrono::Duration::seconds(90))
                .unwrap_or(false);
            if fresh {
                if status == "degraded" {
                    "degraded"
                } else {
                    "online"
                }
            } else {
                "offline"
            }
        }
        _ => "offline",
    }
}

pub fn capabilities_summary(capabilities: &Value) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    map.insert(
        "models".to_owned(),
        capabilities
            .get("models")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    map.insert(
        "polystrata".to_owned(),
        capabilities
            .get("polystrata")
            .cloned()
            .unwrap_or(Value::Null),
    );
    map.insert(
        "tools".to_owned(),
        capabilities
            .get("tools")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    map.insert(
        "programs".to_owned(),
        capabilities
            .get("programs")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    map.insert(
        "networkIsolation".to_owned(),
        capabilities
            .get("networkIsolation")
            .cloned()
            .unwrap_or(Value::Null),
    );
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_token_is_prefixed_and_hashed() {
        let token = new_device_token();
        assert!(token.starts_with("nxa_"));
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, token);
    }

    #[test]
    fn effective_status_handles_stale_heartbeat() {
        let stale = Some(Utc::now() - chrono::Duration::seconds(200));
        let fresh = Some(Utc::now() - chrono::Duration::seconds(5));
        assert_eq!(effective_status("online", stale), "offline");
        assert_eq!(effective_status("online", fresh), "online");
        assert_eq!(effective_status("revoked", fresh), "revoked");
        assert_eq!(effective_status("disabled", fresh), "disabled");
    }
}
