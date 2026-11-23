//! SQLite persistence for the Download Engine.
//!
//! The queue is the database: every task and every file exists in SQLite before
//! the first byte is requested. Progress is persisted on a throttle (and on
//! every pause/terminal transition), so a crash loses at most a small amount of
//! accounting; the staging sidecar remains the authoritative resume offset.

use nexus_core::{Clock, NexusError};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnection;

use crate::db::DbError;

pub(crate) const TASK_STATUSES: &[&str] = &[
    "queued",
    "running",
    "paused",
    "completed",
    "failed",
    "cancelled",
];

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub(crate) struct TaskRow {
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
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub(crate) struct FileRow {
    pub id: String,
    pub task_id: String,
    pub url: String,
    pub root_kind: String,
    pub rel_path: String,
    pub path_key: String,
    pub size_bytes: i64,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub required: i64,
    pub status: String,
    pub bytes_done: i64,
    pub attempts: i64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<i64>,
    pub staging_rel: Option<String>,
    pub resume_offset: i64,
    pub verified_at: Option<i64>,
    pub last_http_status: Option<i64>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FileRow {
    pub fn required(&self) -> bool {
        self.required != 0
    }

    pub fn size_bytes_u64(&self) -> u64 {
        u64::try_from(self.size_bytes.max(0)).unwrap_or(0)
    }

    pub fn bytes_done_u64(&self) -> u64 {
        u64::try_from(self.bytes_done.max(0)).unwrap_or(0)
    }
}

pub(crate) struct NewTaskRow<'a> {
    pub id: &'a str,
    pub job_id: Option<&'a str>,
    pub kind: &'a str,
    pub label: &'a str,
    pub priority: i64,
}

pub(crate) struct NewFileRow<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub root_kind: &'a str,
    pub rel_path: &'a str,
    pub path_key: &'a str,
    pub size_bytes: i64,
    pub sha1: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub required: bool,
}

fn map(error: sqlx::Error) -> NexusError {
    NexusError::from(DbError::from_sqlx(error))
}

async fn recalculate_task_on(
    connection: &mut SqliteConnection,
    task_id: &str,
    now: i64,
) -> Result<(), NexusError> {
    sqlx::query(
        "UPDATE download_tasks
         SET total_files = (
                 SELECT COUNT(*) FROM download_files WHERE task_id = ?
             ),
             completed_files = (
                 SELECT COUNT(*) FROM download_files
                 WHERE task_id = ? AND status IN ('completed', 'skipped')
             ),
             total_bytes = (
                 SELECT COALESCE(SUM(MAX(size_bytes, bytes_done)), 0)
                 FROM download_files WHERE task_id = ?
             ),
             done_bytes = (
                 SELECT COALESCE(SUM(bytes_done), 0)
                 FROM download_files WHERE task_id = ?
             ),
             updated_at = ?
         WHERE id = ?",
    )
    .bind(task_id)
    .bind(task_id)
    .bind(task_id)
    .bind(task_id)
    .bind(now)
    .bind(task_id)
    .execute(&mut *connection)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn create_task_with_files(
    pool: &SqlitePool,
    clock: &dyn Clock,
    task: NewTaskRow<'_>,
    files: &[NewFileRow<'_>],
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    let mut transaction = pool.begin().await.map_err(map)?;

    sqlx::query(
        "INSERT INTO download_tasks
             (id, job_id, kind, label, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'queued', ?, ?, ?)",
    )
    .bind(task.id)
    .bind(task.job_id)
    .bind(task.kind)
    .bind(task.label)
    .bind(task.priority)
    .bind(now)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;

    for file in files {
        sqlx::query(
            "INSERT INTO download_files
                 (id, task_id, url, root_kind, rel_path, path_key, size_bytes,
                  sha1, sha256, required, status, bytes_done, attempts,
                  created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', 0, 0, ?, ?)",
        )
        .bind(file.id)
        .bind(task.id)
        .bind(file.url)
        .bind(file.root_kind)
        .bind(file.rel_path)
        .bind(file.path_key)
        .bind(file.size_bytes)
        .bind(file.sha1)
        .bind(file.sha256)
        .bind(i64::from(file.required))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(map)?;
    }

    recalculate_task_on(&mut transaction, task.id, now).await?;
    transaction.commit().await.map_err(map)?;
    Ok(())
}

pub(crate) async fn get_task(pool: &SqlitePool, id: &str) -> Result<Option<TaskRow>, NexusError> {
    sqlx::query_as::<_, TaskRow>(
        "SELECT id, job_id, kind, label, status, priority, total_files,
                completed_files, total_bytes, done_bytes, error_code, error_message,
                created_at, started_at, finished_at, updated_at
         FROM download_tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(map)
}

pub(crate) async fn list_tasks(
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
                completed_files, total_bytes, done_bytes, error_code, error_message,
                created_at, started_at, finished_at, updated_at
         FROM download_tasks
         WHERE (? IS NULL OR status = ?)
         ORDER BY priority ASC, created_at ASC",
    )
    .bind(status)
    .bind(status)
    .fetch_all(pool)
    .await
    .map_err(map)
}

pub(crate) async fn list_active_tasks(pool: &SqlitePool) -> Result<Vec<TaskRow>, NexusError> {
    sqlx::query_as::<_, TaskRow>(
        "SELECT id, job_id, kind, label, status, priority, total_files,
                completed_files, total_bytes, done_bytes, error_code, error_message,
                created_at, started_at, finished_at, updated_at
         FROM download_tasks
         WHERE status IN ('queued', 'running')
         ORDER BY priority ASC, created_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(map)
}

pub(crate) async fn list_files(
    pool: &SqlitePool,
    task_id: &str,
) -> Result<Vec<FileRow>, NexusError> {
    sqlx::query_as::<_, FileRow>(
        "SELECT id, task_id, url, root_kind, rel_path, path_key, size_bytes,
                sha1, sha256, required, status, bytes_done, attempts, etag,
                last_modified, content_length, staging_rel, resume_offset,
                verified_at, last_http_status, error_code, created_at, updated_at
         FROM download_files
         WHERE task_id = ?
         ORDER BY rel_path",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .map_err(map)
}

#[allow(dead_code)]
pub(crate) async fn get_file(pool: &SqlitePool, id: &str) -> Result<Option<FileRow>, NexusError> {
    sqlx::query_as::<_, FileRow>(
        "SELECT id, task_id, url, root_kind, rel_path, path_key, size_bytes,
                sha1, sha256, required, status, bytes_done, attempts, etag,
                last_modified, content_length, staging_rel, resume_offset,
                verified_at, last_http_status, error_code, created_at, updated_at
         FROM download_files WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(map)
}

pub(crate) async fn set_task_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    if !TASK_STATUSES.contains(&status) {
        return Err(NexusError::invalid_argument("unsupported task status"));
    }
    let now = clock.now_ms();
    let result = sqlx::query(
        "UPDATE download_tasks
         SET status = ?,
             updated_at = ?,
             started_at = CASE WHEN ? = 'running'
                               THEN COALESCE(started_at, ?)
                               ELSE started_at END,
             finished_at = CASE WHEN ? IN ('completed', 'failed', 'cancelled')
                                THEN ? ELSE NULL END,
             error_code = CASE WHEN ? IN ('queued', 'running')
                               THEN NULL ELSE error_code END,
             error_message = CASE WHEN ? IN ('queued', 'running')
                                  THEN NULL ELSE error_message END
         WHERE id = ?",
    )
    .bind(status)
    .bind(now)
    .bind(status)
    .bind(now)
    .bind(status)
    .bind(now)
    .bind(status)
    .bind(status)
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn set_task_failed(
    pool: &SqlitePool,
    id: &str,
    code: &str,
    message: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    sqlx::query(
        "UPDATE download_tasks
         SET status = 'failed', error_code = ?, error_message = ?,
             finished_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(code)
    .bind(truncate(message, 1024))
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn prepare_task_run(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = clock.now_ms();
    let mut transaction = pool.begin().await.map_err(map)?;
    let result = sqlx::query(
        "UPDATE download_tasks
         SET status = 'running',
             started_at = COALESCE(started_at, ?),
             finished_at = NULL,
             error_code = NULL,
             error_message = NULL,
             updated_at = ?
         WHERE id = ? AND status IN ('queued', 'running')",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;

    if result.rows_affected() == 0 {
        transaction.rollback().await.map_err(map)?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE download_files
         SET status = 'queued', error_code = NULL, updated_at = ?
         WHERE task_id = ? AND status IN ('queued', 'running', 'paused')",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;

    recalculate_task_on(&mut transaction, id, now).await?;
    transaction.commit().await.map_err(map)?;
    Ok(true)
}

pub(crate) async fn requeue_task(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    let mut transaction = pool.begin().await.map_err(map)?;
    sqlx::query(
        "UPDATE download_tasks
         SET status = 'queued', finished_at = NULL, error_code = NULL,
             error_message = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;
    sqlx::query(
        "UPDATE download_files
         SET status = 'queued', error_code = NULL, updated_at = ?
         WHERE task_id = ? AND status IN ('paused', 'running', 'queued')",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;
    transaction.commit().await.map_err(map)?;
    Ok(())
}

/// Resets a failed/cancelled task for a new attempt. Staging is kept for
/// transient failures; checksum failures are restarted from byte zero.
pub(crate) async fn reset_for_retry(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    let mut transaction = pool.begin().await.map_err(map)?;
    sqlx::query(
        "UPDATE download_tasks
         SET status = 'queued', finished_at = NULL, error_code = NULL,
             error_message = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;

    sqlx::query(
        "UPDATE download_files
         SET status = 'queued',
             attempts = 0,
             error_code = NULL,
             last_http_status = NULL,
             bytes_done = CASE
                 WHEN error_code IN ('DOWNLOAD_HASH_MISMATCH', 'DOWNLOAD_SIZE_MISMATCH')
                 THEN 0 ELSE bytes_done END,
             resume_offset = CASE
                 WHEN error_code IN ('DOWNLOAD_HASH_MISMATCH', 'DOWNLOAD_SIZE_MISMATCH')
                 THEN 0 ELSE resume_offset END,
             etag = CASE
                 WHEN error_code IN ('DOWNLOAD_HASH_MISMATCH', 'DOWNLOAD_SIZE_MISMATCH')
                 THEN NULL ELSE etag END,
             last_modified = CASE
                 WHEN error_code IN ('DOWNLOAD_HASH_MISMATCH', 'DOWNLOAD_SIZE_MISMATCH')
                 THEN NULL ELSE last_modified END,
             content_length = CASE
                 WHEN error_code IN ('DOWNLOAD_HASH_MISMATCH', 'DOWNLOAD_SIZE_MISMATCH')
                 THEN NULL ELSE content_length END,
             updated_at = ?
         WHERE task_id = ? AND status <> 'completed' AND status <> 'skipped'",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;

    recalculate_task_on(&mut transaction, id, now).await?;
    transaction.commit().await.map_err(map)?;
    Ok(())
}

pub(crate) async fn mark_task_cancelled(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    let mut transaction = pool.begin().await.map_err(map)?;
    sqlx::query(
        "UPDATE download_files
         SET status = 'failed', error_code = 'DOWNLOAD_CANCELLED', updated_at = ?
         WHERE task_id = ? AND status NOT IN ('completed', 'skipped')",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;
    sqlx::query(
        "UPDATE download_tasks
         SET status = 'cancelled', finished_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(map)?;
    recalculate_task_on(&mut transaction, id, now).await?;
    transaction.commit().await.map_err(map)?;
    Ok(())
}

pub(crate) async fn delete_task(pool: &SqlitePool, id: &str) -> Result<bool, NexusError> {
    let result = sqlx::query("DELETE FROM download_tasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(map)?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn clear_finished(pool: &SqlitePool) -> Result<u64, NexusError> {
    let result = sqlx::query(
        "DELETE FROM download_tasks
         WHERE status IN ('completed', 'cancelled')",
    )
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(result.rows_affected())
}

pub(crate) async fn mark_file_running(
    pool: &SqlitePool,
    id: &str,
    http_status: Option<u16>,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    sqlx::query(
        "UPDATE download_files
         SET status = 'running',
             attempts = attempts + 1,
             last_http_status = ?,
             error_code = NULL,
             updated_at = ?
         WHERE id = ?",
    )
    .bind(http_status.map(i64::from))
    .bind(clock.now_ms())
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn update_file_progress(
    pool: &SqlitePool,
    id: &str,
    bytes_done: u64,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let bytes = i64::try_from(bytes_done).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE download_files
         SET bytes_done = ?,
             resume_offset = ?,
             size_bytes = MAX(size_bytes, ?),
             updated_at = ?
         WHERE id = ?",
    )
    .bind(bytes)
    .bind(bytes)
    .bind(bytes)
    .bind(clock.now_ms())
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn update_file_remote(
    pool: &SqlitePool,
    id: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    content_length: Option<u64>,
    http_status: u16,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let content_length = content_length.and_then(|value| i64::try_from(value).ok());
    sqlx::query(
        "UPDATE download_files
         SET etag = COALESCE(?, etag),
             last_modified = COALESCE(?, last_modified),
             content_length = COALESCE(?, content_length),
             size_bytes = CASE
                 WHEN size_bytes = 0 AND ? IS NOT NULL THEN ? ELSE size_bytes END,
             last_http_status = ?,
             updated_at = ?
         WHERE id = ?",
    )
    .bind(etag)
    .bind(last_modified)
    .bind(content_length)
    .bind(content_length)
    .bind(content_length)
    .bind(i64::from(http_status))
    .bind(clock.now_ms())
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn mark_file_completed(
    pool: &SqlitePool,
    id: &str,
    size: u64,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    let now = clock.now_ms();
    sqlx::query(
        "UPDATE download_files
         SET status = 'completed', bytes_done = ?, size_bytes = ?,
             resume_offset = ?, verified_at = ?, error_code = NULL,
             staging_rel = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(size)
    .bind(size)
    .bind(size)
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn mark_file_skipped(
    pool: &SqlitePool,
    id: &str,
    size: u64,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    let now = clock.now_ms();
    sqlx::query(
        "UPDATE download_files
         SET status = 'skipped', bytes_done = ?, size_bytes = ?,
             resume_offset = ?, verified_at = ?, error_code = NULL,
             staging_rel = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(size)
    .bind(size)
    .bind(size)
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn mark_file_paused(
    pool: &SqlitePool,
    id: &str,
    bytes_done: u64,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let bytes = i64::try_from(bytes_done).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE download_files
         SET status = 'paused', bytes_done = ?, resume_offset = ?,
             size_bytes = MAX(size_bytes, ?), updated_at = ?
         WHERE id = ?",
    )
    .bind(bytes)
    .bind(bytes)
    .bind(bytes)
    .bind(clock.now_ms())
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn mark_file_failed(
    pool: &SqlitePool,
    id: &str,
    code: &str,
    bytes_done: u64,
    http_status: Option<u16>,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let bytes = i64::try_from(bytes_done).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE download_files
         SET status = 'failed', error_code = ?, bytes_done = ?,
             resume_offset = ?, last_http_status = COALESCE(?, last_http_status),
             size_bytes = MAX(size_bytes, ?), updated_at = ?
         WHERE id = ?",
    )
    .bind(code)
    .bind(bytes)
    .bind(bytes)
    .bind(http_status.map(i64::from))
    .bind(bytes)
    .bind(clock.now_ms())
    .bind(id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn set_file_staging(
    pool: &SqlitePool,
    id: &str,
    staging_rel: Option<&str>,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    sqlx::query("UPDATE download_files SET staging_rel = ?, updated_at = ? WHERE id = ?")
        .bind(staging_rel)
        .bind(clock.now_ms())
        .bind(id)
        .execute(pool)
        .await
        .map_err(map)?;
    Ok(())
}

pub(crate) async fn recalculate_task(
    pool: &SqlitePool,
    task_id: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let mut connection = pool.acquire().await.map_err(map)?;
    recalculate_task_on(&mut connection, task_id, clock.now_ms()).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_error(
    pool: &SqlitePool,
    task_id: &str,
    file_id: Option<&str>,
    attempt: i64,
    code: &str,
    message: &str,
    http_status: Option<u16>,
    retryable: bool,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let now = clock.now_ms();
    let message = truncate(message, 1024);
    sqlx::query(
        "INSERT INTO download_errors
             (task_id, file_id, attempt, code, message, http_status, retryable, at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(task_id)
    .bind(file_id)
    .bind(attempt.max(0))
    .bind(truncate(code, 64))
    .bind(message)
    .bind(http_status.map(i64::from))
    .bind(i64::from(retryable))
    .bind(now)
    .execute(pool)
    .await
    .map_err(map)?;

    // Keep the history useful without letting a permanently broken URL grow the
    // database without bound.
    sqlx::query(
        "DELETE FROM download_errors
         WHERE task_id = ?
           AND id NOT IN (
               SELECT id FROM download_errors
               WHERE task_id = ?
               ORDER BY at DESC, id DESC
               LIMIT 200
           )",
    )
    .bind(task_id)
    .bind(task_id)
    .execute(pool)
    .await
    .map_err(map)?;
    Ok(())
}

pub(crate) async fn list_errors(
    pool: &SqlitePool,
    task_id: &str,
    limit: u32,
) -> Result<Vec<crate::download::types::DownloadErrorRecord>, NexusError> {
    let limit = limit.clamp(1, 500);
    sqlx::query_as::<_, crate::download::types::DownloadErrorRecord>(
        "SELECT id, task_id, file_id, attempt, code, message, http_status,
                retryable, at
         FROM download_errors
         WHERE task_id = ?
         ORDER BY at DESC, id DESC
         LIMIT ?",
    )
    .bind(task_id)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(map)
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
