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
pub mod mcp;
pub mod models;
pub mod orchestrator;
pub mod providers;
pub mod quota;
pub mod routes;
pub mod runtime;
pub mod skills;
pub mod tools;
pub mod workspace_files;
pub mod workspace_lifecycle;
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
        tool_calls: Vec<crate::ai::runtime::protocol::ToolCall>,
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
    /// AgentRun, цикл которого уже выполняется в этом процессе. Защищает от
    /// параллельного запуска одного task_id из start/resume/reconcile.
    pub runs: DashMap<Uuid, ()>,
    pub stream_events: AtomicU64,
}

impl AiState {
    pub fn new(secret: &str) -> Self {
        Self {
            runtime: RuntimeHub::new(),
            secrets: SecretBox::from_secret(secret),
            chats: DashMap::new(),
            cancels: DashMap::new(),
            runs: DashMap::new(),
            stream_events: AtomicU64::new(0),
        }
    }

    /// Пытается стать единственным исполнителем задачи.
    pub fn begin_run(&self, task_id: Uuid) -> bool {
        self.runs.insert(task_id, ()).is_none()
    }

    pub fn end_run(&self, task_id: Uuid) {
        self.runs.remove(&task_id);
    }

    pub fn note_stream_event(&self) {
        self.stream_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn cancel_token(&self, task_id: Uuid) -> CancellationToken {
        self.cancels.entry(task_id).or_default().clone()
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

/// Итог восстановления после рестарта Backend.
#[derive(Debug, Default)]
pub struct RecoveryReport {
    /// Задачи, которые безопасно продолжить (нет незавершённого side effect).
    pub resume: Vec<Uuid>,
    /// Задачи с запрошенной отменой: нужно повторно отправить task.cancel.
    pub cancel: Vec<(Uuid, Uuid)>,
    /// Задачи, ожидающие сверки receipt с Runtime.
    pub reconcile: Vec<(Uuid, Uuid)>,
    /// Сообщения/команды, переведённые в контролируемую ошибку.
    pub failed_messages: u64,
    pub failed_commands: u64,
    pub offline_nodes: u64,
}

impl RecoveryReport {
    pub fn affected(&self) -> u64 {
        self.resume.len() as u64
            + self.cancel.len() as u64
            + self.reconcile.len() as u64
            + self.failed_messages
            + self.failed_commands
            + self.offline_nodes
    }
}

/// Восстановление после рестарта.
///
/// * `streaming` сообщения не продолжаются — их нельзя безопасно дописать.
/// * `queued` задачи никогда не отправлялись и безопасно перезапускаются.
/// * `running` без незавершённого tool execution безопасно продолжает модельный
///   цикл (повторный вызов модели — идемпотентное чтение, side effects нет).
/// * `running` с незавершённым execution не повторяется: задача ждёт сверки
///   receipt с Runtime после reconnect. Если receipt недоступен, watchdog
///   завершит её контролируемой ошибкой `AI_EXECUTION_UNKNOWN`.
/// * `waiting_approval` сохраняется как есть.
/// * `cancel_requested` задачи остаются running/cancelling до подтверждения.
pub async fn recover_interrupted(db: &PgPool) -> RecoveryReport {
    let mut report = RecoveryReport::default();

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
        report.failed_messages = result.rows_affected();
    }

    // Команды, отправленные до рестарта, не считаются выполненными.
    if let Ok(result) = sqlx::query(
        "update ai_runtime_commands
            set status = 'failed', error = 'NEXUS перезапущен', completed_at = now()
          where status in ('pending', 'sent')",
    )
    .execute(db)
    .await
    {
        report.failed_commands = result.rows_affected();
    }

    if let Ok(result) = sqlx::query(
        "update ai_runtime_nodes
            set status = 'offline', last_seen_at = coalesce(last_seen_at, now()),
                connected_at = null
          where status = 'online'",
    )
    .execute(db)
    .await
    {
        report.offline_nodes = result.rows_affected();
    }

    // Согласованность: execution ждёт approval, а task остался running.
    let _ = sqlx::query(
        "update ai_tasks t
            set status = 'waiting_approval', stage = 'waiting_approval',
                status_text = 'Ожидается подтверждение действия', updated_at = now()
          where t.status = 'running'
            and exists (
                select 1 from ai_tool_executions e
                 where e.task_id = t.id and e.status = 'waiting_approval'
            )",
    )
    .execute(db)
    .await;

    // Задачи с запрошенной отменой: повторно отправим cancel после reconnect.
    if let Ok(rows) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "update ai_tasks
            set recovery_state = 'cancel_pending', recovered_at = now(),
                cancel_status = 'requested', updated_at = now()
          where status in ('queued', 'running') and cancel_requested = true
          returning id, runtime_node_id",
    )
    .fetch_all(db)
    .await
    {
        report.cancel = rows;
    }

    // Queued-задачи безопасно запускаются заново.
    if let Ok(rows) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "update ai_tasks
            set recovery_state = 'requeued', recovered_at = now(), updated_at = now()
          where status = 'queued' and cancel_requested = false
          returning id, runtime_node_id",
    )
    .fetch_all(db)
    .await
    {
        report.resume = rows.into_iter().map(|(id, _)| id).collect();
    }

    // Running без незавершённых executions можно продолжить.
    if let Ok(rows) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "update ai_tasks t
            set recovery_state = 'resumed', recovered_at = now(), updated_at = now()
          where t.status = 'running' and t.cancel_requested = false
            and not exists (
                select 1 from ai_tool_executions e
                 where e.task_id = t.id
                   and e.status in ('pending', 'running', 'waiting_approval')
            )
          returning t.id, t.runtime_node_id",
    )
    .fetch_all(db)
    .await
    {
        report.resume.extend(rows.into_iter().map(|(id, _)| id));
    }

    // Running с незавершённым dispatch: ждём receipt от Runtime.
    if let Ok(rows) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "update ai_tasks t
            set recovery_state = 'awaiting_receipt', recovered_at = now(),
                stage = 'recovering', status_text = 'Ожидается сверка с Runtime после перезапуска',
                updated_at = now()
          where t.status = 'running' and t.cancel_requested = false
            and exists (
                select 1 from ai_tool_executions e
                 where e.task_id = t.id and e.dispatch_message_id is not null
                   and e.status in ('pending', 'running')
            )
          returning t.id, t.runtime_node_id",
    )
    .fetch_all(db)
    .await
    {
        for (task_id, node_id) in rows {
            if let Ok(Some(message_id)) = sqlx::query_scalar::<_, Uuid>(
                "select dispatch_message_id from ai_tool_executions
                  where task_id = $1 and dispatch_message_id is not null
                    and status in ('pending', 'running')
                  order by created_at limit 1",
            )
            .bind(task_id)
            .fetch_optional(db)
            .await
            {
                report.reconcile.push((task_id, message_id));
            } else {
                report.reconcile.push((task_id, Uuid::nil()));
            }
            let _ = node_id;
        }
    }

    report
}

/// Загружает состояние модуля из AppState (для краткости вызовов).
pub fn ai(state: &AppState) -> &AiState {
    &state.ai
}

/// Arc-хелпер для spawn.
pub type SharedState = Arc<AppState>;
