//! Загрузка файлов сборки: сессия → потоковые чанки → фиксация объекта.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{files, validation};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::engine;
use crate::storage::mime;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UploadSession {
    pub id: Uuid,
    pub build_id: Uuid,
    pub user_id: Uuid,
    pub volume_id: Uuid,
    pub target_path: String,
    pub category: String,
    pub side: String,
    pub required: bool,
    pub description: String,
    pub content_type: String,
    pub declared_size: i64,
    pub received_bytes: i64,
    pub status: String,
    pub replace_existing: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

const SESSION_SELECT: &str = "select id, build_id, user_id, volume_id, target_path, category,
        side, required, description, content_type, declared_size, received_bytes, status,
        replace_existing, created_at, expires_at
     from minecraft_upload_sessions";

async fn load_session(db: &PgPool, id: Uuid, user_id: Uuid) -> ApiResult<UploadSession> {
    sqlx::query_as::<_, UploadSession>(&format!("{SESSION_SELECT} where id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .filter(|session| session.user_id == user_id)
        .ok_or_else(|| ApiError::not_found("Загрузка не найдена."))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUploadRequest {
    pub path: String,
    pub size: i64,
    #[serde(default = "default_category")]
    pub category: Option<String>,
    #[serde(default = "default_side")]
    pub side: Option<String>,
    #[serde(default = "default_required")]
    pub required: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

fn default_category() -> Option<String> {
    None
}
fn default_side() -> Option<String> {
    None
}
fn default_required() -> Option<bool> {
    Some(true)
}
fn default_content_type() -> String {
    "application/octet-stream".to_owned()
}

pub async fn create_upload(
    state: &AppState,
    user_id: Uuid,
    build_id: Uuid,
    request: CreateUploadRequest,
) -> ApiResult<UploadSession> {
    create_upload_inner(state, user_id, build_id, request, false).await
}

/// Создаёт сессию загрузки для замены содержимого существующего файла.
/// Путь при этом должен уже принадлежать файлу сборки.
pub async fn create_replace_upload(
    state: &AppState,
    user_id: Uuid,
    build_id: Uuid,
    request: CreateUploadRequest,
) -> ApiResult<UploadSession> {
    create_upload_inner(state, user_id, build_id, request, true).await
}

async fn create_upload_inner(
    state: &AppState,
    user_id: Uuid,
    build_id: Uuid,
    request: CreateUploadRequest,
    replace: bool,
) -> ApiResult<UploadSession> {
    if request.size <= 0 {
        return Err(ApiError::validation(
            "Размер файла должен быть больше нуля.",
        ));
    }
    let max_upload_bytes = state.max_upload_bytes().await;
    if request.size as u64 > max_upload_bytes {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "FILE_TOO_LARGE",
            "Файл превышает максимально допустимый размер.",
        )
        .with_details(json!({ "maxBytes": max_upload_bytes })));
    }

    let path = if replace {
        let normalized = validation::check_build_path(&request.path, "file")?;
        existing_size_for_path(&state.db, build_id, &normalized).await?;
        normalized
    } else {
        files::path_available(&state.db, build_id, &request.path).await?
    };

    // Квота Minecraft: замена файла с тем же путём учитывает только прирост.
    let existing_size: Option<i64> = sqlx::query_scalar(
        "select size_bytes from minecraft_build_files
         where build_id = $1 and lower(path) = lower($2) and kind = 'file'",
    )
    .bind(build_id)
    .bind(&path)
    .fetch_optional(&state.db)
    .await?;
    let delta = match existing_size {
        Some(size) => request.size - size,
        None => request.size,
    };
    crate::storage::quota::ensure_capacity(
        &state.db,
        user_id,
        crate::storage::quota::Scope::Minecraft,
        delta,
    )
    .await?;

    let category = validation::resolve_category(&path, request.category.as_deref())?;
    let side = match request.side.as_deref() {
        Some(value) if !value.trim().is_empty() => validation::validate_side(value)?,
        _ => "client".to_owned(),
    };

    cleanup_expired(state).await?;

    let volume = engine::pick_volume(
        &state.db,
        None,
        request.size,
        engine::VolumeScope::Minecraft,
        Some(user_id),
    )
    .await?;

    let ttl = state.upload_ttl_hours().await as i32;
    let session = sqlx::query_as::<_, UploadSession>(
        "insert into minecraft_upload_sessions
           (build_id, user_id, volume_id, target_path, category, side, required,
            description, content_type, declared_size, status, replace_existing, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'active', $11,
                 now() + make_interval(hours => $12))
         returning id, build_id, user_id, volume_id, target_path, category, side, required,
                   description, content_type, declared_size, received_bytes, status,
                   replace_existing, created_at, expires_at",
    )
    .bind(build_id)
    .bind(user_id)
    .bind(volume.id)
    .bind(&path)
    .bind(&category)
    .bind(&side)
    .bind(request.required.unwrap_or(true))
    .bind(request.description.as_deref().unwrap_or("").trim())
    .bind(&request.content_type)
    .bind(request.size)
    .bind(replace)
    .bind(ttl)
    .fetch_one(&state.db)
    .await?;

    Ok(session)
}

/// Проверяет, что путь уже принадлежит файлу сборки (для замены).
async fn existing_size_for_path(db: &PgPool, build_id: Uuid, path: &str) -> ApiResult<Option<i64>> {
    let row: Option<(Uuid, String, i64)> = sqlx::query_as(
        "select id, kind, size_bytes from minecraft_build_files
         where build_id = $1 and lower(path) = lower($2)",
    )
    .bind(build_id)
    .bind(path)
    .fetch_optional(db)
    .await?;
    match row {
        Some((_, kind, size)) if kind == "file" => Ok(Some(size)),
        Some(_) => Err(ApiError::conflict(
            "BUILD_PATH_IS_FOLDER",
            "По этому пути находится папка, а не файл.",
        )),
        None => Err(ApiError::not_found("Файл для замены не найден.")),
    }
}

pub async fn session_offset(db: &PgPool, id: Uuid, user_id: Uuid) -> ApiResult<UploadSession> {
    let session = load_session(db, id, user_id).await?;
    if session.status != "active" {
        return Err(ApiError::conflict(
            "UPLOAD_NOT_ACTIVE",
            "Загрузка уже завершена или отменена.",
        ));
    }
    Ok(session)
}

/// Пишет тело запроса в staging-файл тома, соблюдая лимиты.
pub async fn append_stream(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    offset: i64,
    body: axum::body::Body,
) -> ApiResult<i64> {
    use http_body_util::BodyExt;
    use tokio::io::AsyncWriteExt;

    let session = session_offset(&state.db, id, user_id).await?;
    if offset != session.received_bytes {
        return Err(ApiError::conflict(
            "UPLOAD_OFFSET_MISMATCH",
            "Смещение загрузки не совпадает с данными на сервере.",
        )
        .with_details(json!({ "offset": session.received_bytes })));
    }

    let volume = engine::load_volume(&state.db, session.volume_id).await?;
    let root = std::path::PathBuf::from(&volume.root_path);
    let path = engine::staging_path(&root, session.id);
    if let Some(parent) = path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_err() {
            let error = ApiError::internal("не удалось подготовить staging");
            return Err(fail_active_session(state, &session, error).await);
        }
    }
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(file) => file,
        Err(_) => {
            let error = ApiError::internal("не удалось открыть staging-файл");
            return Err(fail_active_session(state, &session, error).await);
        }
    };

    let limit = state.chunk_max_bytes().await;
    let remaining_total = session.declared_size - offset;
    let mut written: i64 = 0;
    let mut received: i64 = 0;
    let mut received_body = body;
    let mut over_limit = false;

    while let Some(frame) = received_body.frame().await {
        let frame =
            frame.map_err(|_| ApiError::validation("Не удалось прочитать чанк загрузки."))?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        received += data.len() as i64;
        if written >= limit as i64 {
            over_limit = true;
            break;
        }
        let allowed = (limit as i64 - written).min(remaining_total - written);
        if allowed <= 0 {
            over_limit = true;
            break;
        }
        // Кадр больше разрешённого остатка: чанк превышает допустимый размер.
        if data.len() as i64 > allowed {
            over_limit = true;
        }
        let slice = &data[..data.len().min(allowed as usize)];
        if file.write_all(slice).await.is_err() {
            drop(file);
            let error = ApiError::internal("не удалось записать чанк загрузки");
            return Err(fail_active_session(state, &session, error).await);
        }
        written += slice.len() as i64;
        if over_limit {
            break;
        }
    }

    if file.sync_all().await.is_err() {
        drop(file);
        let error = ApiError::internal("не удалось подтвердить запись чанка");
        return Err(fail_active_session(state, &session, error).await);
    }
    drop(file);

    sqlx::query(
        "update minecraft_upload_sessions set received_bytes = $2, updated_at = now() where id = $1",
    )
    .bind(session.id)
    .bind(offset + written)
    .execute(&state.db)
    .await?;

    if over_limit {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "CHUNK_TOO_LARGE",
            "Чанк превышает допустимый размер. Продолжите загрузку меньшими частями.",
        )
        .with_details(json!({ "limitBytes": limit, "receivedBytes": received })));
    }

    Ok(offset + written)
}

/// Закрывает активную сессию после ошибки записи и уведомляет владельца.
async fn fail_active_session(
    state: &AppState,
    session: &UploadSession,
    error: ApiError,
) -> ApiError {
    if let Ok(volume) = engine::load_volume(&state.db, session.volume_id).await {
        let staging =
            engine::staging_path(&std::path::PathBuf::from(&volume.root_path), session.id);
        let _ = tokio::fs::remove_file(&staging).await;
    }
    let _ = sqlx::query(
        "update minecraft_upload_sessions set status = 'aborted', updated_at = now() where id = $1",
    )
    .bind(session.id)
    .execute(&state.db)
    .await;
    state.realtime.publish_upload_failed(
        session.user_id,
        session.id,
        Some(&session.target_path),
        &error.code,
        &error.message,
        session.received_bytes,
        Some(session.declared_size),
    );
    error
}

/// Фиксирует объект и создаёт запись файла сборки.
pub async fn complete_upload(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
) -> ApiResult<files::FileRow> {
    finish_upload(state, id, user_id, None).await
}

/// Фиксирует загрузку, заменяя содержимое файла по указанному пути.
///
/// Путь берётся из существующей записи: старая запись и объект остаются
/// нетронутыми, пока новый объект не будет зафиксирован.
pub async fn complete_upload_into(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    target_path: &str,
) -> ApiResult<files::FileRow> {
    let normalized = validation::normalize_path(target_path)?;
    finish_upload(state, id, user_id, Some(normalized)).await
}

/// Завершает загрузку и публикует `upload.failed` при любой ошибке.
async fn finish_upload(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    replace_path: Option<String>,
) -> ApiResult<files::FileRow> {
    match complete_upload_inner(state, id, user_id, replace_path).await {
        Ok(file) => Ok(file),
        Err(error) => {
            let session = load_session(&state.db, id, user_id).await.ok();
            let (name, uploaded, size) = match &session {
                Some(session) => (
                    Some(session.target_path.as_str()),
                    session.received_bytes,
                    Some(session.declared_size),
                ),
                None => (None, 0, None),
            };
            state.realtime.publish_upload_failed(
                user_id,
                id,
                name,
                &error.code,
                &error.message,
                uploaded,
                size,
            );
            Err(error)
        }
    }
}

async fn complete_upload_inner(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    replace_path: Option<String>,
) -> ApiResult<files::FileRow> {
    let session = load_session(&state.db, id, user_id).await?;
    if session.status != "active" {
        return Err(ApiError::conflict(
            "UPLOAD_NOT_ACTIVE",
            "Загрузка уже завершена или отменена.",
        ));
    }
    let volume = engine::load_volume(&state.db, session.volume_id).await?;
    if !volume.enabled || volume.state != "online" {
        return Err(ApiError::conflict(
            "VOLUME_UNAVAILABLE",
            "Хранилище недоступно. Завершение загрузки отложено.",
        ));
    }

    let staging = engine::staging_path(&std::path::PathBuf::from(&volume.root_path), session.id);
    let actual = match tokio::fs::metadata(&staging).await {
        Ok(metadata) => metadata.len() as i64,
        Err(_) => {
            return Err(ApiError::conflict(
                "UPLOAD_DATA_MISSING",
                "Данные загрузки отсутствуют на сервере. Начните загрузку заново.",
            ));
        }
    };
    if actual != session.declared_size {
        return Err(ApiError::conflict(
            "UPLOAD_INCOMPLETE",
            "Загрузка не завершена: размер на диске не совпадает с заявленным.",
        )
        .with_details(json!({ "received": actual, "declared": session.declared_size })));
    }

    let target_path = replace_path
        .clone()
        .unwrap_or_else(|| session.target_path.clone());
    let replacing = replace_path.is_some() || session.replace_existing;
    if replacing {
        // Файл мог исчезнуть, пока шла загрузка. Замена не должна молча
        // создавать новый файл по другому пути.
        files::existing_file_size(&state.db, session.build_id, &target_path).await?;
    } else {
        // Путь мог быть занят, пока шла загрузка.
        files::path_available(&state.db, session.build_id, &target_path).await?;
    }

    // MIME определяется по первым байтам; `Content-Type` клиента не используется.
    let content_type = mime::sniff_file(&staging).await?;

    let location =
        engine::commit_from_staging(&state.db, &volume, &staging, actual, content_type).await?;
    let object_id: Uuid = sqlx::query_scalar(
        "select id from storage_objects where volume_id = $1 and object_key = $2",
    )
    .bind(location.volume_id)
    .bind(&location.key)
    .fetch_one(&state.db)
    .await?;
    let sha256: String = sqlx::query_scalar("select sha256 from storage_objects where id = $1")
        .bind(object_id)
        .fetch_one(&state.db)
        .await?;

    let file = match files::register_file(
        &state.db,
        session.build_id,
        object_id,
        &target_path,
        actual,
        &sha256,
        &session.category,
        &session.side,
        session.required,
        &session.description,
    )
    .await
    {
        Ok(file) => file,
        Err(error) => {
            // Запись не создана — освобождаем объект, чтобы не терять место.
            sqlx::query("delete from storage_object_refs where object_id = $1")
                .bind(object_id)
                .execute(&state.db)
                .await
                .ok();
            sqlx::query("delete from storage_objects where id = $1")
                .bind(object_id)
                .execute(&state.db)
                .await
                .ok();
            let _ = engine::delete_object(&location).await;
            sqlx::query(
                "update minecraft_upload_sessions set status = 'aborted', updated_at = now() where id = $1",
            )
            .bind(session.id)
            .execute(&state.db)
            .await
            .ok();
            return Err(error);
        }
    };

    sqlx::query(
        "update minecraft_upload_sessions set status = 'completed', updated_at = now() where id = $1",
    )
    .bind(session.id)
    .execute(&state.db)
    .await?;

    Ok(file)
}

pub async fn abort_upload(state: &AppState, id: Uuid, user_id: Uuid) -> ApiResult<()> {
    let session = load_session(&state.db, id, user_id).await?;
    if session.status != "active" {
        return Ok(());
    }
    let volume = engine::load_volume(&state.db, session.volume_id).await?;
    let staging = engine::staging_path(&std::path::PathBuf::from(&volume.root_path), session.id);
    let _ = tokio::fs::remove_file(&staging).await;
    sqlx::query(
        "update minecraft_upload_sessions set status = 'aborted', updated_at = now() where id = $1",
    )
    .bind(session.id)
    .execute(&state.db)
    .await?;
    state.realtime.publish_upload_failed(
        user_id,
        session.id,
        Some(&session.target_path),
        "UPLOAD_ABORTED",
        "Загрузка отменена.",
        session.received_bytes,
        Some(session.declared_size),
    );
    Ok(())
}

pub async fn cleanup_expired(state: &AppState) -> ApiResult<usize> {
    let rows: Vec<(Uuid, Uuid, String, String, i64, i64)> = sqlx::query_as(
        "select s.id, s.user_id, v.root_path, s.target_path,
                s.declared_size, s.received_bytes
         from minecraft_upload_sessions s
         join storage_volumes v on v.id = s.volume_id
         where s.status = 'active' and s.expires_at < now()",
    )
    .fetch_all(&state.db)
    .await?;
    let mut removed = 0;
    for (session_id, user_id, root_path, target_path, declared_size, received_bytes) in rows {
        let path = engine::staging_path(&std::path::PathBuf::from(&root_path), session_id);
        let _ = tokio::fs::remove_file(&path).await;
        sqlx::query(
            "update minecraft_upload_sessions set status = 'expired', updated_at = now() where id = $1",
        )
        .bind(session_id)
        .execute(&state.db)
        .await?;
        state.realtime.publish_upload_failed(
            user_id,
            session_id,
            Some(&target_path),
            "UPLOAD_EXPIRED",
            "Срок хранения сессии загрузки истёк. Начните загрузку заново.",
            received_bytes,
            Some(declared_size),
        );
        removed += 1;
    }
    Ok(removed)
}

pub async fn session_config(state: &AppState) -> serde_json::Value {
    json!({
        "chunkMaxBytes": state.chunk_max_bytes().await,
        "maxUploadBytes": state.max_upload_bytes().await,
        "uploadTtlHours": state.upload_ttl_hours().await,
        "serverTime": Utc::now(),
    })
}
