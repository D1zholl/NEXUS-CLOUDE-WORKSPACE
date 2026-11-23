//! Одноразовые подтверждения опасных действий.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ai::models::redact_arguments;
use crate::ai::{errors, orchestrator};
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub const APPROVAL_TTL_SECONDS: i64 = 15 * 60;

#[derive(Clone, Debug, sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRow {
    pub id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub requester_id: Uuid,
    pub approver_id: Option<Uuid>,
    pub tool_id: String,
    pub args_hash: String,
    pub arguments: Value,
    pub decision: String,
    pub reason: String,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn create(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: Uuid,
    execution_id: Uuid,
    requester_id: Uuid,
    tool_id: &str,
    args_hash: &str,
    arguments: &Value,
) -> ApiResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "insert into ai_tool_approvals
            (task_id, execution_id, requester_id, tool_id, args_hash, arguments, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id",
    )
    .bind(task_id)
    .bind(execution_id)
    .bind(requester_id)
    .bind(tool_id)
    .bind(args_hash)
    .bind(arguments)
    .bind(Utc::now() + Duration::seconds(APPROVAL_TTL_SECONDS))
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn expire_stale(db: &PgPool) -> u64 {
    sqlx::query(
        "update ai_tool_approvals
            set decision = 'expired', decided_at = now()
          where decision = 'pending' and expires_at <= now()",
    )
    .execute(db)
    .await
    .map(|result| result.rows_affected())
    .unwrap_or(0)
}

pub async fn pending_for_user(db: &PgPool, user_id: Uuid) -> ApiResult<Vec<Value>> {
    expire_stale(db).await;
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        task_id: Uuid,
        execution_id: Uuid,
        tool_id: String,
        arguments: Value,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
        conversation_id: Uuid,
        workspace_id: Option<Uuid>,
        status_text: String,
        risk: String,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select a.id, a.task_id, a.execution_id, a.tool_id, a.arguments, a.expires_at,
                a.created_at, t.conversation_id, t.workspace_id, t.status_text,
                coalesce(e.risk, 'high') as risk
           from ai_tool_approvals a
           join ai_tasks t on t.id = a.task_id
           left join ai_tool_executions e on e.id = a.execution_id
          where a.requester_id = $1 and a.decision = 'pending' and a.expires_at > now()
          order by a.created_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "taskId": row.task_id,
                "executionId": row.execution_id,
                "conversationId": row.conversation_id,
                "workspaceId": row.workspace_id,
                "toolId": row.tool_id,
                "risk": row.risk,
                "arguments": redact_arguments(&row.arguments),
                "statusText": row.status_text,
                "expiresAt": row.expires_at,
                "createdAt": row.created_at,
            })
        })
        .collect())
}

pub async fn load(db: &PgPool, id: Uuid) -> ApiResult<ApprovalRow> {
    sqlx::query_as::<_, ApprovalRow>(
        "select id, task_id, execution_id, requester_id, approver_id, tool_id, args_hash,
                arguments, decision, reason, expires_at, decided_at, consumed_at, created_at
           from ai_tool_approvals where id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Подтверждение не найдено."))
}

/// Решение пользователя. Аргументы не меняются: approval привязан к args_hash.
pub async fn decide(
    state: &Arc<AppState>,
    actor: &AuthUser,
    approval_id: Uuid,
    approve: bool,
    reason: &str,
) -> ApiResult<Value> {
    expire_stale(&state.db).await;
    let approval = load(&state.db, approval_id).await?;
    let is_admin = actor.can("ai.admin.manage");
    if approval.requester_id != actor.id && !is_admin {
        // Не раскрываем существование чужого approval.
        return Err(ApiError::not_found("Подтверждение не найдено."));
    }
    if approval.decision != "pending" {
        return Err(errors::approval_expired());
    }
    if approval.expires_at <= Utc::now() {
        return Err(errors::approval_expired());
    }
    let (decision, message) = if approve {
        ("approved", "Действие подтверждено")
    } else {
        ("rejected", "Действие отклонено")
    };
    let updated = sqlx::query(
        "update ai_tool_approvals
            set decision = $2, approver_id = $3, decided_at = now(), reason = $4
          where id = $1 and decision = 'pending' and expires_at > now()",
    )
    .bind(approval_id)
    .bind(decision)
    .bind(actor.id)
    .bind(reason.chars().take(500).collect::<String>())
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(errors::approval_expired());
    }

    record(
        &state.db,
        AuditEvent {
            event_type: if approve {
                "ai.approval.approved"
            } else {
                "ai.approval.rejected"
            },
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_tool_approval",
            target_id: &approval_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "taskId": approval.task_id,
                "toolId": approval.tool_id,
                "argsHash": approval.args_hash,
            }),
        },
    )
    .await;

    if approve {
        let state = state.clone();
        tokio::spawn(async move {
            orchestrator::resume_after_approval(state, approval.task_id, approval_id).await;
        });
    } else {
        // Отклонение останавливает задачу: модель не должна обходить решение.
        sqlx::query(
            "update ai_tasks
                set status = 'cancelled', stage = 'rejected', status_text = 'Действие отклонено',
                    finished_at = now(), updated_at = now()
              where id = $1 and status = 'waiting_approval'",
        )
        .bind(approval.task_id)
        .execute(&state.db)
        .await?;
        sqlx::query(
            "update ai_tool_executions
                set status = 'cancelled', error_code = 'AI_APPROVAL_REJECTED',
                    error_message = 'Пользователь отклонил действие.', updated_at = now()
              where id = $1",
        )
        .bind(approval.execution_id)
        .execute(&state.db)
        .await?;
        state.realtime.publish_user(
            "ai.task.cancelled",
            approval.requester_id,
            json!({ "taskId": approval.task_id, "message": "Действие отклонено" }),
        );
    }

    Ok(json!({
        "id": approval_id,
        "taskId": approval.task_id,
        "decision": decision,
        "message": message,
    }))
}

/// Атомарно потребляет approval ровно для тех аргументов, которые были
/// показаны пользователю. Повторное потребление невозможно.
pub async fn consume(db: &PgPool, approval_id: Uuid, expected_hash: &str) -> ApiResult<bool> {
    let result = sqlx::query(
        "update ai_tool_approvals
            set consumed_at = now()
          where id = $1 and decision = 'approved' and consumed_at is null
            and args_hash = $2 and expires_at > now()",
    )
    .bind(approval_id)
    .bind(expected_hash)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(errors::approval_expired());
    }
    Ok(true)
}
