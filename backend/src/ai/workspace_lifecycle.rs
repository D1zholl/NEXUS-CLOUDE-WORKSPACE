//! Полный жизненный цикл AI Workspace: мягкое удаление, восстановление,
//! очередь физической очистки и подтверждение владения каталогом Runtime.
//!
//! HTTP-обработчики не удаляют файлы. Они фиксируют намерение в
//! `ai_workspace_lifecycle`; отдельный фоновый worker отправляет Runtime
//! `workspace.delete`, ждёт подтверждения и только затем очищает snapshots.
//! Runtime повторно проверяет, что путь — это
//! `<provisioning_root>/<workspaceId>` с совпадающим маркером.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ai::models::WorkspaceRow;
use crate::ai::runtime::RuntimeReply;
use crate::ai::runtime::protocol::kind;
use crate::ai::runtime::{RuntimeFailure, send_command};
use crate::ai::{PendingRequest, workspaces};
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const PURGE_COMMAND_TIMEOUT_SECONDS: u64 = 90;
const PURGE_LEASE_MINUTES: i64 = 10;
const PURGE_BATCH_LIMIT: usize = 5;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct LifecycleRow {
    pub workspace_id: Uuid,
    pub owner_id: Uuid,
    pub runtime_node_id: Uuid,
    pub original_name: String,
    pub original_description: String,
    pub state: String,
    pub requested_by: Option<Uuid>,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub purge_after: chrono::DateTime<chrono::Utc>,
    pub attempts: i32,
    pub last_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
    pub lease_until: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
    pub purged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub restored_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bytes_freed: i64,
    pub files_removed: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl LifecycleRow {
    pub fn view(&self, include_error: bool) -> Value {
        json!({
            "workspaceId": self.workspace_id,
            "ownerId": self.owner_id,
            "runtimeNodeId": self.runtime_node_id,
            "originalName": self.original_name,
            "state": self.state,
            "requestedBy": self.requested_by,
            "requestedAt": self.requested_at,
            "purgeAfter": self.purge_after,
            "attempts": self.attempts,
            "lastAttemptAt": self.last_attempt_at,
            "purgedAt": self.purged_at,
            "restoredAt": self.restored_at,
            "bytesFreed": self.bytes_freed,
            "filesRemoved": self.files_removed,
            "updatedAt": self.updated_at,
            "error": if include_error { self.last_error.clone() } else { None },
        })
    }

    fn restorable(&self) -> bool {
        matches!(self.state.as_str(), "soft_deleted" | "pending" | "retry")
    }
}

const SELECT_LIFECYCLE: &str = "select workspace_id, owner_id, runtime_node_id, original_name,
        original_description, state, requested_by, requested_at, purge_after, attempts,
        last_attempt_at, lease_until, last_error, purged_at, restored_at, bytes_freed,
        files_removed, updated_at
   from ai_workspace_lifecycle";

pub async fn load_lifecycle(
    db: &sqlx::PgPool,
    workspace_id: Uuid,
) -> ApiResult<Option<LifecycleRow>> {
    Ok(
        sqlx::query_as::<_, LifecycleRow>(&format!("{SELECT_LIFECYCLE} where workspace_id = $1"))
            .bind(workspace_id)
            .fetch_optional(db)
            .await?,
    )
}

/// Фиксирует мягкое удаление в жизненном цикле. Вызывается внутри транзакции
/// удаления, чтобы workspace и очередь не разошлись.
pub async fn insert_soft_delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: &WorkspaceRow,
    actor_id: Uuid,
    retention_days: i64,
) -> ApiResult<()> {
    let purge_after = chrono::Utc::now() + chrono::Duration::days(retention_days.clamp(1, 3650));
    sqlx::query(
        "insert into ai_workspace_lifecycle
            (workspace_id, owner_id, runtime_node_id, original_name, original_description,
             state, requested_by, requested_at, purge_after)
         values ($1, $2, $3, $4, $5, 'soft_deleted', $6, now(), $7)
         on conflict (workspace_id) do update
            set original_name = excluded.original_name,
                original_description = excluded.original_description,
                state = 'soft_deleted',
                requested_by = excluded.requested_by,
                requested_at = now(),
                purge_after = excluded.purge_after,
                last_error = null,
                restored_at = null,
                updated_at = now()",
    )
    .bind(workspace.id)
    .bind(workspace.owner_id)
    .bind(workspace.runtime_node_id)
    .bind(&workspace.name)
    .bind(&workspace.description)
    .bind(actor_id)
    .bind(purge_after)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Список мягко удалённых workspace владельца (для восстановления).
pub async fn list_deleted_for_owner(db: &sqlx::PgPool, owner_id: Uuid) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query_as::<_, LifecycleRow>(&format!(
        "{SELECT_LIFECYCLE}
          where owner_id = $1 and state in ('soft_deleted', 'pending', 'retry')
          order by requested_at desc"
    ))
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(|row| row.view(false)).collect())
}

/// Восстановление до физической очистки.
pub async fn restore_for_owner(
    state: &AppState,
    actor: &AuthUser,
    workspace_id: Uuid,
) -> ApiResult<WorkspaceRow> {
    let workspace = workspaces::load_workspace(&state.db, workspace_id).await?;
    let admin = actor.can("ai.admin.manage");
    if workspace.owner_id != actor.id && !admin {
        return Err(ApiError::not_found("Рабочее пространство AI не найдено."));
    }
    if workspace.deleted_at.is_none() {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_NOT_DELETED",
            "Рабочее пространство не находится в удалённом состоянии.",
        ));
    }
    let lifecycle = load_lifecycle(&state.db, workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Запись жизненного цикла не найдена."))?;
    if lifecycle.owner_id != workspace.owner_id {
        return Err(ApiError::not_found("Рабочее пространство AI не найдено."));
    }
    if !lifecycle.restorable() {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_RESTORE_UNAVAILABLE",
            match lifecycle.state.as_str() {
                "running" => "Физическая очистка уже выполняется. Восстановление невозможно.",
                "purged" => "Файлы workspace уже удалены физически. Восстановление невозможно.",
                _ => "Восстановление этого workspace недоступно.",
            },
        ));
    }
    if !state.ai.runtime.is_online(workspace.runtime_node_id) {
        return Err(ApiError::unavailable(
            "AI_RUNTIME_OFFLINE",
            "Runtime недоступен: невозможно подтвердить сохранность данных workspace. \
             Повторите восстановление после подключения Runtime.",
            10,
        ));
    }
    // Проверяем, что каталог и данные доступны, до изменения состояния.
    let usage = workspaces::refresh_usage(state, workspace_id).await?;
    if let Some(error) = usage.error.as_deref() {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_DATA_UNAVAILABLE",
            format!("Runtime не подтвердил доступность данных: {error}"),
        ));
    }
    let name = unique_restored_name(
        state,
        workspace.owner_id,
        &lifecycle.original_name,
        workspace_id,
    )
    .await?;
    let mut tx = state.db.begin().await?;
    // Повторно читаем состояние под блокировкой: worker мог начать очистку.
    let current_state: Option<String> = sqlx::query_scalar(
        "select state from ai_workspace_lifecycle where workspace_id = $1 for update",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    match current_state.as_deref() {
        Some("soft_deleted" | "pending" | "retry") => {}
        _ => {
            return Err(ApiError::conflict(
                "AI_WORKSPACE_RESTORE_UNAVAILABLE",
                "Состояние workspace изменилось: физическая очистка уже началась.",
            ));
        }
    }
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "update ai_workspaces
            set deleted_at = null, name = $2, description = $3, updated_at = now()
          where id = $1 and deleted_at is not null
          returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                    max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                    allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                    network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                    description, deleted_at, created_by, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&name)
    .bind(&lifecycle.original_description)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Рабочее пространство AI не найдено."))?;
    sqlx::query(
        "update ai_workspace_lifecycle
            set state = 'restored', restored_at = now(), lease_until = null,
                last_error = null, updated_at = now()
          where workspace_id = $1",
    )
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.restored",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &workspace_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "name": row.name,
                "renamed": row.name != lifecycle.original_name,
                "usageBytes": usage.bytes,
                "usageFiles": usage.files,
            }),
        },
    )
    .await;
    state.realtime.publish_system(
        "ai.workspace.restored",
        json!({ "workspaceId": workspace_id, "ownerId": row.owner_id }),
    );
    Ok(row)
}

async fn unique_restored_name(
    state: &AppState,
    owner_id: Uuid,
    original: &str,
    workspace_id: Uuid,
) -> ApiResult<String> {
    for suffix in 0..5 {
        let candidate = match suffix {
            0 => original.to_owned(),
            n => format!("{original} (восстановлен {n})"),
        };
        let taken: bool = sqlx::query_scalar(
            "select exists(
                select 1 from ai_workspaces
                 where owner_id = $1 and lower(name) = lower($2)
                   and id <> $3 and deleted_at is null
            )",
        )
        .bind(owner_id)
        .bind(&candidate)
        .bind(workspace_id)
        .fetch_one(&state.db)
        .await?;
        if !taken {
            return Ok(candidate);
        }
    }
    Err(ApiError::conflict(
        "AI_WORKSPACE_NAME_CONFLICT",
        "Не удалось подобрать свободное имя для восстановления. Переименуйте конфликтующий workspace.",
    ))
}

// ---------------------------------------------------------------------------
// Фоновая физическая очистка
// ---------------------------------------------------------------------------

/// Запускает worker очистки. Один экземпляр на процесс; захват строк
/// выполняется через `FOR UPDATE SKIP LOCKED` и lease, поэтому повторный
/// запуск безопасен.
pub fn spawn_purge_worker(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match run_due(&state, PURGE_BATCH_LIMIT).await {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "AI workspace purge worker обработал очередь"),
                Err(error) => tracing::warn!(%error, "AI workspace purge worker"),
            }
        }
    });
}

/// Обрабатывает до `limit` созревших записей. Публичная для тестов и ручного
/// запуска администратором.
pub async fn run_due(state: &AppState, limit: usize) -> ApiResult<usize> {
    let mut processed = 0;
    for _ in 0..limit {
        let Some(row) = claim_due(state).await? else {
            break;
        };
        purge_one(state, row).await;
        processed += 1;
    }
    Ok(processed)
}

#[derive(Debug, sqlx::FromRow)]
struct ClaimedPurge {
    workspace_id: Uuid,
    runtime_node_id: Uuid,
    owner_id: Uuid,
    root_path: String,
    attempts: i32,
    original_name: String,
}

async fn claim_due(state: &AppState) -> ApiResult<Option<ClaimedPurge>> {
    let row = sqlx::query_as::<_, ClaimedPurge>(
        "with due as (
            select workspace_id from ai_workspace_lifecycle
             where state in ('soft_deleted', 'pending', 'retry')
               and purge_after <= now()
               and (lease_until is null or lease_until < now())
             order by purge_after
             for update skip locked
             limit 1
         )
         update ai_workspace_lifecycle l
            set state = 'running', attempts = l.attempts + 1,
                last_attempt_at = now(), lease_until = now() + make_interval(mins => $1),
                updated_at = now()
           from due, ai_workspaces w
          where l.workspace_id = due.workspace_id and w.id = l.workspace_id
         returning l.workspace_id, l.runtime_node_id, l.owner_id, w.root_path,
                   l.attempts, l.original_name",
    )
    .bind(PURGE_LEASE_MINUTES as i32)
    .fetch_optional(&state.db)
    .await?;
    Ok(row)
}

async fn purge_one(state: &AppState, row: ClaimedPurge) {
    let error = match command_delete(state, &row).await {
        Ok(result) if result.ok => {
            if let Err(error) = mark_purged(state, &row, result.bytes, result.files).await {
                tracing::warn!(%error, workspace = %row.workspace_id, "не удалось завершить очистку");
            }
            return;
        }
        Ok(result) => result
            .error
            .unwrap_or_else(|| "Runtime не подтвердил удаление workspace.".to_owned()),
        Err(error) => error,
    };
    if let Err(error) = schedule_retry(state, &row, &error).await {
        tracing::warn!(%error, workspace = %row.workspace_id, "не удалось запланировать повтор очистки");
    }
}

async fn command_delete(
    state: &AppState,
    row: &ClaimedPurge,
) -> Result<crate::ai::runtime::protocol::WorkspaceDeleteResult, String> {
    if !state.ai.runtime.is_online(row.runtime_node_id) {
        return Err("Runtime офлайн: физическое удаление отложено.".to_owned());
    }
    let message_id = Uuid::new_v4();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .ai
        .runtime
        .pending_register(message_id, row.runtime_node_id, PendingRequest::Reply(tx));
    if let Err(error) = send_command(
        state,
        row.runtime_node_id,
        message_id,
        kind::WORKSPACE_DELETE,
        Some(message_id),
        None,
        json!({
            "workspaceId": row.workspace_id,
            "workspaceRoot": row.root_path,
        }),
        None,
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error.message);
    }
    let reply = tokio::time::timeout(Duration::from_secs(PURGE_COMMAND_TIMEOUT_SECONDS), rx)
        .await
        .map_err(|_| "Runtime не подтвердил удаление за отведённое время.".to_owned())?;
    match reply {
        Ok(RuntimeReply::Delete(result)) => Ok(result),
        Ok(RuntimeReply::Failed(RuntimeFailure { message, .. })) => Err(message),
        _ => Err(
            "Результат физического удаления неизвестен: Runtime не вернул подтверждение."
                .to_owned(),
        ),
    }
}

async fn mark_purged(
    state: &AppState,
    row: &ClaimedPurge,
    bytes: i64,
    files: i64,
) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "update ai_workspace_lifecycle
            set state = 'purged', purged_at = now(), lease_until = null,
                last_error = null, bytes_freed = $2, files_removed = $3, updated_at = now()
          where workspace_id = $1",
    )
    .bind(row.workspace_id)
    .bind(bytes.max(0))
    .bind(files.max(0))
    .execute(&mut *tx)
    .await?;
    // Снимки могут содержать копии пользовательских данных: после
    // подтверждённого физического удаления они больше не нужны.
    let snapshots = sqlx::query("delete from ai_file_snapshots where workspace_id = $1")
        .bind(row.workspace_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let _ = sqlx::query("delete from ai_workspace_usage_samples where workspace_id = $1")
        .bind(row.workspace_id)
        .execute(&mut *tx)
        .await;
    sqlx::query(
        "update ai_workspaces
            set usage_bytes = 0, usage_files = 0, usage_checked_at = now(),
                usage_error = null, updated_at = now()
          where id = $1",
    )
    .bind(row.workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "insert into ai_workspace_purge_events
            (workspace_id, runtime_node_id, event, attempt, bytes_freed, files_removed)
         values ($1, $2, 'purged', $3, $4, $5)",
    )
    .bind(row.workspace_id)
    .bind(row.runtime_node_id)
    .bind(row.attempts)
    .bind(bytes.max(0))
    .bind(files.max(0))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.purged",
            actor_id: None,
            actor_username: "system",
            target_type: "ai_workspace",
            target_id: &row.workspace_id.to_string(),
            result: "success",
            ip: "",
            details: json!({
                "ownerId": row.owner_id,
                "runtimeNodeId": row.runtime_node_id,
                "name": row.original_name,
                "bytesFreed": bytes,
                "filesRemoved": files,
                "snapshotsRemoved": snapshots,
                "attempts": row.attempts,
            }),
        },
    )
    .await;
    state.realtime.publish_system(
        "ai.workspace.purged",
        json!({
            "workspaceId": row.workspace_id,
            "ownerId": row.owner_id,
            "bytesFreed": bytes,
            "filesRemoved": files,
        }),
    );
    Ok(())
}

async fn schedule_retry(state: &AppState, row: &ClaimedPurge, error: &str) -> ApiResult<()> {
    let message: String = error.chars().take(1000).collect();
    let permanent = message.contains("AI_WORKSPACE_OWNERSHIP_UNVERIFIED")
        || message.contains("OWNERSHIP_UNVERIFIED");
    if permanent {
        sqlx::query(
            "update ai_workspace_lifecycle
                set state = 'blocked', lease_until = null, last_error = $2, updated_at = now()
              where workspace_id = $1",
        )
        .bind(row.workspace_id)
        .bind(&message)
        .execute(&state.db)
        .await?;
    } else {
        let delay_minutes = match row.attempts {
            0 | 1 => 5,
            2 => 15,
            3 => 60,
            _ => 360,
        };
        let next = chrono::Utc::now() + chrono::Duration::minutes(delay_minutes);
        sqlx::query(
            "update ai_workspace_lifecycle
                set state = 'retry', lease_until = null, last_error = $2,
                    purge_after = $3, updated_at = now()
              where workspace_id = $1",
        )
        .bind(row.workspace_id)
        .bind(&message)
        .bind(next)
        .execute(&state.db)
        .await?;
    }
    sqlx::query(
        "insert into ai_workspace_purge_events
            (workspace_id, runtime_node_id, event, attempt, error)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(row.workspace_id)
    .bind(row.runtime_node_id)
    .bind(if permanent { "blocked" } else { "retry" })
    .bind(row.attempts)
    .bind(&message)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.purge_failed",
            actor_id: None,
            actor_username: "system",
            target_type: "ai_workspace",
            target_id: &row.workspace_id.to_string(),
            result: "error",
            ip: "",
            details: json!({
                "attempt": row.attempts,
                "permanent": permanent,
                "error": message,
            }),
        },
    )
    .await;
    Ok(())
}

/// Административный dry-run: что и когда будет удалено, без изменения данных.
pub async fn dry_run(state: &AppState, limit: i64) -> ApiResult<Value> {
    let rows = sqlx::query_as::<_, LifecycleRow>(&format!(
        "{SELECT_LIFECYCLE}
          where state in ('soft_deleted', 'pending', 'retry', 'blocked')
            and purge_after <= now()
          order by purge_after
          limit $1"
    ))
    .bind(limit.clamp(1, 200))
    .fetch_all(&state.db)
    .await?;
    Ok(json!({
        "dryRun": true,
        "retentionDays": state.ai_workspace_retention_days().await,
        "items": rows.iter().map(|row| row.view(true)).collect::<Vec<_>>(),
    }))
}

/// Сводка очереди для административной панели и метрик.
pub async fn metrics(state: &AppState) -> ApiResult<Value> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "select state, count(*) from ai_workspace_lifecycle group by state order by state",
    )
    .fetch_all(&state.db)
    .await?;
    let due: i64 = sqlx::query_scalar(
        "select count(*) from ai_workspace_lifecycle
          where state in ('soft_deleted', 'pending', 'retry')
            and purge_after <= now()",
    )
    .fetch_one(&state.db)
    .await?;
    let mut states = serde_json::Map::new();
    for (state, count) in rows {
        states.insert(state, json!(count));
    }
    Ok(json!({
        "states": states,
        "due": due,
        "retentionDays": state.ai_workspace_retention_days().await,
        "worker": "running",
    }))
}

// ---------------------------------------------------------------------------
// HTTP-обработчики
// ---------------------------------------------------------------------------

pub async fn list_deleted(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<crate::api::ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let items = list_deleted_for_owner(&state.db, actor.id).await?;
    Ok(crate::api::ApiOk(json!({
        "items": items,
        "retentionDays": state.ai_workspace_retention_days().await,
    })))
}

pub async fn restore(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    actor.require("ai.workspace.write")?;
    let workspace_id = path
        .map(|Path(value)| value)
        .map_err(|_| ApiError::validation("Некорректный идентификатор в пути."))?;
    let row = restore_for_owner(&state, &actor, workspace_id).await?;
    Ok(crate::api::ApiOk(row.view(actor.can("ai.admin.manage"))))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DryRunQuery {
    #[serde(default)]
    limit: Option<i64>,
}

pub async fn admin_dry_run(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<DryRunQuery>, QueryRejection>,
) -> ApiResult<crate::api::ApiOk<Value>> {
    require_admin(&actor)?;
    let limit = query
        .map(|Query(value)| value.limit)
        .ok()
        .flatten()
        .unwrap_or(50);
    Ok(crate::api::ApiOk(dry_run(&state, limit).await?))
}

pub async fn admin_metrics(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<crate::api::ApiOk<Value>> {
    require_admin(&actor)?;
    Ok(crate::api::ApiOk(metrics(&state).await?))
}

pub async fn admin_list(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<crate::api::ApiOk<Value>> {
    require_admin(&actor)?;
    let rows = sqlx::query_as::<_, LifecycleRow>(&format!(
        "{SELECT_LIFECYCLE} order by requested_at desc limit 500"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(crate::api::ApiOk(json!({
        "items": rows.iter().map(|row| row.view(true)).collect::<Vec<_>>(),
    })))
}

/// Администратор ставит очистку в очередь немедленно (всё равно через worker,
/// не в HTTP-обработчике).
pub async fn admin_purge_now(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<Json<Value>> {
    require_admin(&actor)?;
    let workspace_id = path
        .map(|Path(value)| value)
        .map_err(|_| ApiError::validation("Некорректный идентификатор в пути."))?;
    let workspace = workspaces::load_workspace(&state.db, workspace_id).await?;
    if workspace.deleted_at.is_none() {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_NOT_DELETED",
            "Workspace не удалён: сначала выполните мягкое удаление.",
        ));
    }
    let lifecycle = load_lifecycle(&state.db, workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Запись жизненного цикла не найдена."))?;
    if matches!(lifecycle.state.as_str(), "running" | "purged") {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_PURGE_STATE",
            "Очистка уже выполняется или завершена.",
        ));
    }
    sqlx::query(
        "update ai_workspace_lifecycle
            set state = 'pending', purge_after = now(), lease_until = null,
                last_error = null, updated_at = now()
          where workspace_id = $1",
    )
    .bind(workspace_id)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.purge_scheduled",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &workspace_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "immediate": true }),
        },
    )
    .await;
    Ok(Json(json!({
        "workspaceId": workspace_id,
        "queued": true,
        "message": "Очистка поставлена в очередь фонового worker.",
    })))
}

fn require_admin(actor: &AuthUser) -> ApiResult<()> {
    if actor.can("ai.admin.manage") {
        Ok(())
    } else {
        Err(ApiError::forbidden("ai.admin.manage"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restorable_states_are_only_unfinished_deletions() {
        let base = LifecycleRow {
            workspace_id: Uuid::nil(),
            owner_id: Uuid::nil(),
            runtime_node_id: Uuid::nil(),
            original_name: "w".into(),
            original_description: String::new(),
            state: "soft_deleted".into(),
            requested_by: None,
            requested_at: chrono::Utc::now(),
            purge_after: chrono::Utc::now(),
            attempts: 0,
            last_attempt_at: None,
            lease_until: None,
            last_error: None,
            purged_at: None,
            restored_at: None,
            bytes_freed: 0,
            files_removed: 0,
            updated_at: chrono::Utc::now(),
        };
        for state in ["soft_deleted", "pending", "retry"] {
            assert!(
                LifecycleRow {
                    state: state.into(),
                    ..base.clone()
                }
                .restorable()
            );
        }
        for state in ["running", "blocked", "purged", "restored"] {
            assert!(
                !LifecycleRow {
                    state: state.into(),
                    ..base.clone()
                }
                .restorable()
            );
        }
    }
}
