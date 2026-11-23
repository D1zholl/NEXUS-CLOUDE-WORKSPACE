//! API Cloud Storage.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::exports::{self, CreateExportRequest, ExportJob};
use super::files::{self, ListQuery};
use super::models::{CloudEntry, breadcrumb};
use super::service;
use super::shares::{self, CreateShareRequest};
use super::uploads::{self, CompleteUploadRequest, CreateUploadRequest, UploadSession};
use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::auth::session::{SESSION_COOKIE, cookie_value, load_user_for_token};
use crate::core::error::{ApiError, ApiResult};
use crate::core::idempotency;
use crate::core::tickets;
use crate::middleware::client_ip;
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const VIEW: &str = "cloud.view";
const UPLOAD: &str = "cloud.file.upload";
const DOWNLOAD: &str = "cloud.file.download";
const DELETE: &str = "cloud.file.delete";
const SHARE: &str = "cloud.file.share";
const FOLDER: &str = "cloud.folder.create";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config", get(config))
        .route("/entries", get(list_entries).post(create_folder))
        .route("/entries/batch", post(create_folders_batch))
        .route(
            "/entries/{id}",
            get(entry_details).patch(update_entry).delete(trash_entry),
        )
        .route("/entries/{id}/restore", post(restore_entry))
        .route("/entries/{id}/purge", axum::routing::delete(purge_entry))
        .route("/entries/{id}/download", get(download_entry))
        .route("/entries/{id}/preview", get(preview_entry))
        .route("/trash", axum::routing::delete(empty_trash))
        .route("/uploads", post(create_upload))
        .route(
            "/uploads/{id}",
            get(upload_status).patch(upload_chunk).delete(abort_upload),
        )
        .route("/uploads/{id}/complete", post(complete_upload))
        .route("/shares", get(list_shares).post(create_share))
        .route("/shares/{id}", axum::routing::delete(revoke_share))
        .route("/exports", get(list_exports).post(create_export))
        .route("/exports/{id}", get(export_status).delete(delete_export))
        .route("/exports/{id}/file", get(export_file))
}

fn parse_json<T>(body: Result<axum::Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|axum::Json(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректное тело запроса: {error}")))
}

fn parse_query<T>(query: Result<Query<T>, QueryRejection>) -> ApiResult<T> {
    query
        .map(|Query(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректные параметры запроса: {error}")))
}

fn parse_id(path: Result<Path<Uuid>, PathRejection>) -> ApiResult<Uuid> {
    path.map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))
}

async fn config(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(uploads::session_config(&state).await))
}

async fn list_entries(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let query = parse_query(query)?;
    let page = files::list_entries(&state.db, actor.id, &query).await?;
    let crumbs = match query.parent_id {
        Some(parent) => breadcrumb(&state.db, parent)
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    let usage: Option<(i64, i64)> =
        sqlx::query_as("select used_bytes, file_count from storage_usage where user_id = $1")
            .bind(actor.id)
            .fetch_optional(&state.db)
            .await?;
    Ok(ApiOk(json!({
        "items": page.items,
        "page": page.page,
        "perPage": page.per_page,
        "total": page.total,
        "breadcrumb": crumbs,
        "usage": {
            "usedBytes": usage.map(|row| row.0).unwrap_or(0),
            "fileCount": usage.map(|row| row.1).unwrap_or(0),
        },
    })))
}

async fn create_folder(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<service::CreateFolderRequest>, JsonRejection>,
) -> ApiResult<ApiOk<CloudEntry>> {
    actor.require(FOLDER)?;
    let request = parse_json(body)?;
    let name = request.name.clone();
    let parent = request.parent_id;
    let entry = service::create_folder(&state.db, actor.id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.folder.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_entry",
            target_id: &entry.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": name, "parentId": parent }),
        },
    )
    .await;
    Ok(ApiOk(entry))
}

/// Пакетное создание папок. Серверный лимит — 200 имён за запрос
/// (см. `service::create_folders_batch`), независимо от клиентского лимита
/// выбора папки в 2000 файлов.
async fn create_folders_batch(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<BatchFoldersBody>, JsonRejection>,
) -> ApiResult<ApiOk<Vec<CloudEntry>>> {
    actor.require(FOLDER)?;
    let body = parse_json(body)?;
    let entries =
        service::create_folders_batch(&state.db, actor.id, body.parent_id, body.names).await?;
    Ok(ApiOk(entries))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchFoldersBody {
    parent_id: Option<Uuid>,
    names: Vec<String>,
}

async fn entry_details(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    files::entry_details(&state.db, actor.id, id).await
}

async fn perform_update_entry(
    state: &AppState,
    actor: &AuthUser,
    id: Uuid,
    request: service::UpdateEntryRequest,
) -> ApiResult<Value> {
    let entry = service::update_entry(&state.db, actor.id, id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.entry.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_entry",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": entry.name, "starred": entry.starred }),
        },
    )
    .await;
    Ok(serde_json::to_value(&entry).unwrap_or(Value::Null))
}

/// E8: `strategy=replace` — опасная операция, повтор с тем же
/// `Idempotency-Key` возвращает сохранённый результат.
async fn update_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<axum::Json<service::UpdateEntryRequest>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    let request = parse_json(body)?;
    let idempotent = request.strategy.as_deref() == Some("replace");
    let request_hash = idempotency::hash_request(&json!({ "entryId": id, "request": &request }));

    if idempotent {
        return idempotency::run_idempotent(
            &state,
            actor.id,
            "cloud.entry.update",
            &headers,
            request_hash,
            perform_update_entry(&state, &actor, id, request),
        )
        .await;
    }

    let value = perform_update_entry(&state, &actor, id, request).await?;
    Ok(ApiOk(value).into_response())
}

async fn trash_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(DELETE)?;
    let id = parse_id(path)?;
    let entry = service::load_entry(&state.db, actor.id, id).await?;
    service::trash_entry(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.entry.trashed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_entry",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": entry.name, "type": entry.entry_type }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "trashed": true })))
}

async fn restore_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<CloudEntry>> {
    actor.require(DELETE)?;
    let id = parse_id(path)?;
    let entry = service::restore_entry(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.entry.restored",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_entry",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": entry.name }),
        },
    )
    .await;
    Ok(ApiOk(entry))
}

async fn purge_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(DELETE)?;
    let id = parse_id(path)?;
    let entry = service::load_entry(&state.db, actor.id, id).await?;
    if entry.trashed_at.is_none() {
        return Err(ApiError::conflict(
            "NOT_IN_TRASH",
            "Сначала переместите запись в корзину.",
        ));
    }
    let removed = service::purge_entry(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.entry.purged",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_entry",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": entry.name, "filesRemoved": removed }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "deleted": true, "files": removed })))
}

async fn empty_trash(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(DELETE)?;
    let removed = service::empty_trash(&state.db, actor.id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.trash.emptied",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_trash",
            target_id: "",
            result: "success",
            ip: &actor.ip,
            details: json!({ "filesRemoved": removed }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "deleted": true, "files": removed })))
}

async fn download_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    actor.require(DOWNLOAD)?;
    let id = parse_id(path)?;
    let response =
        files::stream_file_with_state(Some(&state), &state.db, actor.id, id, &headers, false)
            .await?;
    // Range-запросы (перемотка медиа) не создают отдельную запись аудита.
    if !headers.contains_key(header::RANGE) {
        record(
            &state.db,
            AuditEvent {
                event_type: "cloud.file.downloaded",
                actor_id: Some(actor.id),
                actor_username: &actor.username,
                target_type: "cloud_entry",
                target_id: &id.to_string(),
                result: "success",
                ip: &actor.ip,
                details: json!({}),
            },
        )
        .await;
    }
    Ok(response)
}

async fn preview_entry(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    files::stream_file(&state.db, actor.id, id, &headers, true).await
}

async fn create_upload(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<CreateUploadRequest>, JsonRejection>,
) -> ApiResult<ApiOk<UploadSession>> {
    actor.require(UPLOAD)?;
    let request = parse_json(body)?;
    let name = request.name.clone();
    let size = request.size;
    let session = uploads::create_upload(&state, actor.id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.upload.started",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "upload_session",
            target_id: &session.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": name, "size": size }),
        },
    )
    .await;
    Ok(ApiOk(session))
}

async fn upload_status(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(UPLOAD)?;
    let id = parse_id(path)?;
    let session = uploads::session_offset(&state.db, id, actor.id).await?;
    Ok(ApiOk(json!({
        "id": session.id,
        "offset": session.received_bytes,
        "size": session.declared_size,
        "status": session.status,
        "expiresAt": session.expires_at,
        "chunkMaxBytes": state.chunk_max_bytes().await,
    })))
}

async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(UPLOAD)?;
    let id = parse_id(path)?;
    let offset = headers
        .get("upload-offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| ApiError::validation("Заголовок Upload-Offset обязателен."))?;
    if offset < 0 {
        return Err(ApiError::validation(
            "Upload-Offset не может быть отрицательным.",
        ));
    }
    let received = uploads::append_stream(&state, id, actor.id, offset, body).await?;
    Ok(ApiOk(json!({ "offset": received })))
}

async fn complete_upload(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<axum::Json<CompleteUploadRequest>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require(UPLOAD)?;
    let id = parse_id(path)?;
    let request = parse_json(body)?;
    let request_hash = idempotency::hash_request(&json!({ "uploadId": id, "request": &request }));

    idempotency::run_idempotent(
        &state,
        actor.id,
        "cloud.upload.complete",
        &headers,
        request_hash,
        async {
            let entry = uploads::complete_upload(&state, id, actor.id, request).await?;
            record(
                &state.db,
                AuditEvent {
                    event_type: "cloud.file.uploaded",
                    actor_id: Some(actor.id),
                    actor_username: &actor.username,
                    target_type: "cloud_entry",
                    target_id: &entry.id.to_string(),
                    result: "success",
                    ip: &actor.ip,
                    details: json!({ "name": entry.name, "size": entry.size_bytes }),
                },
            )
            .await;
            state.realtime.publish_user(
                "upload.completed",
                actor.id,
                json!({
                    "uploadId": id,
                    "entryId": entry.id,
                    "name": entry.name,
                    "size": entry.size_bytes,
                }),
            );
            Ok(serde_json::to_value(&entry).unwrap_or(Value::Null))
        },
    )
    .await
}

async fn abort_upload(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(UPLOAD)?;
    let id = parse_id(path)?;
    uploads::abort_upload(&state, id, actor.id).await?;
    Ok(ApiOk(json!({ "aborted": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateShareBody {
    entry_id: Uuid,
    expires_in_hours: Option<i64>,
    password: Option<String>,
    max_downloads: Option<i32>,
    #[serde(default)]
    auth_only: bool,
}

async fn create_share(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    headers: HeaderMap,
    body: Result<axum::Json<CreateShareBody>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require(SHARE)?;
    let body = parse_json(body)?;
    let entry_id = body.entry_id;
    let request = CreateShareRequest {
        expires_in_hours: body.expires_in_hours,
        password: body.password,
        max_downloads: body.max_downloads,
        auth_only: body.auth_only,
    };
    let request_hash =
        idempotency::hash_request(&json!({ "entryId": entry_id, "request": &request }));

    idempotency::run_idempotent(
        &state,
        actor.id,
        "cloud.share.create",
        &headers,
        request_hash,
        async {
            let (row, token) = shares::create_share(&state, actor.id, entry_id, request).await?;
            record(
                &state.db,
                AuditEvent {
                    event_type: "cloud.share.created",
                    actor_id: Some(actor.id),
                    actor_username: &actor.username,
                    target_type: "cloud_entry",
                    target_id: &entry_id.to_string(),
                    result: "success",
                    ip: &actor.ip,
                    details: json!({
                        "shareId": row.id,
                        "hasPassword": row.has_password,
                        "authOnly": row.auth_only,
                    }),
                },
            )
            .await;
            Ok(shares::share_payload(
                &row,
                &token,
                &state.config.server.public_url,
            ))
        },
    )
    .await
}

async fn list_shares(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<SharesQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Vec<shares::ShareRow>>> {
    actor.require(SHARE)?;
    let query = parse_query(query)?;
    Ok(ApiOk(
        shares::list_shares(&state.db, actor.id, query.entry_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharesQuery {
    entry_id: Option<Uuid>,
}

async fn revoke_share(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(SHARE)?;
    let id = parse_id(path)?;
    shares::revoke_share(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.share.revoked",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "cloud_share",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "revoked": true })))
}

async fn create_export(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    headers: HeaderMap,
    body: Result<axum::Json<CreateExportRequest>, JsonRejection>,
) -> ApiResult<Response> {
    actor.require(DOWNLOAD)?;
    let request = parse_json(body)?;
    let request_hash = idempotency::hash_request(&json!({ "request": &request }));

    idempotency::run_idempotent(
        &state,
        actor.id,
        "cloud.export.create",
        &headers,
        request_hash,
        async {
            let job = exports::create_export(&state, actor.id, request).await?;
            // Задачу подхватит планировщик фоновых задач (system/scheduler.rs).
            record(
                &state.db,
                AuditEvent {
                    event_type: "cloud.export.started",
                    actor_id: Some(actor.id),
                    actor_username: &actor.username,
                    target_type: "job",
                    target_id: &job.id.to_string(),
                    result: "success",
                    ip: &actor.ip,
                    details: json!({}),
                },
            )
            .await;
            Ok(serde_json::to_value(&job).unwrap_or(Value::Null))
        },
    )
    .await
}

async fn list_exports(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Vec<ExportJob>>> {
    actor.require(DOWNLOAD)?;
    Ok(ApiOk(exports::list_jobs(&state.db, actor.id).await?))
}

async fn export_status(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<ExportJob>> {
    actor.require(DOWNLOAD)?;
    let id = parse_id(path)?;
    Ok(ApiOk(exports::get_job(&state.db, actor.id, id).await?))
}

async fn delete_export(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(DOWNLOAD)?;
    let id = parse_id(path)?;
    exports::delete_export(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.export.deleted",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "job",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "deleted": true })))
}

async fn export_file(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<Response> {
    actor.require(DOWNLOAD)?;
    let id = parse_id(path)?;
    let file_path = exports::export_file(&state.db, actor.id, id).await?;
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|_| ApiError::not_found("Файл архива не найден."))?;
    let size = tokio::fs::metadata(&file_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let stream = tokio_util::io::ReaderStream::new(file);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            exports::export_filename()
        ))
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        axum::http::HeaderValue::from_str(&size.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("0")),
    );
    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

/* ------------------------------- публичные ссылки ------------------------------- */

pub fn public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{token}", get(public_share_info))
        .route("/{token}/unlock", post(public_share_unlock))
        .route("/{token}/download", get(public_share_download))
}

#[derive(Deserialize, Default)]
struct UnlockRequest {
    #[serde(default)]
    password: String,
}

#[derive(Deserialize)]
struct PublicDownloadQuery {
    /// Короткоживущий тикет, выпущенный `unlock`.
    #[serde(default)]
    ticket: Option<String>,
}

/// E3: ссылка с `auth_only` требует действующую cookie-сессию.
///
/// Проверка ручная, а не через `AuthUser`: публичный роутер должен оставаться
/// доступным без сессии для обычных ссылок. `load_user_for_token` возвращает
/// 401 для отсутствующей, истёкшей или отозванной сессии.
async fn require_share_session(
    state: &AppState,
    share: &shares::PublicShare,
    headers: &HeaderMap,
) -> ApiResult<()> {
    if !share.auth_only {
        return Ok(());
    }
    let Some(token) = cookie_value(headers, SESSION_COOKIE) else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "SHARE_AUTH_REQUIRED",
            "Ссылка доступна только авторизованным пользователям.",
        )
        .with_details(json!({ "authRequired": true })));
    };
    load_user_for_token(state, &token).await?;
    Ok(())
}

async fn public_share_info(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let token = path
        .map(|Path(token)| token)
        .map_err(|_| ApiError::not_found("Ссылка не найдена."))?;
    let share = shares::resolve_public_share(&state.db, &token).await?;
    require_share_session(&state, &share, &headers).await?;
    Ok(ApiOk(json!({
        "name": share.name,
        "size": share.size_bytes,
        "contentType": share.content_type,
        "hasPassword": share.has_password,
        "authOnly": share.auth_only,
        "downloadCount": share.download_count,
        "maxDownloads": share.max_downloads,
        "expiresAt": share.expires_at,
    })))
}

/// Проверяет пароль один раз и выпускает тикет скачивания.
///
/// Пароль передаётся только в теле запроса. Здесь же атомарно учитывается
/// скачивание, поэтому `Range`-запросы по тикету не увеличивают счётчик и не
/// позволяют обойти `max_downloads`.
async fn public_share_unlock(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    body: Result<axum::Json<UnlockRequest>, JsonRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let token = path
        .map(|Path(token)| token)
        .map_err(|_| ApiError::not_found("Ссылка не найдена."))?;
    let ip = client_ip(&headers, Some(peer.ip()), state.config.server.trust_proxy);

    // Периодическая очистка, чтобы карты лимитеров не росли бесконечно.
    state.share_unlock.prune();
    state.share_passwords.prune();

    if let Err(retry_after) = state.share_unlock.try_acquire(&ip) {
        return Err(ApiError::rate_limited(retry_after));
    }

    let password = body
        .map(|axum::Json(value)| value.password)
        .unwrap_or_default();

    let share = shares::resolve_share_for_unlock(&state.db, &token).await?;
    require_share_session(&state, &share, &headers).await?;

    let password_key = format!("{}|{}", share.share_id, ip);
    if let Some(hash) = share.password_hash.as_deref() {
        if let Err(retry_after) = state.share_passwords.check(&password_key) {
            return Err(ApiError::rate_limited(retry_after));
        }
        let valid = !password.is_empty() && shares::verify_share_password(hash, &password).await;
        if !valid {
            state.share_passwords.record_failure(&password_key);
            // В журнал попадают только идентификатор ссылки и IP, без пароля.
            record(
                &state.db,
                AuditEvent {
                    event_type: "cloud.share.password_failed",
                    actor_id: None,
                    actor_username: "share",
                    target_type: "cloud_share",
                    target_id: &share.share_id.to_string(),
                    result: "failure",
                    ip: &ip,
                    details: json!({ "shareId": share.share_id }),
                },
            )
            .await;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "SHARE_PASSWORD_INVALID",
                "Неверный пароль. Проверьте пароль и повторите попытку.",
            ));
        }
        state.share_passwords.clear(&password_key);
    }

    if !shares::register_download_atomic(&state.db, share.share_id).await? {
        return Err(ApiError::new(
            StatusCode::GONE,
            "DOWNLOAD_LIMIT_REACHED",
            "Лимит скачиваний по этой ссылке исчерпан.",
        ));
    }

    let (ticket, expires_at) = tickets::issue_share_ticket(
        &state.config.security.secret,
        share.share_id,
        share.password_hash.as_deref(),
    );
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.share.unlocked",
            actor_id: None,
            actor_username: "share",
            target_type: "cloud_share",
            target_id: &share.share_id.to_string(),
            result: "success",
            ip: &ip,
            details: json!({ "shareId": share.share_id, "hasPassword": share.has_password }),
        },
    )
    .await;

    Ok(ApiOk(json!({
        "ticket": ticket,
        "expiresAt": expires_at,
    })))
}

async fn public_share_download(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    Query(query): Query<PublicDownloadQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let token = path
        .map(|Path(token)| token)
        .map_err(|_| ApiError::not_found("Ссылка не найдена."))?;
    let ip = client_ip(&headers, Some(peer.ip()), state.config.server.trust_proxy);
    let share = shares::resolve_share_for_unlock(&state.db, &token).await?;
    require_share_session(&state, &share, &headers).await?;

    if let Some(ticket) = query.ticket.as_deref().filter(|value| !value.is_empty()) {
        let verified = tickets::verify_share_ticket(
            &state.config.security.secret,
            ticket,
            share.password_hash.as_deref(),
        )
        .filter(|ticket| ticket.share_id == share.share_id);
        if verified.is_none() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "SHARE_TICKET_INVALID",
                "Срок действия разрешения на скачивание истёк. Откройте ссылку заново.",
            ));
        }
        // Тикет уже учёл скачивание: Range не увеличивает счётчик.
        return files::stream_file(&state.db, share.owner_id, share.entry_id, &headers, false)
            .await;
    }

    // Совместимый путь без тикета (прямые ссылки и старые клиенты).
    // Счётчик увеличивается атомарно до отдачи файла, поэтому Range не обходит лимит.
    if let Some(hash) = share.password_hash.as_deref() {
        let password = headers
            .get("x-share-password")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if password.is_empty() || !shares::verify_share_password(hash, password).await {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "SHARE_PASSWORD_REQUIRED",
                "Для скачивания требуется пароль.",
            ));
        }
    }

    if !shares::register_download_atomic(&state.db, share.share_id).await? {
        return Err(ApiError::new(
            StatusCode::GONE,
            "DOWNLOAD_LIMIT_REACHED",
            "Лимит скачиваний по этой ссылке исчерпан.",
        ));
    }

    let response =
        files::stream_file(&state.db, share.owner_id, share.entry_id, &headers, false).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.share.downloaded",
            actor_id: None,
            actor_username: "share",
            target_type: "cloud_entry",
            target_id: &share.entry_id.to_string(),
            result: "success",
            ip: &ip,
            details: json!({ "shareId": share.share_id }),
        },
    )
    .await;
    Ok(response)
}
