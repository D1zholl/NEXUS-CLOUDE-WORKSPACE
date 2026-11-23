use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{ApiOk, Page, normalize_pagination};
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, AuditRow, record};
use crate::system::settings;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/overview", get(overview))
        .route("/modules", get(list_modules))
        .route("/modules/{key}", put(set_module))
        .route("/audit", get(list_audit))
        .merge(settings::system_router())
}

/// Спецификация OpenAPI (внешний роутер, доступна всегда).
pub fn openapi_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/openapi.json", get(openapi_spec))
        .route("/docs", get(swagger_ui))
}

async fn openapi_spec() -> Json<Value> {
    Json(crate::api::openapi::specification(env!(
        "CARGO_PKG_VERSION"
    )))
}

/// Swagger UI подключается с CDN и открывается только при `NEXUS_ENABLE_DOCS=true`.
async fn swagger_ui() -> axum::response::Response {
    let enabled = std::env::var("NEXUS_ENABLE_DOCS")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if !enabled {
        return (
            axum::http::StatusCode::NOT_FOUND,
            "Документация отключена. Задайте NEXUS_ENABLE_DOCS=true.",
        )
            .into_response();
    }
    axum::response::Html(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <title>NEXUS API</title>\
         <link rel=\"stylesheet\" href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\">\
         </head><body><div id=\"swagger-ui\"></div>\
         <script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\"></script>\
         <script>window.onload=function(){window.ui=SwaggerUIBundle({\
         url:'/api/v1/openapi.json',dom_id:'#swagger-ui'});};</script>\
         </body></html>",
    )
    .into_response()
}

pub async fn health(State(state): State<Arc<AppState>>) -> ApiOk<Value> {
    let database_up = tokio::time::timeout(
        Duration::from_secs(12),
        sqlx::query_scalar::<_, i32>("select 1").fetch_one(&state.db),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false);

    ApiOk(json!({
        "status": if database_up { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeSeconds": state.started_at.elapsed().as_secs(),
        "database": if database_up { "up" } else { "down" },
    }))
}

async fn overview(State(state): State<Arc<AppState>>, actor: AuthUser) -> ApiResult<ApiOk<Value>> {
    actor.require("system.view")?;

    let users_total: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&state.db)
        .await?;
    let users_active: i64 =
        sqlx::query_scalar("select count(*) from users where status = 'active'")
            .fetch_one(&state.db)
            .await?;
    let sessions_active: i64 = sqlx::query_scalar(
        "select count(*) from sessions where revoked_at is null and expires_at > now()",
    )
    .fetch_one(&state.db)
    .await?;
    let audit_count: i64 = sqlx::query_scalar("select count(*) from audit_events")
        .fetch_one(&state.db)
        .await?;
    let database_size: i64 = sqlx::query_scalar("select pg_database_size(current_database())")
        .fetch_one(&state.db)
        .await?;

    let metrics = state.metrics.snapshot();
    let uptime = state.started_at.elapsed().as_secs();
    let started_at = Utc::now() - chrono::Duration::seconds(uptime as i64);

    Ok(ApiOk(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": started_at,
        "uptimeSeconds": uptime,
        "database": {
            "status": "up",
            "sizeBytes": database_size,
        },
        "users": {
            "total": users_total,
            "active": users_active,
        },
        "sessions": {
            "active": sessions_active,
        },
        "audit": {
            "events": audit_count,
        },
        "modules": module_map(&state.db).await?,
        "system": metrics,
        "connections": {
            "activeRequests": state.requests.active_connections(),
            "activeDownloads": state.requests.active_downloads(),
            "activeWebSockets": state.realtime.active(),
        },
    })))
}

async fn module_map(db: &sqlx::PgPool) -> ApiResult<HashMap<String, bool>> {
    let rows = sqlx::query_as::<_, (String, bool)>("select key, enabled from module_states")
        .fetch_all(db)
        .await?;
    let mut map: HashMap<String, bool> = HashMap::new();
    map.insert("minecraft".into(), true);
    map.insert("cloud".into(), true);
    map.insert("ai".into(), true);
    for (key, enabled) in rows {
        map.insert(key, enabled);
    }
    Ok(map)
}

#[derive(serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct ModuleRow {
    key: String,
    enabled: bool,
    updated_at: DateTime<Utc>,
}

async fn list_modules(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Vec<ModuleRow>>> {
    actor.require("system.view")?;
    let rows = sqlx::query_as::<_, ModuleRow>(
        "select key, enabled, updated_at from module_states order by key",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(rows))
}

#[derive(Deserialize)]
struct SetModuleRequest {
    enabled: bool,
}

async fn set_module(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    body: Result<Json<SetModuleRequest>, JsonRejection>,
) -> ApiResult<ApiOk<ModuleRow>> {
    actor.require("system.settings.manage")?;
    let key = path
        .map(|Path(v)| v)
        .map_err(|_| ApiError::validation("Некорректный ключ модуля."))?;
    if !matches!(key.as_str(), "minecraft" | "cloud" | "ai") {
        return Err(ApiError::not_found("Модуль не найден."));
    }
    let body = body
        .map(|Json(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректное тело запроса: {e}")))?;
    let row = sqlx::query_as::<_, ModuleRow>(
        "update module_states set enabled = $2, updated_at = now(), updated_by = $3
         where key = $1
         returning key, enabled, updated_at",
    )
    .bind(&key)
    .bind(body.enabled)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Модуль не найден."))?;

    // Новое состояние должно действовать немедленно, без ожидания TTL кэша.
    state.modules.invalidate().await;

    record(
        &state.db,
        AuditEvent {
            event_type: "module.toggled",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "module",
            target_id: &key,
            result: "success",
            ip: &actor.ip,
            details: json!({ "enabled": body.enabled }),
        },
    )
    .await;
    Ok(ApiOk(row))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    event_type: Option<String>,
}

async fn list_audit(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<AuditQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Page<AuditRow>>> {
    actor.require("system.view")?;
    let query = query
        .map(|Query(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректные параметры запроса: {e}")))?;
    let (page, per_page, offset) = normalize_pagination(query.page, query.per_page);
    let event_type = query
        .event_type
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let total: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where ($1::text is null or event_type = $1)",
    )
    .bind(&event_type)
    .fetch_one(&state.db)
    .await?;

    let rows = sqlx::query_as::<_, AuditRow>(
        "select id, created_at, event_type, actor_id, actor_username,
                target_type, target_id, result, ip, details
         from audit_events
         where ($1::text is null or event_type = $1)
         order by created_at desc
         limit $2 offset $3",
    )
    .bind(&event_type)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(ApiOk(Page::new(rows, page, per_page, total)))
}
