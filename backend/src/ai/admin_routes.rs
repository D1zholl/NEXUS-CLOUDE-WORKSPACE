//! Административные маршруты AI: Runtime, провайдеры, workspace, Skills,
//! tools/programs, квоты и журналы. Все обработчики требуют `ai.admin.manage`.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::routing::{delete, get, patch, post, put};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ai::models::WorkspaceRow;
use crate::ai::runtime;
use crate::ai::{admin, errors, providers, skills, tools, workspaces};
use crate::api::ApiOk;
use crate::auth::session::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/overview", get(overview))
        .route("/logs", get(logs))
        .route("/runtime-nodes", get(list_nodes))
        .route("/runtime-nodes/{id}", get(node_detail))
        .route("/runtime-nodes/{id}/revoke", post(revoke_node))
        .route("/runtime-nodes/{id}/disable", post(disable_node))
        .route("/runtime-nodes/{id}/enable", post(enable_node))
        .route("/runtime-nodes/{id}/rotate-token", post(rotate_token))
        .route(
            "/enrollment-codes",
            get(list_enrollment_codes).post(create_enrollment),
        )
        .route("/enrollment-codes/{id}/revoke", post(revoke_enrollment))
        .route("/providers", post(create_provider))
        .route(
            "/providers/{id}",
            patch(update_provider).delete(delete_provider),
        )
        .route("/providers/{id}/test", post(test_provider))
        .route("/providers/{id}/models", get(provider_models))
        .route("/providers/{id}/status", get(provider_status))
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route("/workspaces/{id}", patch(update_workspace))
        .route("/skills", get(list_skills))
        .route("/skills/import", post(import_skill))
        .route("/skills/{id}", patch(update_skill).delete(delete_skill))
        .route("/skills/{id}/enable", post(enable_skill))
        .route("/skills/{id}/disable", post(disable_skill))
        .route(
            "/skills/{id}/assignments",
            get(list_assignments).post(assign_skill),
        )
        .route(
            "/skills/{id}/assignments/{assignment_id}",
            delete(remove_assignment),
        )
        .route("/tools", get(list_tools))
        .route("/tools/{id}", patch(update_tool))
        .route("/programs", get(list_programs))
        .route("/programs/{id}", patch(update_program))
        .route("/quotas", get(list_quotas))
        .route("/quotas/{user_id}", put(set_quota))
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

fn admin(actor: &AuthUser) -> ApiResult<()> {
    actor.require("ai.admin.manage")
}

async fn overview(State(state): State<Arc<AppState>>, actor: AuthUser) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    Ok(ApiOk(admin::overview(&state).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    search: Option<String>,
}

async fn logs(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<LogsQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let query = parse_query(query)?;
    Ok(ApiOk(
        admin::logs(
            &state.db,
            query.page,
            query.per_page,
            query.search.as_deref(),
        )
        .await?,
    ))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    Ok(ApiOk(json!({
        "items": admin::runtime_nodes(&state.db, &state).await?
    })))
}

async fn node_detail(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    Ok(ApiOk(
        admin::runtime_node_detail(&state.db, &state, id).await?,
    ))
}

async fn revoke_node(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    runtime::revoke_node(&state, &actor, id).await?;
    Ok(ApiOk(json!({ "id": id, "status": "revoked" })))
}

async fn disable_node(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    runtime::set_status(&state, &actor, id, "disabled").await?;
    Ok(ApiOk(json!({ "id": id, "status": "disabled" })))
}

async fn enable_node(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    runtime::set_status(&state, &actor, id, "offline").await?;
    Ok(ApiOk(
        json!({ "id": id, "status": "offline", "note": "Runtime должен переподключиться" }),
    ))
}

async fn rotate_token(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let token = runtime::rotate_token(&state, &actor, id).await?;
    Ok(ApiOk(json!({
        "id": id,
        "deviceToken": token,
        "note": "Токен показывается один раз. Сохраните его в конфигурации Runtime.",
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentRequest {
    label: Option<String>,
    ttl_seconds: Option<i64>,
    scopes: Option<Vec<String>>,
}

async fn create_enrollment(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<Json<EnrollmentRequest>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let request = parse_body(body)?;
    let scopes = request.scopes.unwrap_or_else(|| vec!["runtime".to_owned()]);
    let (id, code, expires_at) = runtime::create_enrollment_code(
        &state,
        &actor,
        request.label.as_deref().unwrap_or(""),
        request.ttl_seconds.unwrap_or(900),
        &scopes,
    )
    .await?;
    Ok(ApiOk(json!({
        "id": id,
        "code": code,
        "expiresAt": expires_at,
        "note": "Код показывается один раз и действует до первого использования.",
    })))
}

async fn list_enrollment_codes(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    #[derive(sqlx::FromRow, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        id: Uuid,
        label: String,
        scopes: Vec<String>,
        expires_at: chrono::DateTime<chrono::Utc>,
        used_at: Option<chrono::DateTime<chrono::Utc>>,
        used_by_node: Option<Uuid>,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
        created_at: chrono::DateTime<chrono::Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select id, label, scopes, expires_at, used_at, used_by_node, revoked_at, created_at
           from ai_runtime_enrollment_codes order by created_at desc limit 100",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(json!({ "items": rows })))
}

async fn revoke_enrollment(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    sqlx::query(
        "update ai_runtime_enrollment_codes
            set revoked_at = now()
          where id = $1 and used_at is null and revoked_at is null",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.runtime.enrollment_revoked",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_runtime_enrollment",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "id": id, "revoked": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    name: String,
    base_url: String,
    api_key: Option<String>,
    default_model: Option<String>,
    generation: Option<Value>,
    limits: Option<Value>,
    runtime_node_id: Option<Uuid>,
    enabled: Option<bool>,
}

async fn validate_provider_node(db: &sqlx::PgPool, node_id: Option<Uuid>) -> ApiResult<()> {
    if let Some(node_id) = node_id {
        let status: Option<String> =
            sqlx::query_scalar("select status from ai_runtime_nodes where id = $1")
                .bind(node_id)
                .fetch_optional(db)
                .await?;
        match status.as_deref() {
            Some("revoked") => return Err(errors::runtime_revoked()),
            Some("disabled") => {
                return Err(ApiError::validation("Runtime отключён администратором."));
            }
            Some(_) => {}
            None => return Err(ApiError::validation("Runtime Node не найден.")),
        }
    }
    Ok(())
}

async fn create_provider(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<Json<ProviderInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let request = parse_body(body)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::validation(
            "Название провайдера должно содержать 1..100 символов.",
        ));
    }
    providers::validate_base_url(&request.base_url)?;
    validate_provider_node(&state.db, request.runtime_node_id).await?;
    let api_key_cipher = match request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(key) => Some(state.ai.secrets.encrypt(key)?),
        None => None,
    };
    let row = sqlx::query_as::<_, crate::ai::models::ProviderRow>(
        "insert into ai_provider_configs
            (name, base_url, api_key_cipher, default_model, generation, limits,
             runtime_node_id, enabled, created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         returning id, name, provider_type, base_url, api_key_cipher, default_model,
                   generation, limits, runtime_node_id, enabled, status, last_checked_at,
                   last_error, last_models, created_by, created_at, updated_at",
    )
    .bind(name)
    .bind(request.base_url.trim())
    .bind(&api_key_cipher)
    .bind(request.default_model.unwrap_or_default().trim())
    .bind(request.generation.unwrap_or_else(|| json!({})))
    .bind(request.limits.unwrap_or_else(|| json!({})))
    .bind(request.runtime_node_id)
    .bind(request.enabled.unwrap_or(true))
    .bind(actor.id)
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.provider.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_provider",
            target_id: &row.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": row.name, "runtimeNodeId": row.runtime_node_id }),
        },
    )
    .await;
    Ok(ApiOk(row.summary()))
}

async fn update_provider(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<ProviderInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let existing = providers::load_provider(&state.db, id).await?;
    let request = parse_body(body)?;
    providers::validate_base_url(&request.base_url)?;
    validate_provider_node(&state.db, request.runtime_node_id).await?;
    let api_key_cipher = match request.api_key.as_deref() {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(state.ai.secrets.encrypt(value.trim())?),
        None => existing.api_key_cipher.clone(),
    };
    let row = sqlx::query_as::<_, crate::ai::models::ProviderRow>(
        "update ai_provider_configs
            set name = $2, base_url = $3, api_key_cipher = $4, default_model = $5,
                generation = $6, limits = $7, runtime_node_id = $8, enabled = $9,
                status = 'unknown', last_error = null, updated_at = now()
          where id = $1
          returning id, name, provider_type, base_url, api_key_cipher, default_model,
                    generation, limits, runtime_node_id, enabled, status, last_checked_at,
                    last_error, last_models, created_by, created_at, updated_at",
    )
    .bind(id)
    .bind(request.name.trim())
    .bind(request.base_url.trim())
    .bind(&api_key_cipher)
    .bind(
        request
            .default_model
            .unwrap_or(existing.default_model)
            .trim(),
    )
    .bind(request.generation.unwrap_or(existing.generation))
    .bind(request.limits.unwrap_or(existing.limits))
    .bind(request.runtime_node_id)
    .bind(request.enabled.unwrap_or(existing.enabled))
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.provider.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_provider",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": row.name, "runtimeNodeId": row.runtime_node_id }),
        },
    )
    .await;
    Ok(ApiOk(row.summary()))
}

async fn delete_provider(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let _ = providers::load_provider(&state.db, id).await?;
    sqlx::query("update ai_provider_configs set enabled = false, updated_at = now() where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.provider.disabled",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_provider",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "id": id, "enabled": false })))
}

async fn test_provider(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let provider = providers::load_provider(&state.db, id).await?;
    match providers::provider_operation(&state, &provider, "status").await {
        Ok(response) => {
            providers::store_probe_result(&state.db, id, &response.models, None).await;
            Ok(ApiOk(json!({
                "ok": true,
                "models": response.models,
                "data": response.data,
                "checkedAt": chrono::Utc::now(),
            })))
        }
        Err(error) => {
            providers::store_probe_result(&state.db, id, &[], Some(&error.message)).await;
            Err(error)
        }
    }
}

async fn provider_models(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let provider = providers::load_provider(&state.db, id).await?;
    let response = providers::provider_operation(&state, &provider, "models").await?;
    providers::store_probe_result(&state.db, id, &response.models, None).await;
    Ok(ApiOk(
        json!({ "models": response.models, "checkedAt": chrono::Utc::now() }),
    ))
}

async fn provider_status(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let provider = providers::load_provider(&state.db, id).await?;
    let runtime_online = provider
        .runtime_node_id
        .map(|node| state.ai.runtime.is_online(node))
        .unwrap_or(false);
    Ok(ApiOk(json!({
        "id": provider.id,
        "enabled": provider.enabled,
        "status": provider.status,
        "lastCheckedAt": provider.last_checked_at,
        "lastError": provider.last_error,
        "models": provider.last_models,
        "runtimeOnline": runtime_online,
        "runtimeNodeId": provider.runtime_node_id,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceInput {
    owner_id: Uuid,
    runtime_node_id: Uuid,
    name: String,
    root_path: String,
    quota_bytes: Option<i64>,
    max_files: Option<i32>,
    max_file_bytes: Option<i64>,
    max_parallel_tasks: Option<i32>,
    max_task_seconds: Option<i32>,
    allowed_tools: Option<Vec<String>>,
    allowed_programs: Option<Vec<String>>,
    status: Option<String>,
}

async fn validate_workspace_input(
    db: &sqlx::PgPool,
    request: &WorkspaceInput,
) -> ApiResult<(Vec<String>, Vec<String>)> {
    let owner_exists: bool = sqlx::query_scalar("select exists(select 1 from users where id = $1)")
        .bind(request.owner_id)
        .fetch_one(db)
        .await?;
    if !owner_exists {
        return Err(ApiError::validation("Пользователь не найден."));
    }
    let node = runtime::load_node(db, request.runtime_node_id).await?;
    if matches!(node.status.as_str(), "revoked" | "disabled") {
        return Err(ApiError::validation(
            "Нельзя назначить workspace на отключённый или отозванный Runtime.",
        ));
    }
    workspaces::validate_root_path(&request.root_path)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::validation(
            "Название workspace должно содержать 1..100 символов.",
        ));
    }
    let known_tools: std::collections::HashSet<&str> =
        tools::catalog().iter().map(|tool| tool.id).collect();
    let allowed_tools = request
        .allowed_tools
        .clone()
        .unwrap_or_else(tools::default_workspace_tools);
    for tool in &allowed_tools {
        if !known_tools.contains(tool.as_str()) {
            return Err(ApiError::validation(format!(
                "Неизвестный инструмент в allowlist: {tool}"
            )));
        }
    }
    let allowed_programs = request.allowed_programs.clone().unwrap_or_default();
    for program in &allowed_programs {
        let exists: bool = sqlx::query_scalar(
            "select exists(select 1 from ai_programs where runtime_node_id = $1 and program_id = $2)",
        )
        .bind(request.runtime_node_id)
        .bind(program)
        .fetch_one(db)
        .await?;
        if !exists {
            return Err(ApiError::validation(format!(
                "Программа {program} не зарегистрирована на этом Runtime."
            )));
        }
    }
    Ok((allowed_tools, allowed_programs))
}

async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let rows = workspaces::list_all(&state.db).await?;
    Ok(ApiOk(json!({
        "items": rows.iter().map(|row| row.view(true)).collect::<Vec<_>>()
    })))
}

async fn create_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<Json<WorkspaceInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let request = parse_body(body)?;
    let (allowed_tools, allowed_programs) = validate_workspace_input(&state.db, &request).await?;
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "insert into ai_workspaces
            (owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
             max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
             allowed_programs, status, created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                   max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                   allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                   created_by, created_at, updated_at",
    )
    .bind(request.owner_id)
    .bind(request.runtime_node_id)
    .bind(request.name.trim())
    .bind(request.root_path.trim())
    .bind(
        request
            .quota_bytes
            .unwrap_or(workspaces::DEFAULT_QUOTA_BYTES)
            .max(0),
    )
    .bind(
        request
            .max_files
            .unwrap_or(workspaces::DEFAULT_MAX_FILES)
            .max(0),
    )
    .bind(
        request
            .max_file_bytes
            .unwrap_or(workspaces::DEFAULT_MAX_FILE_BYTES)
            .max(0),
    )
    .bind(request.max_parallel_tasks.unwrap_or(1).max(0))
    .bind(request.max_task_seconds.unwrap_or(900).clamp(30, 3600))
    .bind(&allowed_tools)
    .bind(&allowed_programs)
    .bind(request.status.unwrap_or_else(|| "active".to_owned()))
    .bind(actor.id)
    .fetch_one(&state.db)
    .await?;
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
                "ownerId": row.owner_id,
                "runtimeNodeId": row.runtime_node_id,
                "allowedTools": row.allowed_tools,
                "allowedPrograms": row.allowed_programs,
            }),
        },
    )
    .await;
    Ok(ApiOk(row.view(true)))
}

async fn update_workspace(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<WorkspaceInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let existing = workspaces::load_workspace(&state.db, id).await?;
    let request = parse_body(body)?;
    let merged = WorkspaceInput {
        owner_id: if request.owner_id != existing.owner_id {
            request.owner_id
        } else {
            existing.owner_id
        },
        runtime_node_id: request.runtime_node_id,
        name: request.name,
        root_path: request.root_path,
        quota_bytes: request.quota_bytes.or(Some(existing.quota_bytes)),
        max_files: request.max_files.or(Some(existing.max_files)),
        max_file_bytes: request.max_file_bytes.or(Some(existing.max_file_bytes)),
        max_parallel_tasks: request
            .max_parallel_tasks
            .or(Some(existing.max_parallel_tasks)),
        max_task_seconds: request.max_task_seconds.or(Some(existing.max_task_seconds)),
        allowed_tools: request
            .allowed_tools
            .or(Some(existing.allowed_tools.clone())),
        allowed_programs: request
            .allowed_programs
            .or(Some(existing.allowed_programs.clone())),
        status: request.status.or(Some(existing.status.clone())),
    };
    let (allowed_tools, allowed_programs) = validate_workspace_input(&state.db, &merged).await?;
    if !matches!(merged.status.as_deref(), Some("active" | "suspended")) {
        return Err(ApiError::validation(
            "Статус workspace должен быть active или suspended.",
        ));
    }
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "update ai_workspaces
            set owner_id = $2, runtime_node_id = $3, name = $4, root_path = $5,
                quota_bytes = $6, max_files = $7, max_file_bytes = $8,
                max_parallel_tasks = $9, max_task_seconds = $10, allowed_tools = $11,
                allowed_programs = $12, status = $13, updated_at = now()
          where id = $1
          returning id, owner_id, runtime_node_id, name, root_path, quota_bytes, max_files,
                    max_file_bytes, max_parallel_tasks, max_task_seconds, allowed_tools,
                    allowed_programs, status, usage_bytes, usage_files, usage_checked_at,
                    created_by, created_at, updated_at",
    )
    .bind(id)
    .bind(merged.owner_id)
    .bind(merged.runtime_node_id)
    .bind(merged.name.trim())
    .bind(merged.root_path.trim())
    .bind(
        merged
            .quota_bytes
            .unwrap_or(workspaces::DEFAULT_QUOTA_BYTES)
            .max(0),
    )
    .bind(
        merged
            .max_files
            .unwrap_or(workspaces::DEFAULT_MAX_FILES)
            .max(0),
    )
    .bind(
        merged
            .max_file_bytes
            .unwrap_or(workspaces::DEFAULT_MAX_FILE_BYTES)
            .max(0),
    )
    .bind(merged.max_parallel_tasks.unwrap_or(1).max(0))
    .bind(merged.max_task_seconds.unwrap_or(900).clamp(30, 3600))
    .bind(&allowed_tools)
    .bind(&allowed_programs)
    .bind(merged.status.as_deref().unwrap_or("active"))
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.workspace.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_workspace",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "status": row.status, "ownerId": row.owner_id }),
        },
    )
    .await;
    Ok(ApiOk(row.view(true)))
}

async fn list_skills(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let rows = skills::list_all(&state.db).await?;
    Ok(ApiOk(json!({
        "items": rows.iter().map(|row| row.summary()).collect::<Vec<_>>()
    })))
}

async fn import_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    mut multipart: Multipart,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::validation(format!("Некорректный multipart: {error}")))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if !matches!(name.as_str(), "file" | "archive") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| ApiError::validation(format!("Ошибка чтения архива: {error}")))?
            .to_vec();
        let result = skills::import(&state, &actor, &bytes).await?;
        return Ok(ApiOk(result));
    }
    Err(ApiError::validation(
        "ZIP-архив не найден в form-data (поле file).",
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillUpdate {
    name: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
}

async fn update_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<SkillUpdate>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let request = parse_body(body)?;
    let existing = skills::load(&state.db, id).await?;
    let row = sqlx::query_as::<_, crate::ai::models::SkillRow>(
        "update ai_skills
            set name = $2, description = $3, enabled = $4, updated_at = now()
          where id = $1
          returning id, slug, name, description, version, skill_md, checksum, size_bytes,
                    file_count, dependencies, enabled, imported_by, created_at, updated_at",
    )
    .bind(id)
    .bind(request.name.unwrap_or(existing.name).trim())
    .bind(request.description.unwrap_or(existing.description))
    .bind(request.enabled.unwrap_or(existing.enabled))
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.skill.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_skill",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "enabled": row.enabled }),
        },
    )
    .await;
    Ok(ApiOk(row.summary()))
}

async fn enable_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    set_skill_enabled(state, actor, path, true).await
}

async fn disable_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    set_skill_enabled(state, actor, path, false).await
}

async fn set_skill_enabled(
    state: Arc<AppState>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    enabled: bool,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let row = sqlx::query_as::<_, crate::ai::models::SkillRow>(
        "update ai_skills set enabled = $2, updated_at = now() where id = $1
         returning id, slug, name, description, version, skill_md, checksum, size_bytes,
                   file_count, dependencies, enabled, imported_by, created_at, updated_at",
    )
    .bind(id)
    .bind(enabled)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Skill не найден."))?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.skill.enabled_changed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_skill",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "enabled": enabled }),
        },
    )
    .await;
    Ok(ApiOk(row.summary()))
}

async fn delete_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    skills::delete(&state, &actor, id).await?;
    Ok(ApiOk(json!({ "id": id, "deleted": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignmentInput {
    scope: String,
    user_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    role_id: Option<Uuid>,
}

async fn assign_skill(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<AssignmentInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let skill_id = parse_path(path)?;
    let request = parse_body(body)?;
    let _ = skills::load(&state.db, skill_id).await?;
    match request.scope.as_str() {
        "user" => {
            let user_id = request
                .user_id
                .ok_or_else(|| ApiError::validation("Укажите userId."))?;
            let exists: bool =
                sqlx::query_scalar("select exists(select 1 from users where id = $1)")
                    .bind(user_id)
                    .fetch_one(&state.db)
                    .await?;
            if !exists {
                return Err(ApiError::validation("Пользователь не найден."));
            }
        }
        "workspace" => {
            let workspace_id = request
                .workspace_id
                .ok_or_else(|| ApiError::validation("Укажите workspaceId."))?;
            let _ = workspaces::load_workspace(&state.db, workspace_id).await?;
        }
        "role" => {
            let role_id = request
                .role_id
                .ok_or_else(|| ApiError::validation("Укажите roleId."))?;
            let exists: bool =
                sqlx::query_scalar("select exists(select 1 from roles where id = $1)")
                    .bind(role_id)
                    .fetch_one(&state.db)
                    .await?;
            if !exists {
                return Err(ApiError::validation("Роль не найдена."));
            }
        }
        _ => {
            return Err(ApiError::validation(
                "scope должен быть user, workspace или role.",
            ));
        }
    }
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into ai_skill_assignments
            (skill_id, scope, user_id, workspace_id, role_id, assigned_by)
         values ($1, $2, $3, $4, $5, $6)
         on conflict do nothing
         returning id",
    )
    .bind(skill_id)
    .bind(&request.scope)
    .bind(request.user_id)
    .bind(request.workspace_id)
    .bind(request.role_id)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await?;
    let id = match inserted {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "update ai_skill_assignments
                set enabled = true
              where skill_id = $1 and scope = $2
                and coalesce(user_id, '00000000-0000-0000-0000-000000000000'::uuid) =
                    coalesce($3, '00000000-0000-0000-0000-000000000000'::uuid)
                and coalesce(workspace_id, '00000000-0000-0000-0000-000000000000'::uuid) =
                    coalesce($4, '00000000-0000-0000-0000-000000000000'::uuid)
                and coalesce(role_id, '00000000-0000-0000-0000-000000000000'::uuid) =
                    coalesce($5, '00000000-0000-0000-0000-000000000000'::uuid)
              returning id",
            )
            .bind(skill_id)
            .bind(&request.scope)
            .bind(request.user_id)
            .bind(request.workspace_id)
            .bind(request.role_id)
            .fetch_one(&state.db)
            .await?
        }
    };
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.skill.assigned",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_skill",
            target_id: &skill_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "scope": request.scope,
                "userId": request.user_id,
                "workspaceId": request.workspace_id,
                "roleId": request.role_id,
            }),
        },
    )
    .await;
    Ok(ApiOk(
        json!({ "id": id, "skillId": skill_id, "scope": request.scope }),
    ))
}

async fn list_assignments(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let skill_id = parse_path(path)?;
    #[derive(sqlx::FromRow, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        id: Uuid,
        scope: String,
        user_id: Option<Uuid>,
        workspace_id: Option<Uuid>,
        role_id: Option<Uuid>,
        enabled: bool,
        created_at: chrono::DateTime<chrono::Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select id, scope, user_id, workspace_id, role_id, enabled, created_at
           from ai_skill_assignments where skill_id = $1 order by created_at",
    )
    .bind(skill_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(json!({ "items": rows })))
}

async fn remove_assignment(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let (skill_id, assignment_id) = parse_path(path)?;
    sqlx::query("delete from ai_skill_assignments where id = $1 and skill_id = $2")
        .bind(assignment_id)
        .bind(skill_id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.skill.assignment_removed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_skill",
            target_id: &skill_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "assignmentId": assignment_id }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "removed": true })))
}

async fn list_tools(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let rows = tools::policies(&state.db).await?;
    let items: Vec<Value> = tools::catalog()
        .into_iter()
        .map(|spec| {
            let policy = rows.iter().find(|row| row.tool_id == spec.id);
            json!({
                "id": spec.id,
                "name": spec.name,
                "description": spec.description,
                "category": spec.category,
                "serverSide": spec.server_side,
                "inputSchema": spec.input_schema,
                "defaultRisk": spec.risk,
                "enabled": policy.map(|p| p.enabled).unwrap_or(true),
                "risk": policy.map(|p| p.risk.clone()).unwrap_or_else(|| spec.risk.to_owned()),
                "approvalPolicy": policy.map(|p| p.approval_policy.clone()).unwrap_or_else(|| spec.approval_policy.to_owned()),
                "maxTimeoutSeconds": policy.map(|p| p.max_timeout_seconds).unwrap_or(spec.timeout_seconds as i32),
                "maxOutputBytes": policy.map(|p| p.max_output_bytes).unwrap_or(spec.max_output_bytes),
            })
        })
        .collect();
    Ok(ApiOk(json!({ "items": items })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolUpdate {
    enabled: Option<bool>,
    risk: Option<String>,
    approval_policy: Option<String>,
    max_timeout_seconds: Option<i32>,
    max_output_bytes: Option<i64>,
}

async fn update_tool(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<ToolUpdate>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    if tools::by_id(&id).is_none() {
        return Err(ApiError::not_found("Инструмент не найден."));
    }
    let request = parse_body(body)?;
    if let Some(risk) = request.risk.as_deref() {
        if !matches!(risk, "low" | "medium" | "high" | "critical") {
            return Err(ApiError::validation("Недопустимый уровень риска."));
        }
    }
    if let Some(policy) = request.approval_policy.as_deref() {
        if !matches!(policy, "never" | "risk" | "always") {
            return Err(ApiError::validation("Недопустимая политика подтверждения."));
        }
    }
    let row = sqlx::query_as::<_, tools::ToolPolicyRow>(
        "update ai_tool_policies
            set enabled = coalesce($2, enabled), risk = coalesce($3, risk),
                approval_policy = coalesce($4, approval_policy),
                max_timeout_seconds = coalesce($5, max_timeout_seconds),
                max_output_bytes = coalesce($6, max_output_bytes),
                updated_by = $7, updated_at = now()
          where tool_id = $1
          returning tool_id, enabled, risk, approval_policy, max_timeout_seconds, max_output_bytes",
    )
    .bind(&id)
    .bind(request.enabled)
    .bind(request.risk)
    .bind(request.approval_policy)
    .bind(
        request
            .max_timeout_seconds
            .map(|value| value.clamp(1, 3600)),
    )
    .bind(
        request
            .max_output_bytes
            .map(|value| value.clamp(1024, 10 * 1024 * 1024)),
    )
    .bind(actor.id)
    .fetch_one(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.tool.policy_updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_tool",
            target_id: &id,
            result: "success",
            ip: &actor.ip,
            details: json!({
                "enabled": row.enabled,
                "risk": row.risk,
                "approvalPolicy": row.approval_policy,
            }),
        },
    )
    .await;
    Ok(ApiOk(json!(row)))
}

async fn list_programs(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    #[derive(sqlx::FromRow, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        id: Uuid,
        runtime_node_id: Uuid,
        program_id: String,
        name: String,
        executable: String,
        version: String,
        available: bool,
        risk: String,
        enabled: bool,
        updated_at: chrono::DateTime<chrono::Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select id, runtime_node_id, program_id, name, executable, version, available,
                risk, enabled, updated_at
           from ai_programs order by runtime_node_id, program_id",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(json!({ "items": rows })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgramUpdate {
    enabled: Option<bool>,
    risk: Option<String>,
}

async fn update_program(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<ProgramUpdate>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let id = parse_path(path)?;
    let request = parse_body(body)?;
    if let Some(risk) = request.risk.as_deref() {
        if !matches!(risk, "low" | "medium" | "high" | "critical") {
            return Err(ApiError::validation("Недопустимый уровень риска."));
        }
    }
    let updated = sqlx::query(
        "update ai_programs
            set enabled = coalesce($2, enabled), risk = coalesce($3, risk), updated_at = now()
          where id = $1",
    )
    .bind(id)
    .bind(request.enabled)
    .bind(request.risk)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found("Программа не найдена."));
    }
    Ok(ApiOk(json!({ "id": id, "updated": true })))
}

async fn list_quotas(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    #[derive(sqlx::FromRow)]
    struct Row {
        user_id: Uuid,
        username: String,
        display_name: String,
        max_concurrent_tasks: i32,
        max_tasks_per_day: i32,
        max_tool_calls_per_task: i32,
        max_task_seconds: i32,
        max_storage_bytes: i64,
        max_files: i32,
        max_file_bytes: i64,
        updated_at: chrono::DateTime<chrono::Utc>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "select u.id as user_id, u.username, u.display_name,
                coalesce(q.max_concurrent_tasks, 1) as max_concurrent_tasks,
                coalesce(q.max_tasks_per_day, 50) as max_tasks_per_day,
                coalesce(q.max_tool_calls_per_task, 40) as max_tool_calls_per_task,
                coalesce(q.max_task_seconds, 900) as max_task_seconds,
                coalesce(q.max_storage_bytes, 1073741824) as max_storage_bytes,
                coalesce(q.max_files, 10000) as max_files,
                coalesce(q.max_file_bytes, 16777216) as max_file_bytes,
                coalesce(q.updated_at, u.created_at) as updated_at
           from users u left join ai_user_quotas q on q.user_id = u.id
          order by lower(u.username)",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(json!({
        "items": rows.into_iter().map(|row| json!({
            "userId": row.user_id,
            "username": row.username,
            "displayName": row.display_name,
            "maxConcurrentTasks": row.max_concurrent_tasks,
            "maxTasksPerDay": row.max_tasks_per_day,
            "maxToolCallsPerTask": row.max_tool_calls_per_task,
            "maxTaskSeconds": row.max_task_seconds,
            "maxStorageBytes": row.max_storage_bytes,
            "maxFiles": row.max_files,
            "maxFileBytes": row.max_file_bytes,
            "updatedAt": row.updated_at,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaInput {
    max_concurrent_tasks: i32,
    max_tasks_per_day: i32,
    max_tool_calls_per_task: i32,
    max_task_seconds: i32,
    max_storage_bytes: i64,
    max_files: i32,
    max_file_bytes: i64,
}

async fn set_quota(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<QuotaInput>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    admin(&actor)?;
    let user_id = parse_path(path)?;
    let request = parse_body(body)?;
    if !(0..=20).contains(&request.max_concurrent_tasks)
        || !(0..=10_000).contains(&request.max_tasks_per_day)
        || !(1..=500).contains(&request.max_tool_calls_per_task)
        || !(30..=3600).contains(&request.max_task_seconds)
    {
        return Err(ApiError::validation(
            "Значения квот вне допустимого диапазона.",
        ));
    }
    let exists: bool = sqlx::query_scalar("select exists(select 1 from users where id = $1)")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::not_found("Пользователь не найден."));
    }
    sqlx::query(
        "insert into ai_user_quotas
            (user_id, max_concurrent_tasks, max_tasks_per_day, max_tool_calls_per_task,
             max_task_seconds, max_storage_bytes, max_files, max_file_bytes, updated_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         on conflict (user_id) do update
            set max_concurrent_tasks = excluded.max_concurrent_tasks,
                max_tasks_per_day = excluded.max_tasks_per_day,
                max_tool_calls_per_task = excluded.max_tool_calls_per_task,
                max_task_seconds = excluded.max_task_seconds,
                max_storage_bytes = excluded.max_storage_bytes,
                max_files = excluded.max_files,
                max_file_bytes = excluded.max_file_bytes,
                updated_by = excluded.updated_by,
                updated_at = now()",
    )
    .bind(user_id)
    .bind(request.max_concurrent_tasks)
    .bind(request.max_tasks_per_day)
    .bind(request.max_tool_calls_per_task)
    .bind(request.max_task_seconds)
    .bind(request.max_storage_bytes.max(0))
    .bind(request.max_files.max(0))
    .bind(request.max_file_bytes.max(0))
    .bind(actor.id)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "ai.quota.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "ai_user_quota",
            target_id: &user_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "maxConcurrentTasks": request.max_concurrent_tasks }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "userId": user_id, "updated": true })))
}
