use nexus_core::{Clock, NexusError, TaskId};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{
    now_ms, path_key, validate_name, validate_relative_path, validate_sha1, validate_sha256,
};
use crate::db::DbError;

const TASK_STATUSES: &[&str] = &[
    "queued",
    "running",
    "paused",
    "completed",
    "failed",
    "cancelled",
];
const FILE_STATUSES: &[&str] = &[
    "queued",
    "running",
    "paused",
    "completed",
    "failed",
    "skipped",
];

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub job_id: Option<String>,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub priority: i64,
    pub total_files: i64,
    pub completed_files: i64,
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
}

fn default_priority() -> i64 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub id: String,
    pub task_id: String,
    pub url: String,
    pub root_kind: String,
    pub rel_path: String,
    pub path_key: String,
    pub size_bytes: i64,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub required: bool,
    pub status: String,
    pub bytes_done: i64,
    pub attempts: i64,
    pub etag: Option<String>,
    pub last_http_status: Option<i64>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewFile {
    pub url: String,
    pub root_kind: String,
    pub rel_path: String,
    pub size_bytes: i64,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

pub async fn create_task(
    pool: &SqlitePool,
    clock: &dyn Clock,
    new: NewTask,
) -> Result<TaskRow, NexusError> {
    let kinds = [
        "minecraft",
        "java",
        "loader",
        "mod",
        "modpack",
        "nexus",
        "server",
        "custom",
    ];
    if !kinds.contains(&new.kind.as_str()) {
        return Err(NexusError::invalid_argument(
            "unsupported download task kind",
        ));
    }
    let label = validate_name(&new.label, "label", 256)?;
    if !(-1_000..=10_000).contains(&new.priority) {
        return Err(NexusError::invalid_argument(
            "priority must be between -1000 and 10000",
        ));
    }

    let id = TaskId::new().to_string();
    let now = now_ms(clock);
    sqlx::query(
        "INSERT INTO download_tasks
            (id, job_id, kind, label, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'queued', ?, ?, ?)",
    )
    .bind(&id)
    .bind(&new.job_id)
    .bind(&new.kind)
    .bind(&label)
    .bind(new.priority)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "create download task"))?;

    get_task(pool, &id)
        .await?
        .ok_or_else(|| NexusError::not_found("download task was not created"))
}

pub async fn get_task(pool: &SqlitePool, id: &str) -> Result<Option<TaskRow>, NexusError> {
    sqlx::query_as::<_, TaskRow>(
        "SELECT id, job_id, kind, label, status, priority, total_files,
                completed_files, total_bytes, done_bytes, error_code, created_at,
                started_at, finished_at, updated_at
         FROM download_tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list_tasks(
    pool: &SqlitePool,
    status: Option<&str>,
) -> Result<Vec<TaskRow>, NexusError> {
    if let Some(status) = status
        && !TASK_STATUSES.contains(&status)
    {
        return Err(NexusError::invalid_argument("unsupported task status"));
    }
    sqlx::query_as::<_, TaskRow>(
        "SELECT id, job_id, kind, label, status, priority, total_files,
                completed_files, total_bytes, done_bytes, error_code, created_at,
                started_at, finished_at, updated_at
         FROM download_tasks
         WHERE (? IS NULL OR status = ?)
         ORDER BY priority ASC, created_at ASC",
    )
    .bind(status)
    .bind(status)
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn set_task_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    if !TASK_STATUSES.contains(&status) {
        return Err(NexusError::invalid_argument("unsupported task status"));
    }
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE download_tasks
         SET status = ?,
             updated_at = ?,
             started_at = CASE WHEN ? = 'running'
                               THEN COALESCE(started_at, ?)
                               ELSE started_at END,
             finished_at = CASE WHEN ? IN ('completed', 'failed', 'cancelled')
                                THEN ? ELSE finished_at END
         WHERE id = ?",
    )
    .bind(status)
    .bind(now)
    .bind(status)
    .bind(now)
    .bind(status)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

pub async fn add_file(
    pool: &SqlitePool,
    clock: &dyn Clock,
    task_id: &str,
    new: NewFile,
) -> Result<FileRow, NexusError> {
    if !new.url.starts_with("https://") && !new.url.starts_with("http://") {
        return Err(NexusError::invalid_argument(
            "download url must use http:// or https://",
        ));
    }
    if new.url.len() > 4096 || new.url.chars().any(char::is_control) {
        return Err(NexusError::invalid_argument("download url is invalid"));
    }
    let roots = [
        "shared_assets",
        "shared_libraries",
        "shared_versions",
        "instance",
        "server",
        "java",
        "cache",
        "downloads",
    ];
    if !roots.contains(&new.root_kind.as_str()) {
        return Err(NexusError::invalid_argument("unsupported root_kind"));
    }
    if new.size_bytes < 0 {
        return Err(NexusError::invalid_argument("size_bytes must be >= 0"));
    }
    let rel_path = validate_relative_path(&new.rel_path)?;
    validate_sha1(new.sha1.as_deref(), "sha1")?;
    validate_sha256(new.sha256.as_deref(), "sha256")?;

    let id = nexus_core::FileId::new().to_string();
    let now = now_ms(clock);

    sqlx::query(
        "INSERT INTO download_files
            (id, task_id, url, root_kind, rel_path, path_key, size_bytes, sha1, sha256,
             required, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?)",
    )
    .bind(&id)
    .bind(task_id)
    .bind(&new.url)
    .bind(&new.root_kind)
    .bind(&rel_path)
    .bind(path_key(&rel_path))
    .bind(new.size_bytes)
    .bind(&new.sha1)
    .bind(&new.sha256)
    .bind(i64::from(new.required))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "add download file"))?;

    recalculate_totals(pool, task_id, clock).await?;

    get_file(pool, &id)
        .await?
        .ok_or_else(|| NexusError::not_found("download file was not created"))
}

pub async fn get_file(pool: &SqlitePool, id: &str) -> Result<Option<FileRow>, NexusError> {
    sqlx::query_as::<_, FileRow>(
        "SELECT id, task_id, url, root_kind, rel_path, path_key, size_bytes,
                sha1, sha256, required, status, bytes_done, attempts, etag,
                last_http_status, error_code, created_at, updated_at
         FROM download_files WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list_files(pool: &SqlitePool, task_id: &str) -> Result<Vec<FileRow>, NexusError> {
    sqlx::query_as::<_, FileRow>(
        "SELECT id, task_id, url, root_kind, rel_path, path_key, size_bytes,
                sha1, sha256, required, status, bytes_done, attempts, etag,
                last_http_status, error_code, created_at, updated_at
         FROM download_files
         WHERE task_id = ?
         ORDER BY rel_path",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn set_file_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    bytes_done: i64,
    error_code: Option<&str>,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    if !FILE_STATUSES.contains(&status) {
        return Err(NexusError::invalid_argument("unsupported file status"));
    }
    if bytes_done < 0 {
        return Err(NexusError::invalid_argument("bytes_done must be >= 0"));
    }
    let result = sqlx::query(
        "UPDATE download_files
         SET status = ?, bytes_done = ?, error_code = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(status)
    .bind(bytes_done)
    .bind(error_code)
    .bind(now_ms(clock))
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

async fn recalculate_totals(
    pool: &SqlitePool,
    task_id: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    sqlx::query(
        "UPDATE download_tasks
         SET total_files = (SELECT COUNT(*) FROM download_files WHERE task_id = ?),
             total_bytes = (SELECT COALESCE(SUM(size_bytes), 0) FROM download_files WHERE task_id = ?),
             done_bytes = (SELECT COALESCE(SUM(bytes_done), 0) FROM download_files WHERE task_id = ?),
             updated_at = ?
         WHERE id = ?",
    )
    .bind(task_id)
    .bind(task_id)
    .bind(task_id)
    .bind(now_ms(clock))
    .bind(task_id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(())
}

pub async fn count_running(pool: &SqlitePool) -> Result<i64, NexusError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM download_tasks WHERE status = 'running'")
        .fetch_one(pool)
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}
