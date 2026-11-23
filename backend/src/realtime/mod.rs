//! Realtime-канал прогресса (PLAN §11, §9.3).
//!
//! Один процесс публикует события в broadcast-канал. Каждое WebSocket-соединение
//! получает только адресованные ему события (свои пользовательские и системные,
//! если у пользователя есть `system.monitoring.view`). Канал дополняет опрос,
//! а не заменяет его: клиент продолжает работать при прокси без Upgrade.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::auth::session::{SESSION_COOKIE, cookie_value, load_user_for_token};
use crate::core::error::ApiError;
use crate::state::AppState;

/// Ёмкость канала. Медленный клиент теряет старые события, но не блокирует других.
const CHANNEL_CAPACITY: usize = 512;

/// Сколько соединений допускается на одного пользователя.
pub const MAX_CONNECTIONS_PER_USER: usize = 3;

/// Событие realtime-канала.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub kind: String,
    /// `None` — системное событие для администраторов мониторинга.
    pub user_id: Option<Uuid>,
    pub payload: Value,
    #[serde(default = "chrono::Utc::now")]
    pub at: chrono::DateTime<chrono::Utc>,
}

impl Event {
    pub fn user(kind: &str, user_id: Uuid, payload: Value) -> Self {
        Self {
            kind: kind.to_owned(),
            user_id: Some(user_id),
            payload,
            at: chrono::Utc::now(),
        }
    }

    pub fn system(kind: &str, payload: Value) -> Self {
        Self {
            kind: kind.to_owned(),
            user_id: None,
            payload,
            at: chrono::Utc::now(),
        }
    }
}

struct Connection {
    id: Uuid,
    user_id: Uuid,
    since: Instant,
    /// Сигнал принудительного закрытия (при превышении лимита соединений).
    close: mpsc::Sender<()>,
}

/// Брокер событий.
pub struct Hub {
    tx: broadcast::Sender<Event>,
    connections: Arc<std::sync::Mutex<Vec<Connection>>>,
    dropped: AtomicU64,
}

impl Clone for Hub {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            connections: self.connections.clone(),
            dropped: AtomicU64::new(0),
        }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            tx,
            connections: Arc::new(std::sync::Mutex::new(Vec::new())),
            dropped: AtomicU64::new(0),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Публикует событие. Если подписчиков нет, событие отбрасывается без ошибки.
    pub fn publish(&self, event: Event) {
        if self.tx.send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn publish_user(&self, kind: &str, user_id: Uuid, payload: Value) {
        self.publish(Event::user(kind, user_id, payload));
    }

    pub fn publish_system(&self, kind: &str, payload: Value) {
        self.publish(Event::system(kind, payload));
    }

    /// Публикует `upload.failed` владельцу загрузки.
    ///
    /// В событие попадают только безопасные поля: ни внутренних путей, ни SQL,
    /// ни текста технической ошибки. `message` всегда должен быть безопасным
    /// текстом из `ApiError` или статической строкой.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_upload_failed(
        &self,
        user_id: Uuid,
        upload_id: Uuid,
        entry_name: Option<&str>,
        code: &str,
        message: &str,
        uploaded: i64,
        size: Option<i64>,
    ) {
        let mut payload = json!({
            "uploadId": upload_id,
            "code": code,
            "message": message,
            "uploaded": uploaded,
            "size": size,
        });
        if let Some(name) = entry_name {
            payload["entryName"] = json!(name);
        }
        self.publish_user("upload.failed", user_id, payload);
    }

    /// Число живых соединений.
    pub fn active(&self) -> u64 {
        self.connections
            .lock()
            .map(|guard| guard.len() as u64)
            .unwrap_or(0)
    }

    /// Регистрирует соединение. Если у пользователя уже максимум соединений,
    /// старейшее закрывается (получает сигнал по каналу `close`).
    fn register(&self, user_id: Uuid) -> (Uuid, mpsc::Receiver<()>) {
        let id = Uuid::new_v4();
        let (close_tx, close_rx) = mpsc::channel(1);
        let Ok(mut guard) = self.connections.lock() else {
            return (id, close_rx);
        };
        let mut indices: Vec<usize> = guard
            .iter()
            .enumerate()
            .filter(|(_, conn)| conn.user_id == user_id)
            .map(|(index, _)| index)
            .collect();
        while indices.len() >= MAX_CONNECTIONS_PER_USER {
            indices.sort_by_key(|index| guard[*index].since);
            let oldest = indices.remove(0);
            let victim = guard.remove(oldest);
            // Сигнал отправителя: получатель закроет сокет и снимет себя из списка.
            let _ = victim.close.try_send(());
        }
        guard.push(Connection {
            id,
            user_id,
            since: Instant::now(),
            close: close_tx,
        });
        (id, close_rx)
    }

    fn unregister(&self, id: Uuid) {
        if let Ok(mut guard) = self.connections.lock() {
            guard.retain(|conn| conn.id != id);
        }
    }
}

/// `GET /api/v1/ws` — авторизация по cookie-сессии и поток событий.
pub async fn ws_handler(
    State(state): State<std::sync::Arc<AppState>>,
    headers: axum::http::HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    let token = cookie_value(&headers, SESSION_COOKIE).ok_or_else(ApiError::session_expired)?;
    let user = load_user_for_token(&state, &token).await?;
    let can_system = user.can("system.monitoring.view");
    let user_id = user.id;

    Ok(upgrade
        .on_upgrade(move |socket| async move {
            run_socket(state, socket, user_id, can_system).await;
        })
        .into_response())
}

async fn run_socket(
    state: std::sync::Arc<AppState>,
    mut socket: WebSocket,
    user_id: Uuid,
    can_system: bool,
) {
    let hub = &state.realtime;
    let (id, mut close_rx) = hub.register(user_id);
    let mut rx = hub.subscribe();

    let hello = json!({
        "kind": "connected",
        "userId": user_id,
        "system": can_system,
        "activeConnections": hub.active(),
    });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        hub.unregister(id);
        return;
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Ok(event) => {
                        let allowed = match event.user_id {
                            Some(owner) => owner == user_id,
                            None => can_system,
                        };
                        if !allowed {
                            continue;
                        }
                        let payload = serde_json::to_string(&event).unwrap_or_default();
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let notice = json!({ "kind": "lagged", "skipped": skipped });
                        if socket.send(Message::Text(notice.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = close_rx.recv() => {
                let _ = socket
                    .send(Message::Text(
                        json!({ "kind": "closed", "reason": "connection_limit" })
                            .to_string()
                            .into(),
                    ))
                    .await;
                break;
            }
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    hub.unregister(id);
    let _ = socket.send(Message::Close(None)).await;
}

/// Счётчик соединений для метрик.
pub fn active_connections(state: &AppState) -> u64 {
    state.realtime.active()
}
