//! Административные представления: overview, журналы, узлы Runtime.

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ai::runtime;
use crate::api::{Page, normalize_pagination};
use crate::core::error::ApiResult;
use crate::state::AppState;

pub async fn overview(state: &AppState) -> ApiResult<Value> {
    let module_enabled = state.module_enabled(crate::ai::MODULE_KEY).await;
    let runtimes_total: i64 = sqlx::query_scalar("select count(*) from ai_runtime_nodes")
        .fetch_one(&state.db)
        .await?;
    let runtimes_online: i64 = sqlx::query_scalar(
        "select count(*) from ai_runtime_nodes where status in ('online', 'degraded')",
    )
    .fetch_one(&state.db)
    .await?;
    let runtimes_disabled: i64 = sqlx::query_scalar(
        "select count(*) from ai_runtime_nodes where status in ('disabled', 'revoked')",
    )
    .fetch_one(&state.db)
    .await?;
    let providers_total: i64 = sqlx::query_scalar("select count(*) from ai_provider_configs")
        .fetch_one(&state.db)
        .await?;
    let providers_enabled: i64 =
        sqlx::query_scalar("select count(*) from ai_provider_configs where enabled = true")
            .fetch_one(&state.db)
            .await?;
    let providers_error: i64 =
        sqlx::query_scalar("select count(*) from ai_provider_configs where status = 'error'")
            .fetch_one(&state.db)
            .await?;
    let tasks_running: i64 =
        sqlx::query_scalar("select count(*) from ai_tasks where status = 'running'")
            .fetch_one(&state.db)
            .await?;
    let tasks_queued: i64 =
        sqlx::query_scalar("select count(*) from ai_tasks where status = 'queued'")
            .fetch_one(&state.db)
            .await?;
    let tasks_waiting: i64 =
        sqlx::query_scalar("select count(*) from ai_tasks where status = 'waiting_approval'")
            .fetch_one(&state.db)
            .await?;
    let tasks_failed_day: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks where status = 'failed' and created_at >= now() - interval '24 hours'",
    )
    .fetch_one(&state.db)
    .await?;
    let approvals_pending: i64 = sqlx::query_scalar(
        "select count(*) from ai_tool_approvals where decision = 'pending' and expires_at > now()",
    )
    .fetch_one(&state.db)
    .await?;
    let active_chats = state.ai.chats.len() as i64;
    let stream_events = state
        .ai
        .stream_events
        .load(std::sync::atomic::Ordering::Relaxed) as i64;
    let last_heartbeat: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select max(last_heartbeat_at) from ai_runtime_nodes")
            .fetch_one(&state.db)
            .await?;
    let queue_age: Option<f64> = sqlx::query_scalar(
        "select extract(epoch from (now() - min(created_at))) from ai_tasks where status = 'queued'",
    )
    .fetch_one(&state.db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct NodeRow {
        id: Uuid,
        name: String,
        status: String,
        last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
        runtime_version: String,
        capabilities: Value,
    }
    let nodes = sqlx::query_as::<_, NodeRow>(
        "select id, name, status, last_seen_at, runtime_version, capabilities
           from ai_runtime_nodes order by name limit 20",
    )
    .fetch_all(&state.db)
    .await?;
    let nodes_json: Vec<Value> = nodes
        .into_iter()
        .map(|node| {
            let connected = state.ai.runtime.is_online(node.id);
            let polystrata = node
                .capabilities
                .get("polystrata")
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "id": node.id,
                "name": node.name,
                "status": runtime::effective_status(&node.status, node.last_seen_at),
                "connected": connected,
                "runtimeVersion": node.runtime_version,
                "lastSeenAt": node.last_seen_at,
                "polystrata": polystrata,
            })
        })
        .collect();

    let recent_errors = sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
        "select kind, message, created_at from ai_runtime_events
          where kind in ('error', 'disconnected') order by created_at desc limit 5",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(kind, message, at)| json!({ "kind": kind, "message": message, "at": at }))
    .collect::<Vec<_>>();

    Ok(json!({
        "moduleEnabled": module_enabled,
        "runtimes": {
            "total": runtimes_total,
            "online": runtimes_online,
            "disabledOrRevoked": runtimes_disabled,
            "connectedNow": state.ai.runtime.online_count(),
        },
        "providers": {
            "total": providers_total,
            "enabled": providers_enabled,
            "error": providers_error,
        },
        "tasks": {
            "running": tasks_running,
            "queued": tasks_queued,
            "waitingApproval": tasks_waiting,
            "failedLast24h": tasks_failed_day,
            "queueAgeSeconds": queue_age,
        },
        "approvals": { "pending": approvals_pending },
        "streaming": { "activeChats": active_chats, "events": stream_events },
        "lastHeartbeatAt": last_heartbeat,
        "nodes": nodes_json,
        "recentErrors": recent_errors,
        "checkedAt": chrono::Utc::now(),
    }))
}

pub async fn runtime_nodes(db: &PgPool, state: &AppState) -> ApiResult<Vec<Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        name: String,
        platform: String,
        runtime_version: String,
        protocol_version: i32,
        status: String,
        capabilities: Value,
        last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
        last_heartbeat_at: Option<chrono::DateTime<chrono::Utc>>,
        registered_at: chrono::DateTime<chrono::Utc>,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
        token_rotated_at: Option<chrono::DateTime<chrono::Utc>>,
        active_tasks: i64,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select n.id, n.name, n.platform, n.runtime_version, n.protocol_version, n.status,
                n.capabilities, n.last_seen_at, n.last_heartbeat_at, n.registered_at,
                n.revoked_at, n.token_rotated_at,
                (select count(*) from ai_tasks t
                  where t.runtime_node_id = n.id
                    and t.status in ('queued', 'running', 'waiting_approval')) as active_tasks
           from ai_runtime_nodes n order by n.registered_at desc",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "name": row.name,
                "platform": row.platform,
                "runtimeVersion": row.runtime_version,
                "protocolVersion": row.protocol_version,
                "status": runtime::effective_status(&row.status, row.last_seen_at),
                "connected": state.ai.runtime.is_online(row.id),
                "lastSeenAt": row.last_seen_at,
                "lastHeartbeatAt": row.last_heartbeat_at,
                "registeredAt": row.registered_at,
                "revokedAt": row.revoked_at,
                "tokenRotatedAt": row.token_rotated_at,
                "activeTasks": row.active_tasks,
                "capabilities": runtime::capabilities_summary(&row.capabilities),
            })
        })
        .collect())
}

pub async fn runtime_node_detail(db: &PgPool, state: &AppState, id: Uuid) -> ApiResult<Value> {
    let node = runtime::load_node(db, id).await?;
    let list = runtime_nodes(db, state).await?;
    let summary = list
        .into_iter()
        .find(|item| item["id"] == json!(id))
        .unwrap_or_else(|| json!({ "id": id, "name": node.name }));
    #[derive(sqlx::FromRow)]
    struct ProgramRow {
        program_id: String,
        name: String,
        executable: String,
        version: String,
        available: bool,
        risk: String,
        enabled: bool,
    }
    let programs = sqlx::query_as::<_, ProgramRow>(
        "select program_id, name, executable, version, available, risk, enabled
           from ai_programs where runtime_node_id = $1 order by program_id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let pending_commands: i64 = sqlx::query_scalar(
        "select count(*) from ai_runtime_commands where node_id = $1 and status in ('pending','sent')",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    let events = sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
        "select kind, message, created_at from ai_runtime_events
          where node_id = $1 order by created_at desc limit 20",
    )
    .bind(id)
    .fetch_all(db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(kind, message, at)| json!({ "kind": kind, "message": message, "at": at }))
    .collect::<Vec<_>>();
    Ok(json!({
        "node": summary,
        "programs": programs.into_iter().map(|row| json!({
            "programId": row.program_id,
            "name": row.name,
            "executable": row.executable,
            "version": row.version,
            "available": row.available,
            "risk": row.risk,
            "enabled": row.enabled,
        })).collect::<Vec<_>>(),
        "pendingCommands": pending_commands,
        "events": events,
    }))
}

pub async fn logs(
    db: &PgPool,
    page: Option<i64>,
    per_page: Option<i64>,
    search: Option<&str>,
) -> ApiResult<Value> {
    let (page, per_page, offset) = normalize_pagination(page, per_page);
    let pattern = search
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"));
    #[derive(sqlx::FromRow, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AuditRow {
        id: Uuid,
        created_at: chrono::DateTime<chrono::Utc>,
        event_type: String,
        actor_username: String,
        target_type: String,
        target_id: String,
        result: String,
        details: Value,
    }
    let total: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
          where event_type like 'ai.%'
            and ($1::text is null or event_type ilike $1 or target_id ilike $1)",
    )
    .bind(&pattern)
    .fetch_one(db)
    .await?;
    let audit = sqlx::query_as::<_, AuditRow>(
        "select id, created_at, event_type, actor_username, target_type, target_id,
                result, details
           from audit_events
          where event_type like 'ai.%'
            and ($1::text is null or event_type ilike $1 or target_id ilike $1)
          order by created_at desc limit $2 offset $3",
    )
    .bind(&pattern)
    .bind(per_page)
    .bind(offset)
    .fetch_all(db)
    .await?;

    #[derive(sqlx::FromRow, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RuntimeRow {
        id: i64,
        node_id: Option<Uuid>,
        kind: String,
        message: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }
    let runtime_total: i64 = sqlx::query_scalar(
        "select count(*) from ai_runtime_events
          where ($1::text is null or kind ilike $1 or message ilike $1)",
    )
    .bind(&pattern)
    .fetch_one(db)
    .await?;
    let runtime_rows = sqlx::query_as::<_, RuntimeRow>(
        "select id, node_id, kind, message, created_at from ai_runtime_events
          where ($1::text is null or kind ilike $1 or message ilike $1)
          order by created_at desc limit $2 offset $3",
    )
    .bind(&pattern)
    .bind(per_page)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(json!({
        "audit": Page::new(audit, page, per_page, total),
        "runtime": Page::new(runtime_rows, page, per_page, runtime_total),
    }))
}
