//! Публичный репозиторий MyLauncher.
//!
//! Пути и формат зафиксированы в `docs/LAUNCHER_CONTRACT.md`. Клиент не
//! отправляет токен, поэтому чтение опубликованных публичных сборок доступно
//! анонимно. Если заголовок `Authorization` всё же передан, токен проверяется:
//! это открывает приватные сборки владельца и даёт единые scopes/лимиты.

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, http::request::Parts};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::artifacts;
use super::catalog::ArtifactRef;
use super::tokens::{self, LauncherClient, SCOPE_ARTIFACTS, SCOPE_CATALOG};
use crate::auth::rate_limit::LimitSnapshot;
use crate::core::error::{ApiError, ApiResult};
use crate::minecraft::launcher_manifest;
use crate::state::AppState;

/// Публичные маршруты репозитория MyLauncher.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/packs", get(packs))
        .route("/api/packs/{pack_id}/versions", get(versions))
        .route(
            "/api/packs/{pack_id}/versions/{version}/manifest",
            get(manifest),
        )
        .route("/api/packs/{pack_id}/icon", get(icon))
        .route("/api/files/{hash}", get(file))
}

/// IP клиента без обязательного `ConnectInfo` (в тестах его может не быть).
struct PeerInfo {
    ip: String,
}

impl FromRequestParts<Arc<AppState>> for PeerInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let ip = crate::middleware::client_ip(
            &parts.headers,
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|value| value.0.ip()),
            state.config.server.trust_proxy,
        );
        Ok(Self { ip })
    }
}

#[derive(Debug, Deserialize)]
struct ChannelQuery {
    channel: Option<String>,
}

fn channel(value: Option<&str>) -> ApiResult<String> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("stable") => Ok("stable".to_owned()),
        Some("beta") => Ok("beta".to_owned()),
        Some(_) => Err(ApiError::validation(
            "Канал репозитория должен быть stable или beta.",
        )),
    }
}

fn key_for(client: &Option<LauncherClient>, ip: &str) -> String {
    match client {
        Some(client) => format!("token:{}", client.token_id),
        None => format!("ip:{ip}"),
    }
}

fn with_limits(mut response: Response, limit: LimitSnapshot) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        ("x-ratelimit-limit", limit.limit as u64),
        ("x-ratelimit-remaining", limit.remaining as u64),
        ("x-ratelimit-reset", limit.reset_seconds),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
            headers.insert(name, value);
        }
    }
    response
}

fn enforce(state: &AppState, key: &str, artifacts: bool) -> Result<LimitSnapshot, Box<Response>> {
    let limiter = if artifacts {
        &state.artifact_limiter
    } else {
        &state.launcher_limiter
    };
    match limiter.try_acquire(key) {
        Ok(snapshot) => Ok(snapshot),
        Err(retry_after) => {
            if limiter.overruns(key) >= 10 {
                tracing::warn!(key, "систематическое превышение лимита репозитория");
            }
            let error = ApiError::rate_limited(retry_after)
                .with_header("x-ratelimit-limit", limiter.limit())
                .with_header("x-ratelimit-remaining", 0)
                .with_header("x-ratelimit-reset", retry_after);
            Err(Box::new(error.into_response()))
        }
    }
}

async fn viewer(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    ip: &str,
    scope: &str,
) -> ApiResult<Option<LauncherClient>> {
    let client = tokens::optional_client(state, headers, ip.to_owned()).await?;
    if let Some(client) = &client {
        client.require(scope)?;
    }
    Ok(client)
}

#[derive(sqlx::FromRow)]
struct PackRow {
    slug: String,
    name: String,
    description: String,
    category: String,
    author: Option<String>,
    has_icon: bool,
    latest_version: String,
    channel: String,
}

const PACK_QUERY: &str = "select b.slug, b.name, b.description, b.category,
        (select coalesce(nullif(u.display_name, ''), u.username)
           from users u where u.id = coalesce(b.owner_id, b.created_by)) as author,
        exists(select 1 from storage_objects o
               where o.id = b.icon_object_id and o.state = 'committed') as has_icon,
        r.version as latest_version, r.channel
     from minecraft_builds b
     join lateral (
        select rr.version, rr.channel
        from minecraft_releases rr
        where rr.build_id = b.id and rr.channel = $2 and rr.status = 'published'
        order by coalesce(rr.id = b.current_release_id, false) desc,
                 rr.published_at desc nulls last, rr.created_at desc
        limit 1
     ) r on true
     where b.archived_at is null
       and b.status = 'published'
       and (b.visibility = 'public' or b.owner_id = $1)";

fn pack_json(state: &AppState, row: &PackRow) -> serde_json::Value {
    json!({
        "id": row.slug,
        "name": row.name,
        "description": row.description,
        "authors": row.author.iter().cloned().collect::<Vec<_>>(),
        "categories": [row.category],
        "iconUrl": row.has_icon.then(|| {
            launcher_manifest::icon_url(&state.config.server.public_url, &row.slug)
        }),
        "latestVersion": row.latest_version,
        "channel": row.channel,
    })
}

/// `GET /api/packs?channel=stable|beta`
async fn packs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: PeerInfo,
    query: Result<Query<ChannelQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Response> {
    let Query(query) =
        query.map_err(|_| ApiError::validation("Некорректные параметры каталога."))?;
    let channel = channel(query.channel.as_deref())?;
    let client = viewer(&state, &headers, &peer.ip, SCOPE_CATALOG).await?;
    let key = key_for(&client, &peer.ip);
    let limit = match enforce(&state, &key, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let rows: Vec<PackRow> = sqlx::query_as(&format!("{PACK_QUERY} order by b.name, b.slug"))
        .bind(client.as_ref().map(|value| value.user_id))
        .bind(&channel)
        .fetch_all(&state.db)
        .await?;
    let packs: Vec<serde_json::Value> = rows.iter().map(|row| pack_json(&state, row)).collect();
    Ok(with_limits(
        Json(json!({ "packs": packs })).into_response(),
        limit,
    ))
}

/// `GET /api/packs/{pack_id}/versions?channel=...`
async fn versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: PeerInfo,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ChannelQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Response> {
    let Path(pack_id) = path.map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    if !launcher_manifest::valid_pack_id(&pack_id) {
        return Err(ApiError::not_found("Сборка не найдена."));
    }
    let Query(query) = query.map_err(|_| ApiError::validation("Некорректные параметры версий."))?;
    let channel = channel(query.channel.as_deref())?;
    let client = viewer(&state, &headers, &peer.ip, SCOPE_CATALOG).await?;
    let key = key_for(&client, &peer.ip);
    let limit = match enforce(&state, &key, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let exists: Option<Uuid> = sqlx::query_scalar(
        "select b.id from minecraft_builds b
         where b.slug = $1 and b.archived_at is null
           and (b.visibility = 'public' or b.owner_id = $2)",
    )
    .bind(&pack_id)
    .bind(client.as_ref().map(|value| value.user_id))
    .fetch_optional(&state.db)
    .await?;
    let Some(build_id) = exists else {
        return Err(ApiError::not_found("Сборка не найдена."));
    };
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "select r.version, r.channel,
                extract(epoch from coalesce(r.published_at, r.created_at))::bigint
         from minecraft_releases r
         where r.build_id = $1 and r.channel = $2
           and r.status in ('published', 'deprecated')
         order by coalesce(
                    r.id = (select current_release_id from minecraft_builds where id = $1),
                    false
                  ) desc,
                  (r.status = 'published') desc,
                  r.published_at desc nulls last, r.created_at desc",
    )
    .bind(build_id)
    .bind(&channel)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Err(ApiError::not_found(
            "В этом канале нет опубликованных версий.",
        ));
    }
    let versions: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(version, channel, updated_at)| {
            json!({ "version": version, "channel": channel, "updatedAt": updated_at })
        })
        .collect();
    Ok(with_limits(
        Json(json!({ "versions": versions })).into_response(),
        limit,
    ))
}

/// `GET /api/packs/{pack_id}/versions/{version}/manifest?channel=...`
async fn manifest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: PeerInfo,
    path: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ChannelQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Response> {
    let Path((pack_id, version)) =
        path.map_err(|_| ApiError::not_found("Версия сборки не найдена."))?;
    if !launcher_manifest::valid_pack_id(&pack_id) || !launcher_manifest::valid_component(&version)
    {
        return Err(ApiError::not_found("Версия сборки не найдена."));
    }
    let Query(query) =
        query.map_err(|_| ApiError::validation("Некорректный канал репозитория."))?;
    let channel = channel(query.channel.as_deref())?;
    let client = viewer(&state, &headers, &peer.ip, SCOPE_CATALOG).await?;
    let key = key_for(&client, &peer.ip);
    let limit = match enforce(&state, &key, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "select b.id, r.id
         from minecraft_builds b
         join minecraft_releases r on r.build_id = b.id
         where b.slug = $1 and b.archived_at is null
           and (b.visibility = 'public' or b.owner_id = $2)
           and r.version = $3 and r.channel = $4
           and r.status in ('published', 'deprecated')
         limit 1",
    )
    .bind(&pack_id)
    .bind(client.as_ref().map(|value| value.user_id))
    .bind(&version)
    .bind(&channel)
    .fetch_optional(&state.db)
    .await?;
    let Some((build_id, release_id)) = row else {
        return Err(ApiError::not_found("Версия сборки не найдена."));
    };
    let manifest = launcher_manifest::snapshot(&state.db, build_id, release_id).await?;
    Ok(with_limits(Json(manifest).into_response(), limit))
}

/// `GET /api/packs/{pack_id}/icon`
async fn icon(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: PeerInfo,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> ApiResult<Response> {
    let Path(pack_id) = path.map_err(|_| ApiError::not_found("Сборка не найдена."))?;
    if !launcher_manifest::valid_pack_id(&pack_id) {
        return Err(ApiError::not_found("Сборка не найдена."));
    }
    let client = viewer(&state, &headers, &peer.ip, SCOPE_CATALOG).await?;
    let key = key_for(&client, &peer.ip);
    let limit = match enforce(&state, &key, false) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let build_id: Option<Uuid> = sqlx::query_scalar(
        "select b.id from minecraft_builds b
         where b.slug = $1 and b.archived_at is null
           and (b.visibility = 'public' or b.owner_id = $2)",
    )
    .bind(&pack_id)
    .bind(client.as_ref().map(|value| value.user_id))
    .fetch_optional(&state.db)
    .await?;
    let Some(build_id) = build_id else {
        return Err(ApiError::not_found("Сборка не найдена."));
    };
    let Some(icon) = crate::minecraft::builds::icon_object(&state.db, build_id).await? else {
        return Err(ApiError::not_found("Иконка не задана."));
    };
    let response =
        crate::storage::http::stream_object(&icon.location, icon.size, &headers, icon.content_type)
            .await?;
    let mut response = response;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("\"{}\"", icon.sha256)) {
        response.headers_mut().insert(header::ETAG, value);
    }
    Ok(with_limits(response, limit))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileQuery {
    pack_id: Option<String>,
    version: Option<String>,
    channel: Option<String>,
}

fn download_key(ip: &str) -> Uuid {
    let digest = Sha256::digest(ip.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

type FileArtifactRow = (Uuid, Uuid, Uuid, String, i64, String, Option<Uuid>);

/// `GET /api/files/sha256:{hex}?packId=&version=&channel=`
async fn file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: PeerInfo,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    query: Result<Query<FileQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Response> {
    let Path(hash) = path.map_err(|_| ApiError::not_found("Файл не найден."))?;
    let Some(digest) = hash.strip_prefix("sha256:") else {
        return Err(ApiError::not_found("Файл не найден."));
    };
    let digest = digest.to_ascii_lowercase();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::not_found("Файл не найден."));
    }
    let Query(query) = query.map_err(|_| ApiError::validation("Некорректные параметры файла."))?;
    let pack_id = query.pack_id.filter(|value| !value.is_empty());
    if let Some(pack_id) = &pack_id {
        if !launcher_manifest::valid_pack_id(pack_id) {
            return Err(ApiError::not_found("Файл не найден."));
        }
    }
    let version = query.version.filter(|value| !value.is_empty());
    if let Some(version) = &version {
        if !launcher_manifest::valid_component(version) {
            return Err(ApiError::not_found("Файл не найден."));
        }
    }
    let channel = match query.channel.as_deref() {
        None => None,
        Some(value) => Some(channel(Some(value))?),
    };

    let client = viewer(&state, &headers, &peer.ip, SCOPE_ARTIFACTS).await?;
    let key = key_for(&client, &peer.ip);
    let limit = match enforce(&state, &key, true) {
        Ok(limit) => limit,
        Err(response) => return Ok(*response),
    };
    let row: Option<FileArtifactRow> = sqlx::query_as(
        "select rf.id, r.build_id, r.id, rf.path, rf.size_bytes, rf.sha256, rf.object_id
         from minecraft_release_files rf
         join minecraft_releases r on r.id = rf.release_id
         join minecraft_builds b on b.id = r.build_id
         where rf.sha256 = $1
           and r.status in ('published', 'deprecated')
           and b.archived_at is null
           and (b.visibility = 'public' or b.owner_id = $2)
           and ($3::text is null or b.slug = $3)
           and ($4::text is null or r.version = $4)
           and ($5::text is null or r.channel = $5)
         order by r.published_at desc nulls last, r.created_at desc
         limit 1",
    )
    .bind(&digest)
    .bind(client.as_ref().map(|value| value.user_id))
    .bind(&pack_id)
    .bind(&version)
    .bind(&channel)
    .fetch_optional(&state.db)
    .await?;
    let Some((artifact_id, build_id, release_id, path, size_bytes, sha256, object_id)) = row else {
        return Err(ApiError::not_found("Файл не найден."));
    };
    // Подмена размера/хэша в БД не должна отдавать некорректный артефакт.
    if !sha256.eq_ignore_ascii_case(&digest) {
        return Err(ApiError::not_found("Файл не найден."));
    }
    if crate::minecraft::validation::check_manifest_path(&path).is_err() {
        return Err(ApiError::not_found("Файл не найден."));
    }
    let Some(object_id) = object_id else {
        return Err(ApiError::conflict(
            "OBJECT_UNAVAILABLE",
            "Файл недоступен в хранилище.",
        ));
    };

    let slot = match state.acquire_download(
        client
            .as_ref()
            .map(|value| value.token_id)
            .unwrap_or_else(|| download_key(&peer.ip)),
    ) {
        Ok(slot) => slot,
        Err(retry_after) => {
            return Ok(ApiError::unavailable(
                "DOWNLOAD_LIMIT_REACHED",
                "Слишком много параллельных скачиваний. Повторите позже.",
                retry_after,
            )
            .into_response());
        }
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();
    let artifact = ArtifactRef {
        build_id,
        release_id,
        path,
        size_bytes,
        sha256,
        object_id,
    };
    let response = artifacts::download_resolved(
        &state,
        artifact_id,
        &artifact,
        client.as_ref().map(|value| value.token_id),
        &peer.ip,
        &user_agent,
        &headers,
    )
    .await?;
    let (parts, body) = with_limits(response, limit).into_parts();
    let body = axum::body::Body::from_stream(super::downloads::PermitStream::new(body, slot));
    Ok(Response::from_parts(parts, body))
}
