//! API Minecraft Build Manager.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::builds::{self, CreateBuildRequest, ListQuery, UpdateBuildRequest};
use super::files::{self, ListFilesQuery, UpdateFileRequest};
use super::releases::{self, CreateReleaseRequest};
use super::uploads::{self, CreateUploadRequest};
use super::{checks, launcher_manifest, manifests, metadata, validation};
use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const VIEW: &str = "minecraft.view";
const CREATE: &str = "minecraft.build.create";
const EDIT: &str = "minecraft.build.edit";
const PUBLISH: &str = "minecraft.build.publish";
const DELETE: &str = "minecraft.build.delete";
const UPLOAD: &str = "minecraft.artifacts.upload";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/stats", get(stats))
        .route("/builds", get(list_builds).post(create_build))
        .route(
            "/builds/{id}",
            get(get_build).patch(update_build).delete(delete_build),
        )
        .route("/builds/{id}/clone", post(clone_build))
        .route("/builds/{id}/archive", post(archive_build))
        .route("/builds/{id}/unpublish", post(unpublish_build))
        .route("/builds/{id}/restore", post(restore_build))
        .route(
            "/builds/{id}/icon",
            axum::routing::put(upload_icon)
                .delete(delete_icon)
                .get(get_icon),
        )
        .route("/builds/{id}/validate", post(validate_build))
        .route(
            "/builds/{id}/files",
            get(list_files).delete(delete_all_files),
        )
        .route("/builds/{id}/files/folder", post(create_folder))
        .route(
            "/builds/{id}/files/{file_id}",
            patch(update_file).delete(delete_file),
        )
        .route("/builds/{id}/files/{file_id}/replace", post(replace_file))
        .route(
            "/builds/{id}/releases",
            get(list_releases).post(create_release),
        )
        .route("/builds/{id}/releases/{release_id}", get(get_release))
        .route(
            "/builds/{id}/releases/{release_id}/publish",
            post(publish_release),
        )
        .route(
            "/builds/{id}/releases/{release_id}/activate",
            post(activate_release),
        )
        .route(
            "/builds/{id}/releases/{release_id}/deprecate",
            post(deprecate_release),
        )
        .route(
            "/builds/{id}/releases/{release_id}/archive",
            post(archive_release),
        )
        .route(
            "/builds/{id}/releases/{release_id}/manifest",
            get(release_manifest),
        )
        .route(
            "/builds/{id}/releases/{release_id}/launcher-manifest",
            get(launcher_release_manifest),
        )
        .route("/builds/{id}/releases/compare", get(compare_releases))
        .route("/builds/{id}/history", get(build_history))
        .route("/uploads", post(create_upload))
        .route(
            "/uploads/{id}",
            get(upload_status).patch(upload_chunk).delete(abort_upload),
        )
        .route("/uploads/{id}/complete", post(complete_upload))
        .route("/uploads/config", get(upload_config))
        .route("/metadata/minecraft", get(minecraft_versions))
        .route("/metadata/fabric", get(fabric_versions))
        .route("/metadata/forge", get(forge_versions))
        .route("/metadata/neoforge", get(neoforge_versions))
        .route("/metadata/quilt", get(quilt_versions))
        .route("/metadata/loaders", get(loader_versions))
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

async fn stats(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let stats = builds::stats(&state.db).await?;
    Ok(ApiOk(json!(stats)))
}

async fn list_builds(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let query = parse_query(query)?;
    let (items, total) = builds::list(&state.db, &query).await?;
    Ok(ApiOk(json!({
        "items": items,
        "total": total,
        "page": query.page.unwrap_or(1).max(1),
        "perPage": query.per_page.unwrap_or(24).clamp(1, 100),
    })))
}

async fn create_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<CreateBuildRequest>, JsonRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(CREATE)?;
    let request = parse_json(body)?;
    let name = request.name.clone();
    let build = builds::create(&state.db, actor.id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &build.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": name, "slug": build.slug }),
        },
    )
    .await;
    Ok(ApiOk(build))
}

async fn get_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    let build = builds::load(&state.db, id).await?;
    let stats = files::stats(&state.db, id).await?;
    let releases = releases::list(&state.db, id).await?;
    Ok(ApiOk(json!({
        "build": build,
        "files": { "files": stats.files, "folders": stats.folders, "totalBytes": stats.total_bytes },
        "releases": releases,
    })))
}

async fn update_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<axum::Json<UpdateBuildRequest>, JsonRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    let request = parse_json(body)?;
    let build = builds::update(&state.db, id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": build.name }),
        },
    )
    .await;
    Ok(ApiOk(build))
}

async fn delete_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(DELETE)?;
    let id = parse_id(path)?;
    let build = builds::load(&state.db, id).await?;
    builds::delete(&state.db, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.deleted",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "name": build.name, "slug": build.slug }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "deleted": true })))
}

async fn clone_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(CREATE)?;
    let id = parse_id(path)?;
    let build = builds::clone_build(&state.db, actor.id, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.cloned",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &build.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "sourceId": id, "slug": build.slug }),
        },
    )
    .await;
    Ok(ApiOk(build))
}

async fn archive_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    let build = builds::set_archived(&state.db, id, true).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.archived",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "slug": build.slug }),
        },
    )
    .await;
    Ok(ApiOk(build))
}

async fn unpublish_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(PUBLISH)?;
    let id = parse_id(path)?;
    let build = releases::unpublish_build(&state.db, actor.id, &actor.username, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.unpublished",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "slug": build.slug }),
        },
    )
    .await;
    state
        .realtime
        .publish_system("build.unpublished", json!({ "buildId": id }));
    Ok(ApiOk(build))
}

async fn restore_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<builds::BuildRow>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    let build = builds::set_archived(&state.db, id, false).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.restored",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "slug": build.slug }),
        },
    )
    .await;
    Ok(ApiOk(build))
}

async fn upload_icon(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: axum::body::Body,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    builds::load(&state.db, id).await?;
    use http_body_util::BodyExt;
    let collected = body
        .collect()
        .await
        .map_err(|_| ApiError::validation("Не удалось прочитать тело запроса."))?;
    let bytes = collected.to_bytes();
    if bytes.len() as i64 > builds::ICON_MAX_BYTES {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "FILE_TOO_LARGE",
            "Иконка не должна превышать 2 МБ.",
        )
        .with_details(json!({ "maxBytes": builds::ICON_MAX_BYTES })));
    }
    builds::set_icon(&state, id, actor.id, bytes.to_vec()).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.build.icon_updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "bytes": bytes.len() }),
        },
    )
    .await;
    Ok(ApiOk(
        json!({ "iconUrl": format!("/api/v1/minecraft/builds/{id}/icon") }),
    ))
}

async fn delete_icon(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    builds::remove_icon(&state.db, id).await?;
    Ok(ApiOk(json!({ "deleted": true })))
}

async fn get_icon(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let id = parse_id(path)?;
    let Some(icon) = builds::icon_object(&state.db, id).await? else {
        return Err(ApiError::not_found("Иконка не задана."));
    };
    let mut response =
        crate::storage::http::stream_object(&icon.location, icon.size, &headers, icon.content_type)
            .await?;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=60"),
    );
    Ok(response)
}

async fn validate_build(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    builds::load(&state.db, id).await?;
    let report = checks::validate(&state.db, id).await?;
    state.realtime.publish_system(
        "build.validation.completed",
        json!({
            "buildId": id,
            "valid": report.valid,
            "errors": report.errors.len(),
            "warnings": report.warnings.len(),
        }),
    );
    Ok(ApiOk(checks::report_json(&report)))
}

async fn list_files(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ListFilesQuery>, QueryRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    let query = parse_query(query)?;
    builds::load(&state.db, id).await?;
    let items = files::list(&state.db, id, &query).await?;
    let stats = files::stats(&state.db, id).await?;
    Ok(ApiOk(json!({
        "items": items,
        "stats": {
            "files": stats.files,
            "folders": stats.folders,
            "totalBytes": stats.total_bytes,
        },
    })))
}

async fn delete_all_files(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    builds::load(&state.db, id).await?;
    let removed: i64 = sqlx::query("delete from minecraft_build_files where build_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map(|result| result.rows_affected() as i64)
        .unwrap_or(0);
    crate::storage::objects::delete_zero_ref_objects(&state.db)
        .await
        .ok();
    Ok(ApiOk(json!({ "removed": removed })))
}

async fn create_folder(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<axum::Json<CreateFolderBody>, JsonRejection>,
) -> ApiResult<ApiOk<files::FileRow>> {
    actor.require(EDIT)?;
    let id = parse_id(path)?;
    let body = parse_json(body)?;
    builds::load(&state.db, id).await?;
    let row = files::create_folder(&state.db, id, &body.path).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.folder.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "path": row.path, "category": row.category }),
        },
    )
    .await;
    Ok(ApiOk(row))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFolderBody {
    path: String,
}

async fn update_file(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    body: Result<axum::Json<UpdateFileRequest>, JsonRejection>,
) -> ApiResult<ApiOk<files::FileRow>> {
    actor.require(EDIT)?;
    let Path((build_id, file_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let request = parse_json(body)?;
    let row = files::update_file(&state.db, build_id, file_id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.artifact.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &build_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({
                "fileId": file_id,
                "path": row.path,
                "category": row.category,
                "side": row.side,
                "required": row.required,
                "enabled": row.enabled,
            }),
        },
    )
    .await;
    Ok(ApiOk(row))
}

async fn delete_file(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(EDIT)?;
    let Path((build_id, file_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let removed = files::delete_file(&state.db, build_id, file_id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.artifact.deleted",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &build_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "fileId": file_id, "removed": removed }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "removed": removed })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaceFileBody {
    /// Новая сессия загрузки, созданная с тем же относительным путём.
    upload_id: Uuid,
}

/// Заменяет содержимое существующего файла сборки.
///
/// До завершения замены старая запись и объект не меняются: новый объект
/// фиксируется отдельно, а переключение `object_id` происходит одной транзакцией.
async fn replace_file(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    body: Result<axum::Json<ReplaceFileBody>, JsonRejection>,
) -> ApiResult<ApiOk<files::FileRow>> {
    actor.require(UPLOAD)?;
    let Path((build_id, file_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let body = parse_json(body)?;
    let target = files::load_file(&state.db, build_id, file_id).await?;
    let file =
        uploads::complete_upload_into(&state, body.upload_id, actor.id, &target.path).await?;
    if file.id != file_id {
        // Загрузка пришла для другого пути — отменяем, чтобы не создать дубль.
        return Err(ApiError::conflict(
            "UPLOAD_PATH_MISMATCH",
            "Загрузка не соответствует заменяемому файлу.",
        ));
    }
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.artifact.replaced",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_build",
            target_id: &build_id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "path": file.path, "size": file.size_bytes, "sha256": file.sha256 }),
        },
    )
    .await;
    Ok(ApiOk(file))
}

async fn list_releases(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Vec<releases::ReleaseRow>>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    Ok(ApiOk(releases::list(&state.db, id).await?))
}

async fn create_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<axum::Json<CreateReleaseRequest>, JsonRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(PUBLISH)?;
    let id = parse_id(path)?;
    let request = parse_json(body)?;
    let release = releases::create(&state.db, actor.id, id, request).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.release.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_release",
            target_id: &release.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "buildId": id, "version": release.version }),
        },
    )
    .await;
    Ok(ApiOk(release))
}

async fn get_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(VIEW)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    Ok(ApiOk(
        releases::load(&state.db, build_id, release_id).await?,
    ))
}

async fn publish_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
    body: Result<axum::Json<PublishBody>, JsonRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(PUBLISH)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let body = parse_json(body)?;
    let release = match releases::publish(
        &state.db,
        actor.id,
        &actor.username,
        build_id,
        release_id,
        body.accept_warnings,
    )
    .await
    {
        Ok(release) => release,
        Err(error) => {
            state.realtime.publish_system(
                "build.publish.failed",
                json!({
                    "buildId": build_id,
                    "releaseId": release_id,
                    "code": error.code,
                    "message": error.message,
                }),
            );
            return Err(error);
        }
    };
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.release.published",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_release",
            target_id: &release.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "buildId": build_id, "version": release.version }),
        },
    )
    .await;
    state.realtime.publish_system(
        "build.published",
        json!({
            "buildId": build_id,
            "releaseId": release.id,
            "version": release.version,
        }),
    );
    Ok(ApiOk(release))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishBody {
    #[serde(default)]
    accept_warnings: bool,
}

async fn activate_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(PUBLISH)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let release = releases::set_current(&state.db, actor.id, build_id, release_id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "minecraft.release.activated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "minecraft_release",
            target_id: &release.id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "buildId": build_id, "version": release.version }),
        },
    )
    .await;
    Ok(ApiOk(release))
}

async fn deprecate_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(PUBLISH)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let release = releases::set_status(
        &state.db,
        actor.id,
        build_id,
        release_id,
        "deprecated",
        "release.deprecated",
    )
    .await?;
    Ok(ApiOk(release))
}

async fn archive_release(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<releases::ReleaseRow>> {
    actor.require(PUBLISH)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let release = releases::set_status(
        &state.db,
        actor.id,
        build_id,
        release_id,
        "archived",
        "release.archived",
    )
    .await?;
    Ok(ApiOk(release))
}

async fn release_manifest(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    let release = releases::load(&state.db, build_id, release_id).await?;
    let manifest: Option<serde_json::Value> =
        sqlx::query_scalar("select manifest from minecraft_releases where id = $1")
            .bind(release.id)
            .fetch_one(&state.db)
            .await?;
    let manifest = match manifest.filter(|value| value.get("schema_version").is_some()) {
        Some(value) => value,
        None => manifests::build_manifest(&state.db, build_id, release_id).await?,
    };
    Ok(ApiOk(manifest))
}

/// Точный JSON, который получит MyLauncher, для панели и страницы релиза.
async fn launcher_release_manifest(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let Path((build_id, release_id)) =
        path.map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    Ok(ApiOk(
        launcher_manifest::snapshot(&state.db, build_id, release_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompareQuery {
    left: Uuid,
    right: Uuid,
}

async fn compare_releases(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<CompareQuery>, QueryRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let build_id = parse_id(path)?;
    let query = parse_query(query)?;
    Ok(ApiOk(
        releases::compare(&state.db, build_id, query.left, query.right).await?,
    ))
}

async fn build_history(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Vec<releases::HistoryRow>>> {
    actor.require(VIEW)?;
    let id = parse_id(path)?;
    Ok(ApiOk(releases::history(&state.db, id, 100).await?))
}

async fn upload_config(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(UPLOAD)?;
    Ok(ApiOk(uploads::session_config(&state).await))
}

async fn create_upload(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<CreateUploadBody>, JsonRejection>,
) -> ApiResult<ApiOk<uploads::UploadSession>> {
    actor.require(UPLOAD)?;
    let body = parse_json(body)?;
    builds::load(&state.db, body.build_id).await?;
    let request = CreateUploadRequest {
        path: body.path,
        size: body.size,
        category: body.category,
        side: body.side,
        required: body.required,
        description: body.description,
        content_type: body.content_type,
    };
    let session = if body.mode.as_deref() == Some("replace") {
        uploads::create_replace_upload(&state, actor.id, body.build_id, request).await?
    } else {
        uploads::create_upload(&state, actor.id, body.build_id, request).await?
    };
    Ok(ApiOk(session))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadBody {
    build_id: Uuid,
    path: String,
    size: i64,
    /// `replace` — заменить содержимое существующего файла с тем же путём.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_content_type")]
    content_type: String,
}

fn default_content_type() -> String {
    "application/octet-stream".to_owned()
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
    body: Body,
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
) -> ApiResult<Response> {
    actor.require(UPLOAD)?;
    let id = parse_id(path)?;
    let request_hash = crate::core::idempotency::hash_request(&json!({ "uploadId": id }));

    crate::core::idempotency::run_idempotent(
        &state,
        actor.id,
        "minecraft.upload.complete",
        &headers,
        request_hash,
        async {
            let file = uploads::complete_upload(&state, id, actor.id).await?;
            record(
                &state.db,
                AuditEvent {
                    event_type: "minecraft.artifact.uploaded",
                    actor_id: Some(actor.id),
                    actor_username: &actor.username,
                    target_type: "minecraft_build",
                    target_id: &file.build_id.to_string(),
                    result: "success",
                    ip: &actor.ip,
                    details: json!({ "path": file.path, "size": file.size_bytes }),
                },
            )
            .await;
            Ok(serde_json::to_value(&file).unwrap_or(serde_json::Value::Null))
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

async fn minecraft_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(metadata::minecraft_versions(&state).await?))
}

async fn fabric_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(metadata::fabric_versions(&state).await?))
}

async fn forge_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(metadata::forge_versions(&state).await?))
}

async fn neoforge_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(metadata::neoforge_versions(&state).await?))
}

async fn quilt_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(metadata::quilt_versions(&state).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoaderQuery {
    loader: String,
    minecraft: String,
}

async fn loader_versions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<LoaderQuery>, QueryRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    actor.require(VIEW)?;
    let query = parse_query(query)?;
    let loader = validation::validate_loader(&query.loader)?;
    let minecraft = validation::validate_minecraft_version(&query.minecraft)?;
    Ok(ApiOk(
        metadata::loaders_for(&state.db, &loader, &minecraft).await?,
    ))
}

/// Отдаёт артефакт сборки (используется формой предпросмотра админки).
pub async fn artifact_response(
    state: &Arc<AppState>,
    entry_id: Uuid,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let row: Option<(String, uuid::Uuid, String, i64)> = sqlx::query_as(
        "select o.object_key, v.id, v.root_path, f.size_bytes
         from minecraft_build_files f
         join storage_objects o on o.id = f.object_id
         join storage_volumes v on v.id = o.volume_id
         where f.id = $1",
    )
    .bind(entry_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((key, volume_id, root_path, size)) = row else {
        return Err(ApiError::not_found("Артефакт не найден."));
    };
    let root = std::path::PathBuf::from(&root_path);
    let location = crate::storage::engine::ObjectLocation {
        volume_id,
        absolute: crate::storage::engine::ObjectLocation::objects_root(&root).join(&key),
        key,
        root,
    };
    let mut response =
        crate::storage::http::stream_object(&location, size, headers, "application/octet-stream")
            .await?;
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("attachment"),
    );
    Ok(response.into_response())
}
