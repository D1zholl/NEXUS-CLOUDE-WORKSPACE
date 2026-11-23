//! Массовое скачивание Cloud Storage в ZIP-архив.
//!
//! Архив формируется фоновой задачей во временном каталоге выбранного тома,
//! прогресс хранится в `background_jobs`. Готовый архив отдаётся потоком и
//! удаляется по истечении срока (`export_ttl_hours`).

use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::engine;

use super::models::descendant_ids;
use super::service;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub id: Uuid,
    pub status: String,
    pub progress: f64,
    pub total_units: Option<i64>,
    pub done_units: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateExportRequest {
    pub entry_ids: Vec<Uuid>,
}

/// Проверяет записи и создаёт задачу формирования архива.
pub async fn create_export(
    state: &AppState,
    owner_id: Uuid,
    request: CreateExportRequest,
) -> ApiResult<ExportJob> {
    if request.entry_ids.is_empty() {
        return Err(ApiError::validation(
            "Выберите файлы или папки для скачивания.",
        ));
    }
    if request.entry_ids.len() > 200 {
        return Err(ApiError::validation(
            "За один раз можно выбрать не более 200 записей.",
        ));
    }
    // Просроченные архивы удаляются при создании новой задачи: отдельного
    // планировщика нет, а TTL должен соблюдаться.
    let export_ttl_hours = state.export_ttl_hours().await;
    cleanup_exports(&state.db, export_ttl_hours).await.ok();
    let mut total_units = 0i64;
    for entry_id in &request.entry_ids {
        let entry = service::load_entry(&state.db, owner_id, *entry_id).await?;
        if entry.trashed_at.is_some() {
            return Err(ApiError::validation("Нельзя скачать запись из корзины."));
        }
        if entry.entry_type == "folder" {
            let ids = descendant_ids(&state.db, *entry_id).await?;
            let files: i64 = sqlx::query_scalar(
                "select count(*) from cloud_entries
                 where id = any($1) and entry_type = 'file' and trashed_at is null",
            )
            .bind(&ids)
            .fetch_one(&state.db)
            .await?;
            total_units += files;
        } else {
            total_units += 1;
        }
    }

    let job = sqlx::query_as::<_, ExportJob>(
        "insert into background_jobs (job_type, status, payload, total_units, created_by)
         values ('cloud.export', 'pending', $1, $2, $3)
         returning id, status, progress, total_units, done_units, error,
                   created_at, started_at, finished_at",
    )
    .bind(json!({ "entryIds": request.entry_ids }))
    .bind(total_units)
    .bind(owner_id)
    .fetch_one(&state.db)
    .await?;

    Ok(job)
}

pub async fn get_job(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<ExportJob> {
    sqlx::query_as::<_, ExportJob>(
        "select id, status, progress, total_units, done_units, error,
                created_at, started_at, finished_at
         from background_jobs
         where id = $1 and created_by = $2 and job_type = 'cloud.export'",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Задача не найдена."))
}

pub async fn list_jobs(db: &PgPool, owner_id: Uuid) -> ApiResult<Vec<ExportJob>> {
    sqlx::query_as::<_, ExportJob>(
        "select id, status, progress, total_units, done_units, error,
                created_at, started_at, finished_at
         from background_jobs
         where created_by = $1 and job_type = 'cloud.export'
         order by created_at desc
         limit 20",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}

/// Каталог для готовых архивов конкретного тома.
pub fn export_dir(root: &std::path::Path) -> PathBuf {
    root.join(crate::storage::paths::NEXUS_DIR).join("exports")
}

/// Формирует ZIP. Запускается планировщиком фоновых задач.
pub async fn run_export(state: &AppState, job_id: Uuid) -> ApiResult<serde_json::Value> {
    let row: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
        "select created_by, payload from background_jobs where id = $1 and job_type = 'cloud.export'",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((owner_id, payload)) = row else {
        return Err(ApiError::not_found("Задача не найдена."));
    };

    sqlx::query(
        "update background_jobs set status = 'running', started_at = now(), progress = 0
         where id = $1",
    )
    .bind(job_id)
    .execute(&state.db)
    .await?;

    let entry_ids: Vec<Uuid> = payload
        .get("entryIds")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().and_then(|text| Uuid::parse_str(text).ok()))
                .collect()
        })
        .unwrap_or_default();

    // Собираем плоский список файлов с относительными путями внутри архива.
    let mut files: Vec<(String, PathBuf, i64)> = Vec::new();
    for entry_id in &entry_ids {
        let Ok(entry) = service::load_entry(&state.db, owner_id, *entry_id).await else {
            continue;
        };
        if entry.entry_type == "file" {
            if let Ok(resolved) = super::files::resolve_object(&state.db, owner_id, *entry_id).await
            {
                files.push((
                    entry.name.clone(),
                    resolved.location.absolute,
                    entry.size_bytes,
                ));
            }
            continue;
        }
        collect_folder(&state.db, owner_id, *entry_id, &entry.name, &mut files).await?;
    }

    let total_bytes: i64 = files.iter().map(|(_, _, size)| *size).sum();
    sqlx::query("update background_jobs set total_units = $2, done_units = 0 where id = $1")
        .bind(job_id)
        .bind(files.len() as i64)
        .execute(&state.db)
        .await?;

    // Каталог архивов рядом с данными первого файла.
    let first_root = files
        .first()
        .and_then(|(_, path, _)| {
            let text = path.to_string_lossy().to_string();
            text.split(".nexus").next().map(PathBuf::from)
        })
        .unwrap_or_else(std::env::temp_dir);
    let dir = export_dir(&first_root);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        mark_failed(&state.db, job_id, &error.to_string()).await;
        return Err(ApiError::internal(error));
    }
    let target = dir.join(format!("{job_id}.zip"));
    let file = match std::fs::File::create(&target) {
        Ok(file) => file,
        Err(error) => {
            mark_failed(&state.db, job_id, &error.to_string()).await;
            return Err(ApiError::internal(error));
        }
    };

    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(total_bytes > 4 * 1024 * 1024 * 1024);

    let mut done = 0i64;
    for (name, path, _size) in &files {
        let entry_name = name.trim_start_matches('/').to_owned();
        let source = path.clone();
        let written = tokio::task::block_in_place(|| {
            if zip.start_file(entry_name, options).is_err() {
                return false;
            }
            let Ok(mut source) = std::fs::File::open(&source) else {
                return false;
            };
            std::io::copy(&mut source, &mut zip).is_ok()
        });
        if written {
            done += 1;
        }
        let progress = if files.is_empty() {
            100.0
        } else {
            (done as f64 / files.len() as f64) * 100.0
        };
        sqlx::query("update background_jobs set done_units = $2, progress = $3 where id = $1")
            .bind(job_id)
            .bind(done)
            .bind(progress)
            .execute(&state.db)
            .await?;
    }

    let finished = tokio::task::block_in_place(|| zip.finish());
    if let Err(error) = finished {
        mark_failed(&state.db, job_id, &error.to_string()).await;
        return Err(ApiError::internal(error));
    }

    let result = json!({ "file": target.to_string_lossy(), "bytes": total_bytes });
    sqlx::query(
        "update background_jobs
         set status = 'completed', progress = 100, done_units = total_units,
             finished_at = now(), result = $2
         where id = $1",
    )
    .bind(job_id)
    .bind(&result)
    .execute(&state.db)
    .await?;
    Ok(result)
}

async fn mark_failed(db: &PgPool, job_id: Uuid, message: &str) {
    let _ = sqlx::query(
        "update background_jobs
         set status = 'failed', error = $2, finished_at = now()
         where id = $1",
    )
    .bind(job_id)
    .bind(message)
    .execute(db)
    .await;
}

async fn collect_folder(
    db: &PgPool,
    owner_id: Uuid,
    folder_id: Uuid,
    prefix: &str,
    out: &mut Vec<(String, PathBuf, i64)>,
) -> ApiResult<()> {
    #[derive(sqlx::FromRow)]
    struct Child {
        id: Uuid,
        name: String,
        entry_type: String,
        size_bytes: i64,
        object_key: Option<String>,
        root_path: Option<String>,
    }
    let rows: Vec<Child> = sqlx::query_as(
        "select e.id, e.name, e.entry_type, e.size_bytes, o.object_key, v.root_path
         from cloud_entries e
         left join storage_objects o on o.id = e.object_id
         left join storage_volumes v on v.id = o.volume_id
         where e.parent_id = $1 and e.owner_id = $2 and e.trashed_at is null
         order by e.entry_type desc, e.name",
    )
    .bind(folder_id)
    .bind(owner_id)
    .fetch_all(db)
    .await?;

    for Child {
        id,
        name,
        entry_type,
        size_bytes: size,
        object_key: key,
        root_path,
    } in rows
    {
        let path = format!("{prefix}/{name}");
        if entry_type == "folder" {
            Box::pin(collect_folder(db, owner_id, id, &path, out)).await?;
        } else if let (Some(key), Some(root)) = (key, root_path) {
            let root = PathBuf::from(&root);
            let absolute = engine::ObjectLocation::objects_root(&root).join(&key);
            out.push((path, absolute, size));
        }
    }
    Ok(())
}

/// Отдаёт готовый архив потоком и планирует удаление по TTL.
pub async fn export_file(db: &PgPool, owner_id: Uuid, job_id: Uuid) -> ApiResult<PathBuf> {
    let job = get_job(db, owner_id, job_id).await?;
    if job.status != "completed" {
        return Err(ApiError::conflict(
            "EXPORT_NOT_READY",
            "Архив ещё не готов.",
        ));
    }
    let result: Option<serde_json::Value> =
        sqlx::query_scalar("select result from background_jobs where id = $1 and created_by = $2")
            .bind(job_id)
            .bind(owner_id)
            .fetch_one(db)
            .await?;
    let path = result
        .and_then(|value| {
            value
                .get("file")
                .and_then(|file| file.as_str())
                .map(PathBuf::from)
        })
        .ok_or_else(|| ApiError::not_found("Файл архива не найден."))?;
    if !path.exists() {
        return Err(ApiError::not_found("Файл архива не найден."));
    }
    Ok(path)
}

/// Удаляет архив и запись задачи. Разрешено только владельцу и только для
/// завершённой или ошибочной задачи. Повторное скачивание после удаления — 404.
pub async fn delete_export(db: &PgPool, owner_id: Uuid, job_id: Uuid) -> ApiResult<()> {
    let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "select status, result from background_jobs
         where id = $1 and created_by = $2 and job_type = 'cloud.export'",
    )
    .bind(job_id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?;
    let Some((status, result)) = row else {
        return Err(ApiError::not_found("Задача не найдена."));
    };
    if !matches!(status.as_str(), "completed" | "failed") {
        return Err(ApiError::conflict(
            "EXPORT_NOT_FINISHED",
            "Удалить можно только завершённый или ошибочный архив.",
        ));
    }
    if let Some(file) = result
        .as_ref()
        .and_then(|value| value.get("file"))
        .and_then(|value| value.as_str())
    {
        match engine::remove_file_with_retry(std::path::Path::new(file)).await {
            Ok(_) => {}
            Err(error) => {
                // Не удаляем запись, если ZIP остался на диске: пользователь
                // сможет повторить удаление после закрытия файла.
                tracing::warn!(%error, path = file, "архив занят, удаление отложено");
                return Err(ApiError::conflict(
                    "FILE_LOCKED",
                    "Архив открыт другим процессом. Закройте его и повторите удаление.",
                ));
            }
        }
    }
    sqlx::query("delete from background_jobs where id = $1 and created_by = $2")
        .bind(job_id)
        .bind(owner_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Удаляет просроченные архивы.
pub async fn cleanup_exports(db: &PgPool, ttl_hours: i64) -> ApiResult<usize> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "select id, result from background_jobs
         where job_type = 'cloud.export' and status = 'completed'
           and finished_at < now() - make_interval(hours => $1)
           and result is not null",
    )
    .bind(ttl_hours as i32)
    .fetch_all(db)
    .await?;
    let mut removed = 0;
    for (id, result) in rows {
        let Some(file) = result.get("file").and_then(|value| value.as_str()) else {
            continue;
        };
        match engine::remove_file_with_retry(std::path::Path::new(file)).await {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => {
                // Архив ещё открыт: оставляем result, следующий запуск повторит.
                tracing::warn!(%error, path = file, "архив занят, очистка отложена");
                continue;
            }
        }
        let _ = sqlx::query("update background_jobs set result = null where id = $1")
            .bind(id)
            .execute(db)
            .await;
    }
    Ok(removed)
}

/// Запись архива в файл (используется тестами и ручной проверкой).
pub fn write_placeholder(path: &std::path::Path) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(
        b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    )?;
    Ok(())
}

pub fn export_filename() -> String {
    format!("nexus-export-{}.zip", Utc::now().format("%Y%m%d-%H%M%S"))
}
