//! Возобновляемые загрузки Cloud Storage.
//!
//! Собственная реализация с семантикой tus 1.0.0:
//! * `POST /cloud/uploads` — создание сессии (name, parentId, size, contentType);
//! * `GET /cloud/uploads/{id}` — текущее смещение;
//! * `PATCH /cloud/uploads/{id}` + `Upload-Offset` — запись чанка на диск;
//! * `POST /cloud/uploads/{id}/complete` — фиксация объекта и создание записи файла;
//! * `DELETE /cloud/uploads/{id}` — отмена и очистка staging.
//!
//! Чанки пишутся сразу на диск в staging-файл выбранного тома, в память не буферизуются.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::engine::{self, ObjectLocation, VolumeRecord};
use crate::storage::mime;

use super::models::CloudEntry;
use super::service::{self, add_usage, entry_name_taken};

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UploadSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub volume_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub file_name: String,
    pub content_type: String,
    pub declared_size: Option<i64>,
    pub received_bytes: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

const SESSION_SELECT: &str = "select id, user_id, volume_id, parent_id, file_name, content_type,
        declared_size, received_bytes, status, created_at, expires_at
     from upload_sessions";

async fn load_session(db: &PgPool, id: Uuid, user_id: Uuid) -> ApiResult<UploadSession> {
    sqlx::query_as::<_, UploadSession>(&format!("{SESSION_SELECT} where id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .filter(|session| session.user_id == user_id)
        .ok_or_else(|| ApiError::not_found("Загрузка не найдена."))
}

fn upload_busy() -> ApiError {
    ApiError::conflict(
        "UPLOAD_BUSY",
        "Другая часть загрузки уже записывается. Повторите запрос через секунду.",
    )
    .with_header("retry-after", 1)
}

/// После неудачной попытки убирает блокировку, если сессия больше не активна.
async fn finish_upload_attempt<T>(state: &AppState, id: Uuid, result: &ApiResult<T>) {
    if result.is_err() {
        let status: Option<String> =
            sqlx::query_scalar("select status from upload_sessions where id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        if status.as_deref() != Some("active") {
            state.release_upload_lock(id);
        }
    }
}

/// Условно сохраняет `received_bytes`. Если значение в БД уже другое
/// (второй экземпляр или ручное вмешательство), staging усекается до
/// подтверждённого смещения.
async fn persist_received(
    state: &AppState,
    session_id: Uuid,
    volume_root: &std::path::Path,
    received: i64,
    expected: i64,
) -> ApiResult<()> {
    let updated = sqlx::query(
        "update upload_sessions
            set received_bytes = $2, updated_at = now()
          where id = $1 and received_bytes = $3",
    )
    .bind(session_id)
    .bind(received)
    .bind(expected)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(());
    }

    let confirmed: i64 =
        sqlx::query_scalar("select received_bytes from upload_sessions where id = $1")
            .bind(session_id)
            .fetch_optional(&state.db)
            .await?
            .unwrap_or(0)
            .max(0);
    let _ = engine::truncate_staging(volume_root, session_id, confirmed as u64).await;
    Err(ApiError::conflict(
        "UPLOAD_OFFSET_MISMATCH",
        "Смещение загрузки изменилось. Продолжите с подтверждённого смещения.",
    )
    .with_details(json!({ "offset": confirmed })))
}

/// Удаляет просроченные сессии и их staging-файлы.
///
/// Истечение TTL — завершение загрузки, поэтому владелец получает
/// `upload.failed` с безопасным кодом и без внутренних путей.
pub async fn cleanup_expired(state: &AppState) -> ApiResult<usize> {
    let rows: Vec<(Uuid, Uuid, String, String, Option<i64>, i64)> = sqlx::query_as(
        "select s.id, s.user_id, v.root_path, s.file_name, s.declared_size, s.received_bytes
         from upload_sessions s
         join storage_volumes v on v.id = s.volume_id
         where s.status = 'active' and s.expires_at < now()",
    )
    .fetch_all(&state.db)
    .await?;
    let mut removed = 0;
    for (session_id, user_id, root_path, file_name, declared_size, received_bytes) in rows {
        let path = engine::staging_path(&std::path::PathBuf::from(&root_path), session_id);
        let _ = tokio::fs::remove_file(&path).await;
        sqlx::query(
            "update upload_sessions set status = 'expired', updated_at = now() where id = $1",
        )
        .bind(session_id)
        .execute(&state.db)
        .await?;
        state.realtime.publish_upload_failed(
            user_id,
            session_id,
            Some(&file_name),
            "UPLOAD_EXPIRED",
            "Срок хранения сессии загрузки истёк. Начните загрузку заново.",
            received_bytes,
            declared_size,
        );
        state.release_upload_lock(session_id);
        removed += 1;
    }
    Ok(removed)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUploadRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub size: i64,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

fn default_content_type() -> String {
    "application/octet-stream".to_owned()
}

pub async fn create_upload(
    state: &AppState,
    user_id: Uuid,
    request: CreateUploadRequest,
) -> ApiResult<UploadSession> {
    let name = service::validate_name(&request.name)?;
    if request.size < 0 {
        return Err(ApiError::validation(
            "Размер файла не может быть отрицательным.",
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

    cleanup_expired(state).await?;

    if let Some(parent) = request.parent_id {
        let parent_entry = service::load_entry(&state.db, user_id, parent).await?;
        if parent_entry.entry_type != "folder" || parent_entry.trashed_at.is_some() {
            return Err(ApiError::validation(
                "Загрузка возможна только в существующую папку вне корзины.",
            ));
        }
    }
    service::ensure_path_budget(&state.db, user_id, request.parent_id, &name).await?;

    // Квота: если имя уже занято файлом, прирост равен разнице размеров.
    let existing = service::entry_name_taken(&state.db, user_id, request.parent_id, &name).await?;
    let delta = match existing.as_ref().filter(|entry| entry.entry_type == "file") {
        Some(entry) => request.size - entry.size_bytes,
        None => request.size,
    };
    super::quota::ensure_capacity(&state.db, user_id, super::quota::Scope::Cloud, delta).await?;

    let volume = engine::pick_volume(
        &state.db,
        None,
        request.size,
        engine::VolumeScope::Cloud,
        Some(user_id),
    )
    .await?;
    let ttl_hours = state.upload_ttl_hours().await;

    let session = sqlx::query_as::<_, UploadSession>(
        "insert into upload_sessions
           (user_id, volume_id, parent_id, file_name, content_type,
            declared_size, temp_key, status, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'active', now() + make_interval(hours => $8))
         returning id, user_id, volume_id, parent_id, file_name, content_type,
                   declared_size, received_bytes, status, created_at, expires_at",
    )
    .bind(user_id)
    .bind(volume.id)
    .bind(request.parent_id)
    .bind(&name)
    .bind(&request.content_type)
    .bind(request.size)
    .bind(Uuid::new_v4().to_string())
    .bind(ttl_hours as i32)
    .fetch_one(&state.db)
    .await?;
    Ok(session)
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

/// Записывает чанк на диск с проверкой смещения.
///
/// Одновременные `PATCH` одной сессии сериализуются: второй запрос получает
/// `409 UPLOAD_BUSY`, а не портит staging-файл.
pub async fn append_chunk(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    offset: i64,
    data: &[u8],
) -> ApiResult<i64> {
    let lock = state.upload_lock(id);
    let guard = lock.try_lock().map_err(|_| upload_busy())?;
    let result = append_chunk_locked(state, id, user_id, offset, data).await;
    drop(guard);
    finish_upload_attempt(state, id, &result).await;
    result
}

async fn append_chunk_locked(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    offset: i64,
    data: &[u8],
) -> ApiResult<i64> {
    let chunk_max_bytes = state.chunk_max_bytes().await;
    if data.len() as u64 > chunk_max_bytes {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "CHUNK_TOO_LARGE",
            "Чанк превышает допустимый размер.",
        )
        .with_details(json!({
            "limitBytes": chunk_max_bytes,
            "receivedBytes": data.len(),
        })));
    }
    let session = session_offset(&state.db, id, user_id).await?;
    if offset != session.received_bytes {
        return Err(ApiError::conflict(
            "UPLOAD_OFFSET_MISMATCH",
            "Смещение загрузки не совпадает с данными на сервере.",
        )
        .with_details(json!({ "offset": session.received_bytes })));
    }
    if session
        .declared_size
        .map(|declared| offset + data.len() as i64 > declared)
        .unwrap_or(false)
    {
        return Err(ApiError::validation(
            "Чанк выходит за заявленный размер файла.",
        ));
    }
    let volume = engine::load_volume(&state.db, session.volume_id).await?;
    let root = std::path::PathBuf::from(&volume.root_path);
    let result = engine::append_chunk(&root, session.id, offset, data).await?;
    persist_received(state, session.id, &root, result.received_bytes, offset).await?;
    Ok(result.received_bytes)
}

/// Записывает тело запроса потоком в staging-файл: в память попадает только текущий кадр.
/// Возвращает новое смещение.
pub async fn append_stream(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    offset: i64,
    body: axum::body::Body,
) -> ApiResult<i64> {
    let lock = state.upload_lock(id);
    let guard = lock.try_lock().map_err(|_| upload_busy())?;
    let result = append_stream_locked(state, id, user_id, offset, body).await;
    drop(guard);
    finish_upload_attempt(state, id, &result).await;
    result
}

async fn append_stream_locked(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    offset: i64,
    body: axum::body::Body,
) -> ApiResult<i64> {
    use http_body_util::BodyExt;
    use std::io::SeekFrom;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

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
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .await
    {
        Ok(file) => file,
        Err(_) => {
            let error = ApiError::internal("не удалось открыть staging-файл");
            return Err(fail_active_session(state, &session, error).await);
        }
    };
    // Клиент мог оборвать предыдущий PATCH: сервер не успел обновить
    // received_bytes, но часть тела уже лежит в staging. Приводим файл к
    // подтверждённому offset, иначе продолжение загрузки допишет байты
    // поверх «хвоста» и complete не сойдётся по размеру.
    match file.metadata().await {
        Ok(metadata) if metadata.len() > offset as u64 => {
            if file.set_len(offset as u64).await.is_err() {
                drop(file);
                let error = ApiError::internal("не удалось выровнять staging по offset");
                return Err(fail_active_session(state, &session, error).await);
            }
        }
        Ok(metadata) if metadata.len() < offset as u64 => {
            drop(file);
            let error = ApiError::conflict(
                "UPLOAD_DATA_MISSING",
                "Данные загрузки потеряны. Начните загрузку заново.",
            );
            return Err(fail_active_session(state, &session, error).await);
        }
        Ok(_) => {}
        Err(_) => {
            drop(file);
            let error = ApiError::internal("не удалось проверить staging-файл");
            return Err(fail_active_session(state, &session, error).await);
        }
    }
    if file.seek(SeekFrom::Start(offset as u64)).await.is_err() {
        drop(file);
        let error = ApiError::internal("не удалось перейти к offset staging-файла");
        return Err(fail_active_session(state, &session, error).await);
    }

    let limit = state.chunk_max_bytes().await;
    let declared = session.declared_size;
    let mut written: i64 = 0;
    let mut received_total: i64 = 0;
    let mut body = body;
    let mut over_limit = false;

    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|_| ApiError::validation("Не удалось прочитать чанк загрузки."))?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        received_total += data.len() as i64;
        if written >= limit as i64 {
            over_limit = true;
            break;
        }
        let remaining = (limit as i64 - written) as usize;
        // Данных в кадре больше, чем разрешено лимитом: чанк превышает допустимый размер.
        if data.len() > remaining {
            over_limit = true;
        }
        let slice = &data[..data.len().min(remaining)];
        if file.write_all(slice).await.is_err() {
            drop(file);
            let error = ApiError::internal("не удалось записать чанк загрузки");
            return Err(fail_active_session(state, &session, error).await);
        }
        written += slice.len() as i64;
        if over_limit || written > declared.unwrap_or(i64::MAX) - offset {
            break;
        }
    }

    if file.sync_all().await.is_err() {
        drop(file);
        let error = ApiError::internal("не удалось подтвердить запись чанка");
        return Err(fail_active_session(state, &session, error).await);
    }
    drop(file);

    let received = offset + written;
    if let Some(declared) = declared {
        if received > declared {
            sqlx::query(
                "update upload_sessions set received_bytes = $2, updated_at = now() where id = $1",
            )
            .bind(session.id)
            .bind(declared)
            .execute(&state.db)
            .await?;
            return Err(ApiError::validation(
                "Получено больше байтов, чем заявлено для файла.",
            ));
        }
    }
    persist_received(state, session.id, &root, received, offset).await?;

    state.realtime.publish_user(
        "upload.progress",
        user_id,
        json!({
            "uploadId": session.id,
            "uploaded": received,
            "size": declared,
        }),
    );

    if over_limit {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "CHUNK_TOO_LARGE",
            "Чанк превышает допустимый размер. Продолжите загрузку меньшими частями.",
        )
        .with_details(json!({ "limitBytes": limit, "receivedBytes": received_total })));
    }

    Ok(received)
}

/// Закрывает активную сессию после ошибки записи и уведомляет владельца.
///
/// Событие `upload.failed` содержит только код, безопасное сообщение и
/// прогресс; технические детали остаются в журнале.
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
        "update upload_sessions set status = 'aborted', updated_at = now() where id = $1",
    )
    .bind(session.id)
    .execute(&state.db)
    .await;
    state.realtime.publish_upload_failed(
        session.user_id,
        session.id,
        Some(&session.file_name),
        &error.code,
        &error.message,
        session.received_bytes,
        session.declared_size,
    );
    error
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadRequest {
    /// `auto` — ошибка при конфликте имени, `replace` — заменить существующий файл,
    /// `rename` — использовать `newName`.
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub new_name: Option<String>,
}

fn default_strategy() -> String {
    "auto".to_owned()
}

/// Фиксирует загрузку: staging → объект → запись файла (или замена содержимого).
///
/// Любая ошибка завершения закрывает операцию и публикует владельцу
/// `upload.failed` с кодом и безопасным сообщением.
pub async fn complete_upload(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    request: CompleteUploadRequest,
) -> ApiResult<CloudEntry> {
    let result = complete_upload_guarded(state, id, user_id, request).await;
    state.release_upload_lock(id);
    result
}

async fn complete_upload_guarded(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    request: CompleteUploadRequest,
) -> ApiResult<CloudEntry> {
    match complete_upload_inner(state, id, user_id, request).await {
        Ok(entry) => Ok(entry),
        Err(error) => {
            let session = load_session(&state.db, id, user_id).await.ok();
            let (name, uploaded, size) = match &session {
                Some(session) => (
                    Some(session.file_name.as_str()),
                    session.received_bytes,
                    session.declared_size,
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

/// Фиксирует загрузку: staging → объект → запись файла (или замена содержимого).
pub async fn complete_upload_inner(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    request: CompleteUploadRequest,
) -> ApiResult<CloudEntry> {
    let session = load_session(&state.db, id, user_id).await?;
    if session.status != "active" {
        return Err(ApiError::conflict(
            "UPLOAD_NOT_ACTIVE",
            "Загрузка уже завершена или отменена.",
        ));
    }
    if !matches!(request.strategy.as_str(), "auto" | "replace" | "rename") {
        return Err(ApiError::validation(
            "Неизвестная стратегия завершения загрузки.",
        ));
    }
    let volume: VolumeRecord = engine::load_volume(&state.db, session.volume_id).await?;
    if !volume.enabled || volume.state != "online" {
        return Err(ApiError::conflict(
            "VOLUME_UNAVAILABLE",
            "Хранилище недоступно. Завершение загрузки отложено до проверки тома.",
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
    if let Some(declared) = session.declared_size {
        if actual != declared {
            return Err(ApiError::conflict(
                "UPLOAD_INCOMPLETE",
                "Загрузка не завершена: размер на диске не совпадает с заявленным.",
            )
            .with_details(json!({ "received": actual, "declared": declared })));
        }
    }

    let target_name = match request.strategy.as_str() {
        "rename" => service::validate_name(request.new_name.as_deref().unwrap_or_default())?,
        _ => session.file_name.clone(),
    };
    service::ensure_path_budget(&state.db, user_id, session.parent_id, &target_name).await?;
    let existing = entry_name_taken(&state.db, user_id, session.parent_id, &target_name).await?;

    if let Some(found) = &existing {
        match request.strategy.as_str() {
            "replace" => {
                if found.entry_type != "file" {
                    return Err(ApiError::conflict(
                        "FILE_NAME_CONFLICT",
                        "Запись с таким именем является папкой. Выберите другое имя.",
                    )
                    .with_details(json!({ "existingFileId": found.id })));
                }
            }
            _ => {
                return Err(ApiError::conflict(
                    "FILE_NAME_CONFLICT",
                    "Файл с таким именем уже существует в этой папке.",
                )
                .with_details(json!({ "existingFileId": found.id })));
            }
        }
    }

    // Тип определяется по содержимому, а не по заголовку клиента: подмена
    // Content-Type или расширения не должна менять тип хранения и предпросмотра.
    let content_type = mime::sniff_file(&staging).await?;

    let location =
        engine::commit_from_staging(&state.db, &volume, &staging, actual, content_type).await?;
    let object_id = object_id_for(&location, &state.db).await?;

    let result = persist_completion(
        state,
        user_id,
        &session,
        &existing,
        &target_name,
        object_id,
        actual,
        content_type,
    )
    .await;

    let entry_id = match result {
        Ok(id) => id,
        Err(error) => {
            // Новая запись не сохранена — освобождаем созданный объект, чтобы не терять место.
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
                "update upload_sessions set status = 'aborted', updated_at = now() where id = $1",
            )
            .bind(session.id)
            .execute(&state.db)
            .await
            .ok();
            return Err(error);
        }
    };

    sqlx::query(
        "update upload_sessions set status = 'completed', updated_at = now() where id = $1",
    )
    .bind(session.id)
    .execute(&state.db)
    .await?;
    add_usage(
        &state.db,
        user_id,
        actual - existing.as_ref().map(|entry| entry.size_bytes).unwrap_or(0),
        if existing.is_none() { 1 } else { 0 },
    )
    .await?;

    // Объект, заменённый при `strategy=replace`, освобождается в транзакции;
    // физически он удаляется только когда на него больше нет ссылок.
    if existing.is_some() {
        service::delete_zero_ref_objects(&state.db).await.ok();
    }

    service::load_entry(&state.db, user_id, entry_id).await
}

/// Сохраняет запись файла в транзакции. Возвращает id записи.
#[allow(clippy::too_many_arguments)]
async fn persist_completion(
    state: &AppState,
    user_id: Uuid,
    session: &UploadSession,
    existing: &Option<CloudEntry>,
    name: &str,
    object_id: Uuid,
    size: i64,
    content_type: &str,
) -> ApiResult<Uuid> {
    let mut tx = state.db.begin().await?;

    let entry_id = match existing {
        Some(found) => {
            let old_object: Option<Uuid> =
                sqlx::query_scalar("select object_id from cloud_entries where id = $1")
                    .bind(found.id)
                    .fetch_one(&mut *tx)
                    .await?;

            sqlx::query(
                "update cloud_entries
                 set name = $2, object_id = $3, size_bytes = $4,
                     content_type = $5, updated_at = now()
                 where id = $1",
            )
            .bind(found.id)
            .bind(name)
            .bind(object_id)
            .bind(size)
            .bind(content_type)
            .execute(&mut *tx)
            .await?;

            if let Some(old) = old_object {
                if old != object_id {
                    release_ref_in_tx(&mut tx, found.id, old).await?;
                }
            }
            found.id
        }
        None => {
            let id: Uuid = sqlx::query_scalar(
                "insert into cloud_entries
                   (owner_id, parent_id, name, entry_type, object_id, size_bytes, content_type)
                 values ($1, $2, $3, 'file', $4, $5, $6)
                 returning id",
            )
            .bind(user_id)
            .bind(session.parent_id)
            .bind(name)
            .bind(object_id)
            .bind(size)
            .bind(content_type)
            .fetch_one(&mut *tx)
            .await
            .map_err(service::name_conflict_from_sqlx)?;
            id
        }
    };

    // Счётчик объекта уже равен 1 после commit_from_staging; фиксируем владельца.
    sqlx::query(
        "insert into storage_object_refs (object_id, owner_module, owner_id, display_name)
         values ($1, 'cloud', $2, $3)
         on conflict (owner_module, owner_id) do update
         set object_id = excluded.object_id, display_name = excluded.display_name",
    )
    .bind(object_id)
    .bind(entry_id)
    .bind(name)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(entry_id)
}

async fn release_ref_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entry_id: Uuid,
    object_id: Uuid,
) -> ApiResult<()> {
    sqlx::query("delete from storage_object_refs where owner_module = 'cloud' and owner_id = $1")
        .bind(entry_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("update storage_objects set ref_count = greatest(ref_count - 1, 0) where id = $1")
        .bind(object_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn object_id_for(location: &ObjectLocation, db: &PgPool) -> ApiResult<Uuid> {
    sqlx::query_scalar("select id from storage_objects where volume_id = $1 and object_key = $2")
        .bind(location.volume_id)
        .bind(&location.key)
        .fetch_one(db)
        .await
        .map_err(ApiError::from)
}

pub async fn abort_upload(state: &AppState, id: Uuid, user_id: Uuid) -> ApiResult<()> {
    let session = load_session(&state.db, id, user_id).await?;
    if session.status != "active" {
        state.release_upload_lock(id);
        return Ok(());
    }
    let volume = engine::load_volume(&state.db, session.volume_id).await?;
    let staging = engine::staging_path(&std::path::PathBuf::from(&volume.root_path), session.id);
    let _ = tokio::fs::remove_file(&staging).await;
    sqlx::query("update upload_sessions set status = 'aborted', updated_at = now() where id = $1")
        .bind(session.id)
        .execute(&state.db)
        .await?;
    state.release_upload_lock(id);
    state.realtime.publish_upload_failed(
        user_id,
        session.id,
        Some(&session.file_name),
        "UPLOAD_ABORTED",
        "Загрузка отменена.",
        session.received_bytes,
        session.declared_size,
    );
    Ok(())
}

pub async fn session_config(state: &AppState) -> serde_json::Value {
    let max_bytes = state.max_upload_bytes().await;
    let chunk_max_bytes = state.chunk_max_bytes().await;
    let upload_ttl_hours = state.upload_ttl_hours().await;
    json!({
        "maxUploadBytes": max_bytes,
        "chunkMaxBytes": chunk_max_bytes,
        "uploadTtlHours": upload_ttl_hours,
        "uploadConcurrency": state.upload_concurrency().await,
        "shareTtlHours": state.share_ttl_hours().await,
        "exportTtlHours": state.export_ttl_hours().await,
        "trashRetentionDays": state.trash_retention_days().await,
        "serverTime": Utc::now(),
        "maxUploadMb": max_bytes / (1024 * 1024),
    })
}

/// Удаляет staging-файл сессии, если он остался после ошибки.
pub fn remove_staging(root: &std::path::Path, session_id: Uuid) {
    let path = engine::staging_path(root, session_id);
    let _ = std::fs::remove_file(path);
}

/// Очищает staging-каталог тома от файлов без активной сессии (при старте).
pub async fn cleanup_orphans(db: &PgPool) -> ApiResult<usize> {
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("select v.id, v.root_path from storage_volumes v where v.state = 'online'")
            .fetch_all(db)
            .await?;
    let mut removed = 0;
    for (_id, root_path) in rows {
        let staging = std::path::PathBuf::from(&root_path)
            .join(crate::storage::paths::NEXUS_DIR)
            .join("staging");
        let Ok(mut dir) = tokio::fs::read_dir(&staging).await else {
            continue;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(stem) = name.strip_suffix(".part") else {
                continue;
            };
            let Ok(session_id) = Uuid::parse_str(stem) else {
                continue;
            };
            let active: Option<Uuid> = sqlx::query_scalar(
                "select id from upload_sessions where id = $1 and status = 'active'",
            )
            .bind(session_id)
            .fetch_optional(db)
            .await?;
            if active.is_none() {
                let _ = tokio::fs::remove_file(entry.path()).await;
                removed += 1;
            }
        }
    }
    Ok(removed)
}
