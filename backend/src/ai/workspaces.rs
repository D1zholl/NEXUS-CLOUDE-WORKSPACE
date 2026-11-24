//! Пользовательские workspace на Runtime: доступ, квоты и безопасные метаданные.

use std::path::Path;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ai::models::{QuotaRow, WorkspaceRow, load_quota};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const DEFAULT_QUOTA_BYTES: i64 = 1024 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: i32 = 10_000;
pub const DEFAULT_MAX_FILE_BYTES: i64 = 16 * 1024 * 1024;

pub async fn load_workspace(db: &PgPool, id: Uuid) -> ApiResult<WorkspaceRow> {
    sqlx::query_as::<_, WorkspaceRow>(
        "select id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                created_by, created_at, updated_at
           from ai_workspaces where id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Рабочее пространство AI не найдено."))
}

pub async fn list_for_owner(db: &PgPool, owner_id: Uuid) -> ApiResult<Vec<WorkspaceRow>> {
    Ok(sqlx::query_as::<_, WorkspaceRow>(
        "select id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                created_by, created_at, updated_at
           from ai_workspaces where owner_id = $1 order by lower(name)",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await?)
}

pub async fn list_all(db: &PgPool) -> ApiResult<Vec<WorkspaceRow>> {
    Ok(sqlx::query_as::<_, WorkspaceRow>(
        "select id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                created_by, created_at, updated_at
           from ai_workspaces order by created_at desc",
    )
    .fetch_all(db)
    .await?)
}

/// Доступ к workspace: владелец или администратор AI.
pub async fn accessible(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    require_active: bool,
) -> ApiResult<WorkspaceRow> {
    let workspace = load_workspace(db, workspace_id).await?;
    if workspace.owner_id != user_id && !is_admin {
        return Err(ApiError::not_found("Рабочее пространство AI не найдено."));
    }
    if require_active && workspace.status != "active" {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "AI_WORKSPACE_SUSPENDED",
            "Рабочее пространство приостановлено администратором.",
        ));
    }
    Ok(workspace)
}

pub fn validate_root_path(value: &str) -> ApiResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 1000 {
        return Err(ApiError::validation(
            "Укажите корректный корневой каталог workspace.",
        ));
    }
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err(ApiError::validation(
            "UNC-пути не поддерживаются для workspace AI.",
        ));
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(ApiError::validation(
            "Корневой каталог должен быть абсолютным путём на компьютере Runtime.",
        ));
    }
    if trimmed.split(['/', '\\']).any(|part| part == "..") {
        return Err(ApiError::validation(
            "Корневой каталог не должен содержать '..'.",
        ));
    }
    Ok(())
}

pub async fn ensure_quota(db: &PgPool, user_id: Uuid) -> ApiResult<QuotaRow> {
    sqlx::query(
        "insert into ai_user_quotas (user_id) values ($1) on conflict (user_id) do nothing",
    )
    .bind(user_id)
    .execute(db)
    .await?;
    load_quota(db, user_id)
        .await
        .ok_or_else(|| ApiError::internal("AI quota row not found"))
}

pub async fn active_task_count(db: &PgPool, user_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "select count(*) from ai_tasks
          where owner_id = $1 and status in ('queued', 'running', 'waiting_approval')",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

pub async fn tasks_today(db: &PgPool, user_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "select count(*) from ai_tasks
          where owner_id = $1 and created_at >= date_trunc('day', now())",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// Проверка квоты с блокировкой на пользователя, чтобы параллельные запросы
/// не превысили лимит. Вызывается внутри транзакции создания задачи.
pub async fn reserve_task_slot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> ApiResult<QuotaRow> {
    // Advisory xact lock сериализует создание задач одного пользователя.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("nexus.ai.tasks:{user_id}"))
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "insert into ai_user_quotas (user_id) values ($1) on conflict (user_id) do nothing",
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;

    let quota = sqlx::query_as::<_, QuotaRow>(
        "select user_id, max_concurrent_tasks, max_tasks_per_day, max_tool_calls_per_task,
                max_task_seconds, max_storage_bytes, max_files, max_file_bytes, updated_at
           from ai_user_quotas where user_id = $1 for update",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;

    let active: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks
          where owner_id = $1 and status in ('queued', 'running', 'waiting_approval')",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if active >= quota.max_concurrent_tasks as i64 {
        return Err(crate::ai::errors::task_limit_reached());
    }
    let today: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks
          where owner_id = $1 and created_at >= date_trunc('day', now())",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if today >= quota.max_tasks_per_day as i64 {
        return Err(crate::ai::errors::quota_exceeded(
            "Достигнут суточный лимит AI-задач.",
        ));
    }
    Ok(quota)
}

pub fn safe_view(workspace: &WorkspaceRow, admin: bool) -> Value {
    workspace.view(admin)
}

pub async fn usage_summary(db: &PgPool, workspace_id: Uuid) -> ApiResult<Value> {
    let workspace = load_workspace(db, workspace_id).await?;
    Ok(json!({
        "workspaceId": workspace.id,
        "quotaBytes": workspace.quota_bytes,
        "usedBytes": workspace.usage_bytes,
        "maxFiles": workspace.max_files,
        "usedFiles": workspace.usage_files,
        "maxFileBytes": workspace.max_file_bytes,
        "maxParallelTasks": workspace.max_parallel_tasks,
        "maxTaskSeconds": workspace.max_task_seconds,
        "networkPolicy": workspace.network_policy,
        "checkedAt": workspace.usage_checked_at,
        "source": if workspace.usage_checked_at.is_some() { "runtime" } else { "none" },
        "error": workspace.usage_error,
        "now": Utc::now(),
    }))
}

/// Сохраняет usage, измеренный Runtime, и историю замеров.
pub async fn record_usage(
    db: &PgPool,
    workspace_id: Uuid,
    node_id: Uuid,
    usage: &crate::ai::runtime::protocol::WorkspaceUsage,
) -> ApiResult<()> {
    let checked_at = usage.checked_at.unwrap_or_else(Utc::now);
    sqlx::query(
        "update ai_workspaces
            set usage_bytes = $2, usage_files = $3, usage_checked_at = $4,
                usage_reported_by = $5, usage_error = $6, updated_at = now()
          where id = $1",
    )
    .bind(workspace_id)
    .bind(usage.bytes.max(0))
    .bind(usage.files.max(0))
    .bind(checked_at)
    .bind(node_id)
    .bind(usage.error.as_deref())
    .execute(db)
    .await?;
    let _ = sqlx::query(
        "insert into ai_workspace_usage_samples
            (workspace_id, node_id, used_bytes, used_files, source, error, created_at)
         values ($1, $2, $3, $4, 'runtime', $5, now())",
    )
    .bind(workspace_id)
    .bind(node_id)
    .bind(usage.bytes.max(0))
    .bind(usage.files.max(0))
    .bind(usage.error.as_deref())
    .execute(db)
    .await;
    Ok(())
}

/// Запрашивает актуальный usage у Runtime по защищённому каналу.
pub async fn refresh_usage(
    state: &AppState,
    workspace_id: Uuid,
) -> ApiResult<crate::ai::runtime::protocol::WorkspaceUsage> {
    let workspace = load_workspace(&state.db, workspace_id).await?;
    if !state.ai.runtime.is_online(workspace.runtime_node_id) {
        return Err(crate::ai::errors::runtime_offline());
    }
    let message_id = Uuid::new_v4();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.ai.runtime.pending_register(
        message_id,
        workspace.runtime_node_id,
        crate::ai::PendingRequest::Reply(tx),
    );
    if let Err(error) = crate::ai::runtime::send_command(
        state,
        workspace.runtime_node_id,
        message_id,
        crate::ai::runtime::protocol::kind::WORKSPACE_USAGE,
        Some(message_id),
        None,
        json!({
            "workspaceId": workspace.id,
            "workspaceRoot": workspace.root_path,
        }),
        None,
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error);
    }
    let usage = match tokio::time::timeout(std::time::Duration::from_secs(25), rx).await {
        Ok(Ok(crate::ai::runtime::RuntimeReply::Usage(usage))) => usage,
        _ => {
            let _ = state.ai.runtime.pending_remove(message_id);
            return Err(ApiError::unavailable(
                "AI_USAGE_TIMEOUT",
                "Runtime не вернул usage рабочего пространства.",
                5,
            ));
        }
    };
    record_usage(&state.db, workspace.id, workspace.runtime_node_id, &usage).await?;
    Ok(usage)
}
