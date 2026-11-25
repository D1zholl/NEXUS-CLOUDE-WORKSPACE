//! HTTP-маршруты AI Workspace (пользовательская часть + admin nest).

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, patch, post};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ai::runtime;
use crate::ai::{
    approvals, conversations, orchestrator, providers, skills, tools, workspace_files,
    workspace_lifecycle, workspaces,
};
use crate::api::{ApiOk, Page, normalize_pagination};
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::core::idempotency;
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runtime/enroll", post(enroll))
        .route("/runtime/connect", get(runtime::ws_handler))
        .route("/providers", get(list_providers))
        .route("/providers/{id}", get(get_provider))
        .route(
            "/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/conversations/{id}",
            get(get_conversation)
                .patch(update_conversation)
                .delete(delete_conversation),
        )
        .route(
            "/conversations/{id}/messages",
            get(list_messages).post(send_message),
        )
        .route(
            "/conversations/{id}/messages/{message_id}/regenerate",
            post(regenerate_message),
        )
        .route(
            "/conversations/{id}/messages/{message_id}",
            patch(edit_message),
        )
        .route("/conversations/{id}/stop", post(stop_generation))
        .route("/conversations/{id}/attachments", post(upload_attachment))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/{id}", get(get_task))
        .route("/tasks/{id}/events", get(task_events))
        .route("/tasks/{id}/cancel", post(cancel_task))
        .route("/tasks/{id}/executions", get(task_executions))
        .route("/approvals/pending", get(pending_approvals))
        .route("/approvals/{id}/approve", post(approve))
        .route("/approvals/{id}/reject", post(reject))
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route(
            "/workspaces/deleted",
            get(workspace_lifecycle::list_deleted),
        )
        .route(
            "/workspaces/{id}/restore",
            post(workspace_lifecycle::restore),
        )
        .route(
            "/workspaces/{id}",
            get(get_workspace)
                .patch(update_workspace)
                .delete(delete_workspace),
        )
        .route("/workspaces/{id}/overview", get(workspace_overview))
        .route(
            "/workspaces/{id}/files",
            get(workspace_files::tree).delete(workspace_files::delete_entry),
        )
        .route(
            "/workspaces/{id}/files/content",
            get(workspace_files::content).put(workspace_files::save_content),
        )
        .route("/workspaces/{id}/files/raw", get(workspace_files::raw_file))
        .route(
            "/workspaces/{id}/files/stat",
            get(workspace_files::stat_entry),
        )
        .route(
            "/workspaces/{id}/files/directory",
            post(workspace_files::create_directory),
        )
        .route(
            "/workspaces/{id}/files/move",
            post(workspace_files::move_entry),
        )
        .route(
            "/workspaces/{id}/files/copy",
            post(workspace_files::copy_entry),
        )
        .route(
            "/workspaces/{id}/files/search",
            post(workspace_files::search_files),
        )
        .route(
            "/workspaces/{id}/files/upload",
            post(workspace_files::upload),
        )
        .route(
            "/workspaces/{id}/changes",
            get(workspace_files::list_changes),
        )
        .route(
            "/workspaces/{id}/snapshots/{snapshot_id}",
            get(workspace_files::snapshot_detail),
        )
        .route(
            "/workspaces/{id}/snapshots/{snapshot_id}/restore",
            post(workspace_files::restore_snapshot),
        )
        .route(
            "/workspaces/{id}/tasks/{task_id}/restore",
            post(workspace_files::restore_task),
        )
        .route("/workspaces/{id}/usage", get(workspace_usage))
        .route(
            "/workspaces/{id}/usage/refresh",
            post(refresh_workspace_usage),
        )
        .route("/skills", get(list_skills))
        .route("/skills/{id}", get(get_skill))
        .route("/tools", get(list_tools))
        .nest("/admin", crate::ai::admin_routes::router())
}

fn parse_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректное тело запроса: {error}")))
}

fn parse_path<T>(path: Result<Path<T>, PathRejection>) -> ApiResult<T> {
    path.map(|Path(value)| value)
        .map_err(|_| ApiError::validation("Некорректный идентификатор в пути."))
}

fn parse_query<T>(query: Result<Query<T>, QueryRejection>) -> ApiResult<T> {
    query
        .map(|Query(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректные параметры запроса: {error}")))
}

async fn enroll(
    State(state): State<Arc<AppState>>,
    body: Result<Json<runtime::EnrollRequest>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    let request = parse_body(body)?;
    let result = runtime::enroll(&state, request, "runtime").await?;
    Ok(ApiOk(result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    search: Option<String>,
    mode: Option<String>,
    archived: Option<bool>,
}

async fn list_providers(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let rows = providers::list_providers(&state.db, false).await?;
    Ok(ApiOk(json!({
        "items": rows.iter().map(providers::user_view).collect::<Vec<_>>()
    })))
}

async fn get_provider(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let row = providers::load_provider(&state.db, id).await?;
    if !row.enabled {
        return Err(ApiError::not_found("Провайдер не найден."));
    }
    Ok(ApiOk(providers::user_view(&row)))
}

async fn list_conversations(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<ConversationQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Page<Value>>> {
    actor.require("ai.access")?;
    let query = parse_query(query)?;
    let page = conversations::list(
        &state.db,
        actor.id,
        query.page,
        query.per_page,
        query.search.as_deref(),
        query.mode.as_deref(),
        query.archived.unwrap_or(false),
    )
    .await?;
    Ok(ApiOk(page))
}

async fn create_conversation(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<Json<conversations::CreateConversation>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let request = parse_body(body)?;
    Ok(ApiOk(conversations::create(&state, &actor, request).await?))
}

async fn get_conversation(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let row = conversations::owned_conversation(&state.db, id, actor.id).await?;
    let active = state.ai.chats.get(&id).map(|entry| entry.message_id);
    Ok(ApiOk(conversations::conversation_view(&row, active)))
}

async fn update_conversation(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<conversations::UpdateConversation>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let request = parse_body(body)?;
    Ok(ApiOk(
        conversations::update(&state, &actor, id, request).await?,
    ))
}

async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    conversations::remove(&state, &actor, id).await?;
    Ok(ApiOk(json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    tail: Option<bool>,
}

async fn list_messages(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<MessageQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Page<Value>>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let query = parse_query(query)?;
    Ok(ApiOk(
        conversations::messages(
            &state.db,
            actor.id,
            id,
            query.page,
            query.per_page,
            query.tail.unwrap_or(false),
        )
        .await?,
    ))
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<conversations::SendMessage>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&json!({
        "conversationId": id,
        "body": &request,
    }));
    let scope = format!("ai.chat.send:{id}");
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        conversations::send(&state, &actor, id, request),
    )
    .await
}

async fn regenerate_message(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let (conversation_id, message_id) = parse_path(path)?;
    Ok(ApiOk(
        conversations::regenerate(&state, &actor, conversation_id, message_id).await?,
    ))
}

async fn edit_message(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    body: Result<Json<conversations::EditMessage>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let (conversation_id, message_id) = parse_path(path)?;
    let request = parse_body(body)?;
    Ok(ApiOk(
        conversations::edit_message(&state, &actor, conversation_id, message_id, request).await?,
    ))
}

async fn stop_generation(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    Ok(ApiOk(
        conversations::stop_generation(&state, &actor, id).await?,
    ))
}

async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    mut multipart: Multipart,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let conversation_id = parse_path(path)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::validation(format!("Некорректный multipart: {error}")))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if name != "file" {
            continue;
        }
        let file_name = field.file_name().unwrap_or("attachment.txt").to_owned();
        let mime = field.content_type().unwrap_or("text/plain").to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|error| ApiError::validation(format!("Ошибка чтения файла: {error}")))?
            .to_vec();
        let result = conversations::store_attachment(
            &state,
            &actor,
            conversation_id,
            &file_name,
            &mime,
            bytes,
        )
        .await?;
        return Ok(ApiOk(result));
    }
    Err(ApiError::validation(
        "Файл не найден в form-data (поле file).",
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    status: Option<String>,
    /// Список статусов через запятую для групповых фильтров UI.
    statuses: Option<String>,
    conversation_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
}

/// Допустимые статусы Agent-задачи для фильтра.
fn task_statuses(raw: Option<&str>) -> ApiResult<Option<Vec<String>>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let allowed = [
        "queued",
        "running",
        "waiting_approval",
        "completed",
        "failed",
        "cancelled",
    ];
    let mut statuses = Vec::new();
    for value in raw.split(',') {
        let value = value.trim();
        if !allowed.contains(&value) {
            return Err(ApiError::validation("Некорректный фильтр статуса задачи."));
        }
        statuses.push(value.to_owned());
    }
    statuses.sort();
    statuses.dedup();
    Ok(Some(statuses))
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<TaskQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Page<Value>>> {
    actor.require("ai.access")?;
    let query = parse_query(query)?;
    let (page, per_page, offset) = normalize_pagination(query.page, query.per_page);
    let statuses = task_statuses(query.statuses.as_deref())?;
    let total: i64 = sqlx::query_scalar(
        "select count(*) from ai_tasks
          where owner_id = $1
            and ($2::text is null or status = $2)
            and ($3::text[] is null or status = any($3))
            and ($4::uuid is null or conversation_id = $4)
            and ($5::uuid is null or workspace_id = $5)",
    )
    .bind(actor.id)
    .bind(&query.status)
    .bind(&statuses)
    .bind(query.conversation_id)
    .bind(query.workspace_id)
    .fetch_one(&state.db)
    .await?;
    let rows = sqlx::query_as::<_, crate::ai::models::TaskRow>(
        "select id, conversation_id, owner_id, runtime_node_id, provider_id, model,
                workspace_id, status, stage, status_text, progress, iterations,
                max_iterations, tool_calls_count, timeout_seconds, human_input, context,
                cancel_requested, cancel_status, cancel_requested_at, cancel_confirmed_at, started_at, finished_at, error_code, error_message,
                created_at, updated_at
           from ai_tasks
          where owner_id = $1
            and ($2::text is null or status = $2)
            and ($3::text[] is null or status = any($3))
            and ($4::uuid is null or conversation_id = $4)
            and ($5::uuid is null or workspace_id = $5)
          order by created_at desc limit $6 offset $7",
    )
    .bind(actor.id)
    .bind(&query.status)
    .bind(&statuses)
    .bind(query.conversation_id)
    .bind(query.workspace_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(Page::new(
        rows.iter().map(orchestrator::task_view).collect(),
        page,
        per_page,
        total,
    )))
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateTaskRequest {
    conversation_id: Uuid,
    content: String,
    idempotency_key: Option<String>,
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    headers: HeaderMap,
    body: Result<Json<CreateTaskRequest>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require("ai.agent.use")?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&request);
    let scope = format!("ai.task.create:{}", request.conversation_id);
    let conversation_id = request.conversation_id;
    let content = request.content.clone();
    let idempotency_key = request.idempotency_key.clone();
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move {
            let result = conversations::send(
                &st,
                &actor,
                conversation_id,
                conversations::SendMessage {
                    content,
                    attachment_ids: None,
                    idempotency_key,
                    skill_ids: None,
                },
            )
            .await?;
            result
                .get("task")
                .cloned()
                .ok_or_else(|| ApiError::validation("Диалог не в Agent Mode."))
        },
    )
    .await
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let task = orchestrator::owned_task(&state.db, id, actor.id).await?;
    Ok(ApiOk(orchestrator::task_view(&task)))
}

async fn task_events(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let _ = orchestrator::owned_task(&state.db, id, actor.id).await?;
    let query = parse_query(query)?;
    let events =
        orchestrator::task_events(&state.db, id, query.per_page.unwrap_or(200), query.after)
            .await?;
    Ok(ApiOk(json!({ "items": events })))
}

async fn cancel_task(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let request_hash = idempotency::hash_request(&json!({ "taskId": id, "action": "cancel" }));
    let scope = format!("ai.task.cancel:{id}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move { orchestrator::cancel_task(&st, &actor, id).await },
    )
    .await
}

async fn task_executions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let id = parse_path(path)?;
    let _ = orchestrator::owned_task(&state.db, id, actor.id).await?;
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        tool_call_id: String,
        tool_id: String,
        tool_version: i32,
        arguments: Value,
        args_hash: String,
        risk: String,
        approval_id: Option<Uuid>,
        status: String,
        result: Value,
        result_text: Option<String>,
        error_code: Option<String>,
        error_message: Option<String>,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
        finished_at: Option<chrono::DateTime<chrono::Utc>>,
        created_at: chrono::DateTime<chrono::Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select id, tool_call_id, tool_id, tool_version, arguments, args_hash, risk,
                approval_id, status, coalesce(result, '{}'::jsonb) as result, result_text,
                error_code, error_message, exit_code, duration_ms, started_at, finished_at,
                created_at
           from ai_tool_executions where task_id = $1 order by created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "toolCallId": row.tool_call_id,
                "toolId": row.tool_id,
                "toolVersion": row.tool_version,
                "arguments": tools::arguments_for_ui(&row.arguments),
                "argsHash": row.args_hash,
                "risk": row.risk,
                "approvalId": row.approval_id,
                "status": row.status,
                "result": row.result.clone(),
                "changedFiles": row.result.get("changedFiles").cloned().unwrap_or(json!([])),
                "diff": row.result.get("diff").cloned().unwrap_or(Value::Null),
                "resultText": row.result_text.map(|text| text.chars().take(50_000).collect::<String>()),
                "errorCode": row.error_code,
                "errorMessage": row.error_message,
                "exitCode": row.exit_code,
                "durationMs": row.duration_ms,
                "startedAt": row.started_at,
                "finishedAt": row.finished_at,
                "createdAt": row.created_at,
            })
        })
        .collect();
    Ok(ApiOk(json!({ "items": items })))
}

async fn pending_approvals(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.access")?;
    let items = approvals::pending_for_user(&state.db, actor.id).await?;
    Ok(ApiOk(json!({ "items": items })))
}

async fn approve(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<ApprovalDecision>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require("ai.actions.approve")?;
    let id = parse_path(path)?;
    let request = parse_body(body).unwrap_or(ApprovalDecision { reason: None });
    let reason = request.reason.unwrap_or_default();
    let request_hash = idempotency::hash_request(
        &json!({ "approvalId": id, "decision": "approve", "reason": &reason }),
    );
    let scope = format!("ai.approval.approve:{id}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move { approvals::decide(&st, &actor, id, true, &reason).await },
    )
    .await
}

async fn reject(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<ApprovalDecision>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require("ai.actions.approve")?;
    let id = parse_path(path)?;
    let request = parse_body(body).unwrap_or(ApprovalDecision { reason: None });
    let reason = request.reason.unwrap_or_default();
    let request_hash = idempotency::hash_request(
        &json!({ "approvalId": id, "decision": "reject", "reason": &reason }),
    );
    let scope = format!("ai.approval.reject:{id}");
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        &scope,
        &headers,
        request_hash,
        async move { approvals::decide(&st, &actor, id, false, &reason).await },
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalDecision {
    reason: Option<String>,
}

async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let rows = workspaces::list_for_owner(&state.db, actor.id).await?;
    Ok(ApiOk(json!({
        "items": rows.iter().map(|row| row.view(false)).collect::<Vec<_>>()
    })))
}

async fn get_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let id = parse_path(path)?;
    let row = workspaces::accessible(&state.db, id, actor.id, false, false).await?;
    Ok(ApiOk(row.view(false)))
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkspaceBody {
    name: String,
    description: Option<String>,
    runtime_node_id: Option<Uuid>,
}

async fn create_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    headers: HeaderMap,
    body: Result<Json<CreateWorkspaceBody>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require("ai.workspace.write")?;
    let request = parse_body(body)?;
    let request_hash = idempotency::hash_request(&request);
    let st = state.clone();
    idempotency::run_idempotent(
        &state,
        actor.id,
        "ai.workspace.create",
        &headers,
        request_hash,
        async move {
            let row = workspaces::create_for_owner(
                &st,
                &actor,
                workspaces::WorkspaceCreate {
                    name: request.name,
                    description: request.description.unwrap_or_default(),
                    runtime_node_id: request.runtime_node_id,
                },
            )
            .await?;
            Ok(row.view(false))
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWorkspaceBody {
    name: Option<String>,
    description: Option<String>,
}

async fn update_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<UpdateWorkspaceBody>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.write")?;
    let id = parse_path(path)?;
    let request = parse_body(body)?;
    let row =
        workspaces::rename_for_owner(&state, &actor, id, request.name, request.description).await?;
    Ok(ApiOk(row.view(false)))
}

async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.write")?;
    let id = parse_path(path)?;
    let row = workspaces::delete_for_owner(&state, &actor, id).await?;
    Ok(ApiOk(json!({
        "deleted": true,
        "id": row.id,
        "softDelete": true,
        "filesRetained": true,
    })))
}

async fn workspace_overview(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let id = parse_path(path)?;
    let workspace = workspaces::accessible(&state.db, id, actor.id, false, true).await?;
    let mut value = workspaces::overview(&state.db, id).await?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "runtimeOnline".to_owned(),
            json!(state.ai.runtime.is_online(workspace.runtime_node_id)),
        );
    }
    Ok(ApiOk(value))
}

async fn workspace_usage(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let id = parse_path(path)?;
    let _ = workspaces::accessible(&state.db, id, actor.id, false, false).await?;
    Ok(ApiOk(workspaces::usage_summary(&state.db, id).await?))
}

async fn refresh_workspace_usage(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.workspace.read")?;
    let id = parse_path(path)?;
    let _ = workspaces::accessible(&state.db, id, actor.id, false, true).await?;
    workspaces::refresh_usage(&state, id).await?;
    Ok(ApiOk(workspaces::usage_summary(&state.db, id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillQuery {
    workspace_id: Option<Uuid>,
}

async fn list_skills(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<SkillQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.skills.use")?;
    let query = parse_query(query)?;
    if let Some(workspace_id) = query.workspace_id {
        let _ = workspaces::accessible(&state.db, workspace_id, actor.id, false, true).await?;
    }
    let rows = skills::assigned_for_user(&state.db, actor.id, query.workspace_id).await?;
    Ok(ApiOk(json!({
        "items": rows.iter().map(|row| {
            json!({
                "id": row.id,
                "slug": row.slug,
                "name": row.name,
                "description": row.description,
                "version": row.version,
                "dependencies": row.dependencies,
            })
        }).collect::<Vec<_>>()
    })))
}

async fn get_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.skills.use")?;
    let id = parse_path(path)?;
    if !skills::is_assigned(&state.db, actor.id, None, id).await? {
        return Err(ApiError::not_found("Skill не найден."));
    }
    let row = skills::load(&state.db, id).await?;
    Ok(ApiOk(json!({
        "id": row.id,
        "slug": row.slug,
        "name": row.name,
        "description": row.description,
        "version": row.version,
        "content": row.skill_md,
        "dependencies": row.dependencies,
        "untrusted": true,
    })))
}

async fn list_tools(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("ai.tools.use")?;
    Ok(ApiOk(tools::public_catalog(&state.db).await?))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PageQuery {
    per_page: Option<i64>,
    /// Возвращать только события с id больше указанного (инкрементальное
    /// восстановление после reconnect).
    after: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_filter_accepts_known_statuses() {
        let statuses = task_statuses(Some("running,queued,running"))
            .unwrap()
            .unwrap();
        assert_eq!(statuses, vec!["queued".to_owned(), "running".to_owned()]);
        assert!(task_statuses(Some("")).unwrap().is_none());
        assert!(task_statuses(None).unwrap().is_none());
    }

    #[test]
    fn task_status_filter_rejects_unknown_values() {
        assert!(task_statuses(Some("running;drop")).is_err());
        assert!(task_statuses(Some("deleted")).is_err());
    }
}
