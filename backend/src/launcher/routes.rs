//! API лаунчера: `/api/v1/launcher/*`.
//!
//! Публичные методы (каталог, манифесты) требуют токен лаунчера,
//! но не браузерную сессию. Токены выдаются по логину/паролю пользователя.

use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use uuid::Uuid;

use super::catalog;
use super::tokens::{self, CreateTokenRequest, LauncherClient, TokenRow};
use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::auth::rate_limit::LimitSnapshot;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const MANAGE: &str = "launcher.manage";

fn set_number_header(headers: &mut HeaderMap, name: &'static str, value: u64) {
    if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
        headers.insert(name, value);
    }
}

fn with_limits(mut response: Response, limit: LimitSnapshot) -> Response {
    let headers = response.headers_mut();
    set_number_header(headers, "x-ratelimit-limit", limit.limit as u64);
    set_number_header(headers, "x-ratelimit-remaining", limit.remaining as u64);
    set_number_header(headers, "x-ratelimit-reset", limit.reset_seconds);
    response
}

/// Проверяет лимит запросов по токену лаунчера и готовит заголовки `RateLimit`.
///
/// При превышении возвращает готовый ответ 429 с `Retry-After` и `X-RateLimit-*`.
fn enforce_limit(
    state: &AppState,
    client: &LauncherClient,
    artifacts: bool,
) -> Result<LimitSnapshot, Box<Response>> {
    let limiter = if artifacts {
        &state.artifact_limiter
    } else {
        &state.launcher_limiter
    };
    let key = client.token_id.to_string();
    match limiter.try_acquire(&key) {
        Ok(snapshot) => Ok(snapshot),
        Err(retry_after) => {
            if limiter.overruns(&key) >= 10 {
                tracing::warn!(
                    token = %client.token_id,
                    "систематическое превышение лимита запросов лаунчера"
                );
            }
            let error = ApiError::rate_limited(retry_after)
                .with_header("x-ratelimit-limit", limiter.limit())
                .with_header("x-ratelimit-remaining", 0)
                .with_header("x-ratelimit-reset", retry_after);
            Err(Box::new(error.into_response()))
        }
    }
}

/// Маршруты, доступные по токену лаунчера.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/token", post(issue_token))
        .route("/auth/revoke", post(revoke_self))
        .route("/catalog", get(get_catalog))
        .route("/builds", get(list_builds))
        .route("/builds/{slug}", get(get_build))
        .route("/builds/{slug}/icon", get(get_build_icon))
        .route("/builds/{slug}/releases", get(list_releases))
        .route("/builds/{slug}/latest/manifest", get(latest_manifest))
        .route(
            "/builds/{slug}/releases/{version}/manifest",
            get(release_manifest),
        )
        .route("/artifacts/{id}", get(download_artifact))
}

/// Административные маршруты управления токенами (браузерная сессия).
pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tokens", get(list_tokens))
        .route("/tokens/{id}", axum::routing::delete(revoke_token))
        .route("/repository", get(repository_info))
}

fn parse_json<T>(body: Result<axum::Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|axum::Json(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректное тело запроса: {error}")))
}

/* ---------------------------------- токены --------------------------------- */

async fn issue_token(
    State(state): State<Arc<AppState>>,
    body: Result<axum::Json<CreateTokenRequest>, JsonRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let request = parse_json(body)?;
    let name = request.name.clone();
    let username = request.username.clone();
    let issued = tokens::issue_token(&state, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "launcher.token.issued",
            actor_id: None,
            actor_username: &username,
            target_type: "access_token",
            target_id: &name,
            result: "success",
            ip: "",
            details: json!({ "name": name, "scopes": issued.scopes }),
        },
    )
    .await;
    Ok(ApiOk(json!({
        "token": issued.token,
        "tokenType": "Bearer",
        "scopes": issued.scopes,
        "expiresAt": issued.expires_at,
    })))
}

async fn revoke_self(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
) -> ApiResult<ApiOk<serde_json::Value>> {
    tokens::revoke_self(&state.db, client.token_id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "launcher.token.revoked_self",
            actor_id: Some(client.user_id),
            actor_username: "launcher",
            target_type: "access_token",
            target_id: &client.token_id.to_string(),
            result: "success",
            ip: &client.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "revoked": true })))
}

async fn list_tokens(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Vec<TokenRow>>> {
    actor.require(MANAGE)?;
    Ok(ApiOk(tokens::list_tokens(&state.db).await?))
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    tokens::revoke_token(&state.db, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "launcher.token.revoked",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "access_token",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "revoked": true })))
}

/// Параметры подключения MyLauncher к этому серверу.
///
/// Клиент не умеет отправлять токен, поэтому отдаём это явно: страница
/// интеграции не должна обещать приватный доступ, которого нет в клиенте.
async fn repository_info(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(MANAGE)?;
    let base_url = state.config.server.public_url.trim_end_matches('/');
    let row: (i64, i64) = sqlx::query_as(
        "select
            (select count(*)::bigint from minecraft_builds b
              where b.visibility = 'public' and b.status = 'published'
                and b.archived_at is null
                and exists(select 1 from minecraft_releases r
                           where r.build_id = b.id and r.status = 'published')),
            (select count(*)::bigint from minecraft_releases r
              join minecraft_builds b on b.id = r.build_id
              where r.status = 'published' and b.visibility = 'public'
                and b.archived_at is null)",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(ApiOk(json!({
        "baseUrl": base_url,
        "catalogUrl": format!("{base_url}/api/packs"),
        "channels": ["stable", "beta"],
        "publicPacks": row.0,
        "publishedReleases": row.1,
        "client": "MyLauncher 0.1.0",
        "clientTokenSupport": false,
        "tokenNote": "Stock MyLauncher не отправляет токен и запрещает query/учётные данные в URL. Приватные сборки этому клиенту недоступны; Bearer поддержан только для будущих клиентов.",
        "endpoints": {
            "catalog": "/api/packs?channel={stable|beta}",
            "versions": "/api/packs/{packId}/versions?channel={stable|beta}",
            "manifest": "/api/packs/{packId}/versions/{version}/manifest?channel={stable|beta}",
            "file": "/api/files/sha256:{hex}?packId=&version=&channel=",
            "icon": "/api/packs/{packId}/icon"
        }
    })))
}

/* --------------------------------- каталог -------------------------------- */

async fn get_catalog(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let builds = catalog::catalog(&state.db, client.user_id).await?;
    Ok(with_limits(
        ApiOk(json!({
            "builds": builds.iter().map(catalog::build_json).collect::<Vec<_>>(),
            "count": builds.len(),
        }))
        .into_response(),
        limit,
    ))
}

async fn list_builds(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let builds = catalog::catalog(&state.db, client.user_id).await?;
    Ok(with_limits(
        ApiOk(json!({
            "items": builds.iter().map(catalog::build_json).collect::<Vec<_>>(),
            "total": builds.len(),
        }))
        .into_response(),
        limit,
    ))
}

async fn get_build(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<String>, PathRejection>,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let slug = path
        .map(|Path(slug)| slug)
        .map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    let build = catalog::build_by_slug(&state.db, &slug, client.user_id).await?;
    Ok(with_limits(
        ApiOk(catalog::build_json(&build)).into_response(),
        limit,
    ))
}

async fn get_build_icon(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let slug = path
        .map(|Path(slug)| slug)
        .map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    // Видимость проверяется тем же запросом, что и доступ к релизам: приватная
    // сборка чужому токену отвечает 404, не раскрывая существование иконки.
    // Устаревший текущий релиз не должен скрывать иконку, если манифест версии
    // всё ещё доступен.
    let build = catalog::build_by_slug_for_release(&state.db, &slug, client.user_id).await?;
    let Some(icon) = crate::minecraft::builds::icon_object(&state.db, build.id).await? else {
        return Err(ApiError::not_found("Иконка не задана."));
    };
    let mut response =
        crate::storage::http::stream_object(&icon.location, icon.size, &headers, icon.content_type)
            .await?;
    {
        let headers = response.headers_mut();
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=60"),
        );
        if let Ok(value) = HeaderValue::from_str(&format!("\"{}\"", icon.sha256)) {
            headers.insert(header::ETAG, value);
        }
    }
    Ok(with_limits(response, limit))
}

async fn list_releases(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<String>, PathRejection>,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let slug = path
        .map(|Path(slug)| slug)
        .map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    let build = catalog::build_by_slug(&state.db, &slug, client.user_id).await?;
    let releases = catalog::releases(&state.db, build.id).await?;
    Ok(with_limits(
        ApiOk(json!({
            "slug": build.slug,
            "latest": build.release_version,
            "releases": releases,
        }))
        .into_response(),
        limit,
    ))
}

async fn latest_manifest(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<String>, PathRejection>,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let slug = path
        .map(|Path(slug)| slug)
        .map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    Ok(with_limits(
        ApiOk(catalog::latest_manifest(&state.db, &slug, client.user_id).await?).into_response(),
        limit,
    ))
}

async fn release_manifest(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<(String, String)>, PathRejection>,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_CATALOG)?;
    let limit = match enforce_limit(&state, &client, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let (slug, version) = path
        .map(|Path((slug, version))| (slug, version))
        .map_err(|_| ApiError::not_found("Релиз не найден."))?;
    Ok(with_limits(
        ApiOk(catalog::release_manifest(&state.db, &slug, &version, client.user_id).await?)
            .into_response(),
        limit,
    ))
}

async fn download_artifact(
    State(state): State<Arc<AppState>>,
    client: LauncherClient,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    client.require(tokens::SCOPE_ARTIFACTS)?;
    let limit = match enforce_limit(&state, &client, true) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::not_found("Артефакт не найден."))?;
    // Слот параллельного скачивания удерживается до конца потока. Если клиент
    // оборвёт соединение, PermitStream будет отброшен и слот освободится.
    let slot = match state.acquire_download(client.token_id) {
        Ok(slot) => slot,
        Err(retry_after) => {
            tracing::info!(
                token = %client.token_id,
                max_parallel = state.downloads.max_parallel(),
                "лимит параллельных скачиваний лаунчера исчерпан"
            );
            let error = ApiError::unavailable(
                "DOWNLOAD_LIMIT_REACHED",
                "Слишком много параллельных скачиваний. Повторите позже.",
                retry_after,
            )
            .with_details(json!({
                "maxParallel": state.downloads.max_parallel(),
                "retry_after_seconds": retry_after,
            }));
            return Ok(error.into_response());
        }
    };
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();
    let response = super::artifacts::download(
        &state,
        id,
        client.token_id,
        client.user_id,
        &client.ip,
        &user_agent,
        &headers,
    )
    .await?;
    let (parts, body) = with_limits(response, limit).into_parts();
    let body = axum::body::Body::from_stream(super::downloads::PermitStream::new(body, slot));
    Ok(Response::from_parts(parts, body))
}
