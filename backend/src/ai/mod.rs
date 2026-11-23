//! NEXUS AI Workspace: AI Chat, Agent orchestration, Runtime control и Skills.
//!
//! Модуль переиспользует общие механизмы NEXUS: cookie-сессии и RBAC, ApiOk/
//! ApiError, audit, module gate, realtime Hub и SQLx-миграции. Runtime
//! аутентифицируется отдельным device token, а не пользовательской сессией.

pub mod admin;
pub mod admin_routes;
pub mod approvals;
pub mod conversations;
pub mod crypto;
pub mod errors;
pub mod events;
pub mod models;
pub mod orchestrator;
pub mod providers;
pub mod routes;
pub mod runtime;
pub mod skills;
pub mod tools;
pub mod workspaces;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ai::crypto::SecretBox;
use crate::ai::runtime::{RuntimeHub, RuntimeReply};
use crate::state::AppState;

/// Ключ модуля в `module_states`.
pub const MODULE_KEY: &str = "ai";
/// Единственная поддерживаемая версия протокола Runtime.
pub const PROTOCOL_VERSION: i32 = 1;

/// Активная генерация чата, привязанная к диалогу.
#[derive(Clone, Debug)]
pub struct ActiveChat {
    pub conversation_id: Uuid,
    pub owner_id: Uuid,
    pub message_id: Uuid,
    pub correlation_id: Uuid,
    pub node_id: Uuid,
}

/// События, которые Runtime присылает в процессе потокового ответа.
#[derive(Debug)]
pub enum ChatChunk {
    Delta(String),
    Done {
        content: String,
        usage: serde_json::Value,
        finish_reason: Option<String>,
    },
    Failed {
        code: String,
        message: String,
    },
    Cancelled,
}

/// Состояние AI-подсистемы процесса.
pub struct AiState {
    pub runtime: RuntimeHub,
    pub secrets: SecretBox,
    /// Активная генерация по диалогу (не более одной на диалог).
    pub chats: DashMap<Uuid, ActiveChat>,
    /// Токены отмены Agent Task.
    pub cancels: DashMap<Uuid, CancellationToken>,
    pub stream_events: AtomicU64,
}

impl AiState {
    pub fn new(secret: &str) -> Self {
        Self {
            runtime: RuntimeHub::new(),
            secrets: SecretBox::from_secret(secret),
            chats: DashMap::new(),
            cancels: DashMap::new(),
            stream_events: AtomicU64::new(0),
        }
    }

    pub fn note_stream_event(&self) {
        self.stream_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn cancel_token(&self, task_id: Uuid) -> CancellationToken {
        self.cancels
            .entry(task_id)
            .or_insert_with(CancellationToken::new)
            .clone()
    }

    pub fn clear_cancel(&self, task_id: Uuid) {
        self.cancels.remove(&task_id);
    }

    /// Регистрирует потоковый чат. Возвращает `false`, если в диалоге уже идёт
    /// генерация: второй параллельный поток в один диалог не допускается.
    pub fn begin_chat(&self, chat: ActiveChat) -> bool {
        if self.chats.contains_key(&chat.conversation_id) {
            return false;
        }
        self.chats.insert(chat.conversation_id, chat);
        true
    }

    pub fn end_chat(&self, conversation_id: Uuid) {
        self.chats.remove(&conversation_id);
    }
}

/// Канал ответа Runtime для нестриминговых операций.
pub enum PendingRequest {
    Chat(mpsc::Sender<ChatChunk>),
    Reply(tokio::sync::oneshot::Sender<RuntimeReply>),
}

/// Восстановление после рестарта: незавершённые задачи и генерации не могут
/// быть продолжены без риска дублировать side effects. Они переводятся в
/// контролируемую ошибку; waiting_approval сохраняется и может быть продолжено
/// после решения пользователя.
pub async fn recover_interrupted(db: &PgPool) -> u64 {
    let mut affected = 0u64;
    if let Ok(result) = sqlx::query(
        "update ai_messages
            set status = 'failed', error_code = 'AI_GENERATION_INTERRUPTED',
                error_message = 'Генерация прервана перезапуском сервера.',
                updated_at = now()
          where status = 'streaming'",
    )
    .execute(db)
    .await
    {
        affected += result.rows_affected();
    }
    if let Ok(result) = sqlx::query(
        "update ai_tasks
            set status = 'failed', stage = 'interrupted',
                status_text = 'Задача прервана перезапуском сервера.',
                error_code = 'AI_TASK_INTERRUPTED',
                error_message = 'Задача прервана перезапуском NEXUS. Side effects не повторяются автоматически.',
                finished_at = now(), updated_at = now()
          where status in ('queued', 'running')",
    )
    .execute(db)
    .await
    {
        affected += result.rows_affected();
    }
    if let Ok(result) = sqlx::query(
        "update ai_runtime_commands
            set status = 'failed', error = 'NEXUS перезапущен', completed_at = now()
          where status in ('pending', 'sent')",
    )
    .execute(db)
    .await
    {
        affected += result.rows_affected();
    }
    if let Ok(result) = sqlx::query(
        "update ai_runtime_nodes
            set status = 'offline', last_seen_at = coalesce(last_seen_at, now())
          where status = 'online'",
    )
    .execute(db)
    .await
    {
        affected += result.rows_affected();
    }
    affected
}

/// Загружает состояние модуля из AppState (для краткости вызовов).
pub fn ai(state: &AppState) -> &AiState {
    &state.ai
}

/// Arc-хелпер для spawn.
pub type SharedState = Arc<AppState>;
