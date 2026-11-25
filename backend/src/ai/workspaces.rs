//! Пользовательские workspace на Runtime: доступ, квоты и безопасные метаданные.

use std::path::Path;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ai::models::{QuotaRow, WorkspaceRow, load_quota};
use crate::ai::runtime::RuntimeReply;
use crate::ai::{PendingRequest, tools};
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub const DEFAULT_QUOTA_BYTES: i64 = 1024 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: i32 = 10_000;
pub const DEFAULT_MAX_FILE_BYTES: i64 = 16 * 1024 * 1024;

pub async fn load_workspace(db: &PgPool, id: Uuid) -> ApiResult<WorkspaceRow> {
    sqlx::query_as::<_, WorkspaceRow>(
        "select id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                description, deleted_at, created_by, created_at, updated_at
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
                description, deleted_at, created_by, created_at, updated_at
           from ai_workspaces
          where owner_id = $1 and deleted_at is null
          order by lower(name)",
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
                description, deleted_at, created_by, created_at, updated_at
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
    if workspace.deleted_at.is_some() && !is_admin {
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
    workspace_id: Uuid,
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
                max_task_seconds, max_iterations, max_storage_bytes, max_files, max_file_bytes,
                updated_at
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
    // Лимит параллельных задач конкретного workspace (не только пользователя).
    let workspace_limit: i32 =
        sqlx::query_scalar("select max_parallel_tasks from ai_workspaces where id = $1")
            .bind(workspace_id)
            .fetch_one(&mut **tx)
            .await?;
    let active_in_workspace: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks
          where workspace_id = $1 and status in ('queued', 'running', 'waiting_approval')",
    )
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;
    if workspace_limit > 0 && active_in_workspace >= workspace_limit as i64 {
        return Err(crate::ai::errors::workspace_task_limit_reached());
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

#[derive(Clone, Debug)]
pub struct WorkspaceCreate {
    pub name: String,
    pub description: String,
    pub runtime_node_id: Option<Uuid>,
}

fn validate_name(name: &str) -> ApiResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 80 {
        return Err(ApiError::validation(
            "Название workspace должно содержать от 1 до 80 символов.",
        ));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(ApiError::validation(
            "Название workspace содержит недопустимые символы.",
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_description(description: &str) -> ApiResult<String> {
    let trimmed = description.trim();
    if trimmed.chars().count() > 2000 {
        return Err(ApiError::validation(
            "Описание workspace не должно превышать 2000 символов.",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Просит Runtime создать каталог нового workspace. Путь возвращается только
/// backend и никогда не попадает в пользовательский ответ.
pub async fn provision_workspace_dir(
    state: &AppState,
    node_id: Uuid,
    workspace_id: Uuid,
) -> ApiResult<String> {
    if !state.ai.runtime.is_online(node_id) {
        return Err(crate::ai::errors::runtime_offline());
    }
    let message_id = Uuid::new_v4();
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .ai
        .runtime
        .pending_register(message_id, node_id, PendingRequest::Reply(tx));
    if let Err(error) = crate::ai::runtime::send_command(
        state,
        node_id,
        message_id,
        crate::ai::runtime::protocol::kind::WORKSPACE_PROVISION,
        Some(message_id),
        None,
        json!({ "workspaceId": workspace_id }),
        None,
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error);
    }
    let result = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
        Ok(Ok(RuntimeReply::Provision(result))) => result,
        Ok(Ok(RuntimeReply::Failed(failure))) => {
            return Err(ApiError::unavailable(failure.code, failure.message, 5));
        }
        _ => {
            let _ = state.ai.runtime.pending_remove(message_id);
            return Err(ApiError::unavailable(
                "AI_PROVISION_TIMEOUT",
                "Runtime не подтвердил создание workspace.",
                5,
            ));
        }
    };
    if !result.ok {
        return Err(ApiError::unavailable(
            "AI_PROVISION_FAILED",
            result
                .error
                .unwrap_or_else(|| "Runtime не смог создать каталог workspace.".to_owned()),
            5,
        ));
    }
    result
        .path
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            ApiError::unavailable("AI_PROVISION_FAILED", "Runtime вернул пустой путь.", 5)
        })
}

/// Создаёт workspace владельца. Runtime выделяет изолированный каталог,
/// backend сохраняет путь, пользователь его не видит.
pub async fn create_for_owner(
    state: &AppState,
    actor: &AuthUser,
    request: WorkspaceCreate,
) -> ApiResult<WorkspaceRow> {
    let name = validate_name(&request.name)?;
    let description = validate_description(&request.description)?;
    let duplicate: bool = sqlx::query_scalar(
        "select exists(
            select 1 from ai_workspaces
             where owner_id = $1 and lower(name) = lower($2) and deleted_at is null
        )",
    )
    .bind(actor.id)
    .bind(&name)
    .fetch_one(&state.db)
    .await?;
    if duplicate {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_EXISTS",
            "Рабочее пространство с таким названием уже существует.",
        ));
    }
    let quota = ensure_quota(&state.db, actor.id).await?;

    let node_id = match request.runtime_node_id {
        Some(node_id) => {
            if !crate::ai::runtime::node_is_usable(&state.db, node_id).await? {
                return Err(ApiError::validation(
                    "Выбранный Runtime недоступен или отключён.",
                ));
            }
            // Пользователь не может создать workspace на произвольном
            // устройстве: только там, где у него уже есть рабочее пространство.
            let owns: bool = sqlx::query_scalar(
                "select exists(
                    select 1 from ai_workspaces
                     where owner_id = $1 and runtime_node_id = $2 and deleted_at is null
                )",
            )
            .bind(actor.id)
            .bind(node_id)
            .fetch_one(&state.db)
            .await?;
            if !owns {
                return Err(ApiError::forbidden("ai.workspace.write"));
            }
            node_id
        }
        None => {
            let candidates: Vec<Uuid> = sqlx::query_scalar(
                "select id from ai_runtime_nodes
                  where status = 'online' and revoked_at is null
                  order by last_seen_at desc nulls last",
            )
            .fetch_all(&state.db)
            .await?;
            match candidates
                .into_iter()
                .find(|id| state.ai.runtime.is_online(*id))
            {
                Some(id) => id,
                None => {
                    return Err(ApiError::unavailable(
                        "AI_RUNTIME_OFFLINE",
                        "Нет подключённого Runtime для создания workspace.",
                        10,
                    ));
                }
            }
        }
    };

    let workspace_id = Uuid::new_v4();
    let root_path = provision_workspace_dir(state, node_id, workspace_id).await?;
    let allowed_tools = tools::default_workspace_tools();
    // Квоты workspace наследуют пользовательские; 0 означает «без лимита»
    // (Runtime понимает 0 как отсутствие ограничения).
    let quota_bytes = quota.max_storage_bytes.max(0);
    let max_files = quota.max_files.max(0);
    let max_file_bytes = quota.max_file_bytes.max(0);
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "insert into ai_workspaces
            (id, owner_id, runtime_node_id, name, description, root_path, quota_bytes,
             max_files, max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
             allowed_programs, status, created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, $10, $11, '{}', 'active', $2)
         returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                   max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                   allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                   network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                   description, deleted_at, created_by, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(actor.id)
    .bind(node_id)
    .bind(&name)
    .bind(&description)
    .bind(&root_path)
    .bind(quota_bytes.max(0))
    .bind(max_files.max(0))
    .bind(max_file_bytes.max(0))
    .bind(quota.max_task_seconds.clamp(30, 3600))
    .bind(&allowed_tools)
    .fetch_one(&state.db)
    .await
    .map_err(|error| {
        if let sqlx::Error::Database(db) = &error {
            if db.code().as_deref() == Some("23505") {
                return ApiError::conflict(
                    "AI_WORKSPACE_EXISTS",
                    "Рабочее пространство с таким названием уже существует.",
                );
            }
        }
        ApiError::from(error)
    })?;

    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &row.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "name": row.name,
                "runtimeNodeId": row.runtime_node_id,
                "tools": row.allowed_tools.len(),
            }),
        },
    )
    .await;
    Ok(row)
}

pub async fn rename_for_owner(
    state: &AppState,
    actor: &AuthUser,
    workspace_id: Uuid,
    name: Option<String>,
    description: Option<String>,
) -> ApiResult<WorkspaceRow> {
    let current = accessible(&state.db, workspace_id, actor.id, false, true).await?;
    let name = match name {
        Some(value) => validate_name(&value)?,
        None => current.name.clone(),
    };
    let description = match description {
        Some(value) => validate_description(&value)?,
        None => current.description.clone(),
    };
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "update ai_workspaces
            set name = $2, description = $3, updated_at = now()
          where id = $1 and deleted_at is null
          returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                    max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                    allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                    network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                    description, deleted_at, created_by, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&name)
    .bind(&description)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| {
        if let sqlx::Error::Database(db) = &error {
            if db.code().as_deref() == Some("23505") {
                return ApiError::conflict(
                    "AI_WORKSPACE_EXISTS",
                    "Рабочее пространство с таким названием уже существует.",
                );
            }
        }
        ApiError::from(error)
    })?
    .ok_or_else(|| ApiError::not_found("Рабочее пространство AI не найдено."))?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.renamed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &workspace_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": row.name }),
        },
    )
    .await;
    Ok(row)
}

/// Мягкое удаление: workspace исчезает из пользовательских списков, файлы
/// сохраняются до истечения срока хранения. Физическую очистку выполняет
/// `workspace_lifecycle` через Runtime, а не HTTP-обработчик.
pub async fn delete_for_owner(
    state: &AppState,
    actor: &AuthUser,
    workspace_id: Uuid,
) -> ApiResult<WorkspaceRow> {
    let current = accessible(&state.db, workspace_id, actor.id, false, false).await?;
    let retention_days = state.ai_workspace_retention_days().await;
    let mut tx = state.db.begin().await?;
    // Блокируем строку: restore не должен обогнать удаление.
    let locked: Option<Uuid> = sqlx::query_scalar(
        "select id from ai_workspaces where id = $1 and deleted_at is null for update",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    if locked.is_none() {
        return Err(ApiError::not_found("Рабочее пространство AI не найдено."));
    }
    let active: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks
          where workspace_id = $1 and status in ('queued', 'running', 'waiting_approval')",
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await?;
    if active > 0 {
        return Err(ApiError::conflict(
            "AI_WORKSPACE_BUSY",
            "В workspace есть активные задачи. Дождитесь завершения или отмените их.",
        ));
    }
    // Незавершённые резервы квоты больше не должны удерживать место.
    crate::ai::quota::release_workspace(&mut tx, workspace_id).await?;
    // Ожидающие approvals без активной задачи не должны пережить удаление.
    let rejected = sqlx::query(
        "update ai_tool_approvals a
            set decision = 'rejected', reason = 'workspace deleted',
                decided_at = now()
           from ai_tasks t
          where a.task_id = t.id and t.workspace_id = $1 and a.decision = 'pending'",
    )
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "update ai_workspaces
            set deleted_at = now(), updated_at = now(),
                name = left($2, 40) || ' (удалён ' || $1::text || ')'
          where id = $1 and deleted_at is null
          returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                    max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                    allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                    network_policy, max_process_memory_bytes, usage_reported_by, usage_error,
                    description, deleted_at, created_by, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&current.name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Рабочее пространство AI не найдено."))?;
    // `current` — состояние до переименования: именно его имя пользователь
    // должен увидеть при восстановлении.
    crate::ai::workspace_lifecycle::insert_soft_delete(&mut tx, &current, actor.id, retention_days)
        .await?;
    tx.commit().await?;
    let lifecycle = crate::ai::workspace_lifecycle::load_lifecycle(&state.db, workspace_id)
        .await?
        .ok_or_else(|| ApiError::internal("Запись жизненного цикла не создана."))?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.deleted",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &workspace_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "softDelete": true,
                "filesRetained": true,
                "purgeAfter": lifecycle.purge_after,
                "retentionDays": retention_days,
                "rejectedApprovals": rejected,
            }),
        },
    )
    .await;
    state.realtime.publish_system(
        "ai.workspace.deleted",
        json!({
            "workspaceId": workspace_id,
            "ownerId": row.owner_id,
            "purgeAfter": lifecycle.purge_after,
        }),
    );
    Ok(row)
}

/// Сводка для верхней панели и панели Workspace Usage.
pub async fn overview(db: &PgPool, workspace_id: Uuid) -> ApiResult<Value> {
    let workspace = load_workspace(db, workspace_id).await?;
    let (tasks_total, tasks_active): (i64, i64) = sqlx::query_as(
        "select count(*),
                count(*) filter (where status in ('queued','running','waiting_approval'))
           from ai_tasks where workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(db)
    .await?;
    let last_task: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
            "select max(updated_at) from ai_tasks where workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(db)
        .await?;
    let snapshots: i64 =
        sqlx::query_scalar("select count(*) from ai_file_snapshots where workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(db)
            .await?;
    let last_activity = match (last_task, workspace.usage_checked_at) {
        (Some(task), Some(usage)) => Some(task.max(usage)),
        (Some(task), None) => Some(task),
        (None, Some(usage)) => Some(usage),
        (None, None) => None,
    };
    Ok(json!({
        "workspaceId": workspace.id,
        "name": workspace.name,
        "description": workspace.description,
        "status": workspace.status,
        "runtimeNodeId": workspace.runtime_node_id,
        "createdAt": workspace.created_at,
        "updatedAt": workspace.updated_at,
        "lastActivityAt": last_activity,
        "tasksTotal": tasks_total,
        "tasksActive": tasks_active,
        "files": workspace.usage_files,
        "bytes": workspace.usage_bytes,
        "quotaBytes": workspace.quota_bytes,
        "maxFiles": workspace.max_files,
        "maxFileBytes": workspace.max_file_bytes,
        "maxParallelTasks": workspace.max_parallel_tasks,
        "allowedTools": workspace.allowed_tools,
        "allowedPrograms": workspace.allowed_programs,
        "snapshots": snapshots,
        "usageCheckedAt": workspace.usage_checked_at,
        "usageError": workspace.usage_error,
    }))
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
    // Не перезаписываем более свежий замер устаревшим: параллельные операции
    // могут подтвердиться в БД не в том порядке, в котором Runtime их выполнил.
    sqlx::query(
        "update ai_workspaces
            set usage_bytes = case when usage_checked_at is null or $4 >= usage_checked_at
                                  then $2 else usage_bytes end,
                usage_files = case when usage_checked_at is null or $4 >= usage_checked_at
                                  then $3 else usage_files end,
                usage_error = case when usage_checked_at is null or $4 >= usage_checked_at
                                  then $6 else usage_error end,
                usage_checked_at = case when usage_checked_at is null or $4 >= usage_checked_at
                                  then $4 else usage_checked_at end,
                usage_reported_by = case when usage_checked_at is null or $4 >= usage_checked_at
                                  then $5 else usage_reported_by end,
                updated_at = now()
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
