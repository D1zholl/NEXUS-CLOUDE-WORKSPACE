//! Queue scheduler and task lifecycle.
//!
//! The engine owns one SQLite queue, one global speed limiter, one global
//! concurrency limiter and one runtime entry per active task. File workers do
//! the transfers; this module turns their outcomes into persistent task states
//! and Tauri events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexus_core::{Clock, FileId, NexusError, TaskId};
use sqlx::SqlitePool;
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::security::fs::{SafeFs, SafeRelativePath};
use crate::security::net::UrlPolicy;
use crate::security::net::url::UrlPurpose;

use super::control::{TaskCommand, TaskControl};
use super::http::DownloadHttpConfig;
use super::limiter::{ConcurrencyLimiter, SpeedLimiter};
use super::progress::TaskMeter;
use super::staging;
use super::store::{self, FileRow, TaskRow};
use super::types::{
    DownloadCancelledEvent, DownloadCompletedEvent, DownloadEvent, DownloadEventSink,
    DownloadFailedEvent, DownloadFailure, DownloadFileProgress, DownloadFileSnapshot,
    DownloadPausedEvent, DownloadProgressEvent, DownloadResumedEvent, DownloadRoot,
    DownloadStartedEvent, DownloadTaskDetail, DownloadTaskSnapshot, FileStatus, NewDownloadTask,
    TaskStatus,
};
use super::worker::{self, FileOutcome, WorkerContext};

#[derive(Debug, Clone)]
pub struct DownloadEngineConfig {
    pub max_attempts: u32,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub max_redirects: u8,
    pub progress_interval: Duration,
    pub persist_interval: Duration,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub max_file_bytes: u64,
    pub user_agent: String,
}

impl Default for DownloadEngineConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            connect_timeout: Duration::from_secs(20),
            read_timeout: Duration::from_secs(60),
            max_redirects: 3,
            progress_interval: Duration::from_millis(250),
            persist_interval: Duration::from_secs(1),
            retry_base: Duration::from_millis(750),
            retry_max: Duration::from_secs(30),
            max_file_bytes: 16 * 1024 * 1024 * 1024,
            user_agent: format!("NEXUS-Launcher/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

pub struct DownloadEngine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    fs: SafeFs,
    policy: UrlPolicy,
    events: Option<Arc<dyn DownloadEventSink>>,
    config: DownloadEngineConfig,
    limiter: Arc<SpeedLimiter>,
    concurrency: Arc<ConcurrencyLimiter>,
    max_concurrent: AtomicUsize,
    max_speed_bps: AtomicU64,
    runtimes: Mutex<HashMap<String, Arc<TaskRuntime>>>,
    running: Mutex<HashSet<String>>,
    scheduler_wake: Notify,
    shutdown: CancellationToken,
    started: AtomicBool,
    scheduler: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct TaskRuntime {
    control: TaskControl,
    status: watch::Sender<TaskStatus>,
    meter: Arc<TaskMeter>,
}

impl DownloadEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: SqlitePool,
        clock: Arc<dyn Clock>,
        fs: SafeFs,
        policy: UrlPolicy,
        events: Option<Arc<dyn DownloadEventSink>>,
        config: DownloadEngineConfig,
        max_concurrent: usize,
        max_speed_bps: u64,
    ) -> Self {
        let max_concurrent = max_concurrent.clamp(1, 32);
        let inner = EngineInner {
            pool,
            clock,
            fs,
            policy,
            events,
            config,
            limiter: Arc::new(SpeedLimiter::new(max_speed_bps)),
            concurrency: Arc::new(ConcurrencyLimiter::new(max_concurrent)),
            max_concurrent: AtomicUsize::new(max_concurrent),
            max_speed_bps: AtomicU64::new(max_speed_bps),
            runtimes: Mutex::new(HashMap::new()),
            running: Mutex::new(HashSet::new()),
            scheduler_wake: Notify::new(),
            shutdown: CancellationToken::new(),
            started: AtomicBool::new(false),
            scheduler: Mutex::new(None),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Starts the scheduler. Queued tasks from a previous session are picked up
    /// immediately; paused tasks stay paused until the user resumes them.
    pub async fn start(&self) -> Result<(), NexusError> {
        if self.inner.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            scheduler_loop(inner).await;
        });
        *lock(&self.inner.scheduler) = Some(handle);
        self.inner.scheduler_wake.notify_waiters();
        Ok(())
    }

    /// Stops the scheduler and pauses every active task, flushing exact offsets
    /// to staging sidecars. Called from the application exit handler.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        for runtime in lock(&self.inner.runtimes).values() {
            runtime.control.pause();
        }
        self.inner.scheduler_wake.notify_waiters();

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let empty = lock(&self.inner.running).is_empty();
            if empty || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let handle = lock(&self.inner.scheduler).take();
        if let Some(handle) = handle
            && tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .is_err()
        {
            // The handle was already consumed by `timeout`; the scheduler
            // observes `shutdown` and exits on its own.
        }

        // Safety net for a task whose worker did not finish its own pause path.
        if let Ok(active) = store::list_active_tasks(&self.inner.pool).await {
            for task in active {
                if task.status == "running" {
                    let _ = store::set_task_status(
                        &self.inner.pool,
                        &task.id,
                        "paused",
                        self.inner.clock.as_ref(),
                    )
                    .await;
                }
            }
        }
        self.inner.started.store(false, Ordering::Release);
    }

    pub fn limits(&self) -> (usize, u64) {
        (
            self.inner.max_concurrent.load(Ordering::Acquire),
            self.inner.max_speed_bps.load(Ordering::Acquire),
        )
    }

    pub fn set_limits(&self, max_concurrent: usize, max_speed_bps: u64) {
        let max_concurrent = max_concurrent.clamp(1, 32);
        self.inner
            .max_concurrent
            .store(max_concurrent, Ordering::Release);
        self.inner
            .max_speed_bps
            .store(max_speed_bps, Ordering::Release);
        self.inner.concurrency.set_limit(max_concurrent);
        self.inner.limiter.set_rate_bps(max_speed_bps);
        self.inner.scheduler_wake.notify_waiters();
    }

    pub fn speed_bps(&self) -> u64 {
        self.inner.max_speed_bps.load(Ordering::Acquire)
    }

    pub async fn enqueue(
        &self,
        request: NewDownloadTask,
    ) -> Result<DownloadTaskSnapshot, NexusError> {
        validate_task(&self.inner, &request)?;

        let task_id = TaskId::new();
        let mut validated = Vec::with_capacity(request.files.len());
        let mut seen_paths = HashSet::new();

        for file in &request.files {
            let validated_file = validate_file(&self.inner, &request, file, &mut seen_paths)?;
            validated.push(validated_file);
        }

        let task_id_string = task_id.to_string_lower();
        let kind = request.kind.as_db_str();
        let label = request.label.trim();
        let file_rows: Vec<store::NewFileRow<'_>> = validated
            .iter()
            .map(|file| store::NewFileRow {
                id: &file.id,
                url: &file.url,
                root_kind: file.root_kind,
                rel_path: &file.rel_path,
                path_key: &file.path_key,
                size_bytes: file.size_bytes,
                sha1: file.sha1.as_deref(),
                sha256: file.sha256.as_deref(),
                required: file.required,
            })
            .collect();

        store::create_task_with_files(
            &self.inner.pool,
            self.inner.clock.as_ref(),
            store::NewTaskRow {
                id: &task_id_string,
                job_id: request.job_id.as_deref(),
                kind,
                label,
                priority: request.priority,
            },
            &file_rows,
        )
        .await?;

        self.inner.scheduler_wake.notify_one();
        self.get_task(task_id).await
    }

    pub async fn list_tasks(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<DownloadTaskSnapshot>, NexusError> {
        let rows = store::list_tasks(&self.inner.pool, status).await?;
        Ok(rows
            .iter()
            .map(|row| task_snapshot(&self.inner, row))
            .collect())
    }

    pub async fn get_task(&self, id: TaskId) -> Result<DownloadTaskSnapshot, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        Ok(task_snapshot(&self.inner, &row))
    }

    pub async fn get_detail(&self, id: TaskId) -> Result<DownloadTaskDetail, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let files = store::list_files(&self.inner.pool, &id_string).await?;
        Ok(DownloadTaskDetail {
            task: task_snapshot(&self.inner, &row),
            files: file_snapshots(&self.inner, &row, &files),
        })
    }

    pub async fn files(&self, id: TaskId) -> Result<Vec<DownloadFileSnapshot>, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let files = store::list_files(&self.inner.pool, &id_string).await?;
        Ok(file_snapshots(&self.inner, &row, &files))
    }

    pub async fn pause(&self, id: TaskId) -> Result<DownloadTaskSnapshot, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
        if status.is_terminal() {
            return Ok(task_snapshot(&self.inner, &row));
        }

        let runtime = lock(&self.inner.runtimes).get(&id_string).cloned();
        if let Some(runtime) = runtime {
            runtime.control.pause();
            self.inner.scheduler_wake.notify_one();
            let status = wait_for_status(&runtime, Duration::from_secs(20)).await;
            if status == TaskStatus::Running || status == TaskStatus::Queued {
                // Workers did not stop in time; keep the DB authoritative.
                let _ = store::set_task_status(
                    &self.inner.pool,
                    &id_string,
                    "paused",
                    self.inner.clock.as_ref(),
                )
                .await;
            }
        } else {
            store::set_task_status(
                &self.inner.pool,
                &id_string,
                "paused",
                self.inner.clock.as_ref(),
            )
            .await?;
        }

        let snapshot = self.get_task(id).await?;
        inner_emit(
            &self.inner,
            DownloadEvent::Paused(DownloadPausedEvent {
                task_id: id_string,
                files_done: snapshot.completed_files,
                files_total: snapshot.total_files,
                bytes_done: snapshot.done_bytes,
                bytes_total: snapshot.total_bytes,
            }),
        );
        Ok(snapshot)
    }

    pub async fn resume(&self, id: TaskId) -> Result<DownloadTaskSnapshot, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
        match status {
            TaskStatus::Completed => {
                return Err(NexusError::conflict("completed task cannot be resumed"));
            }
            TaskStatus::Failed | TaskStatus::Cancelled => {
                return Err(NexusError::conflict(
                    "use retry for a failed or cancelled task",
                ));
            }
            TaskStatus::Running | TaskStatus::Queued => {
                return Ok(task_snapshot(&self.inner, &row));
            }
            TaskStatus::Paused => {}
        }

        lock(&self.inner.runtimes).remove(&id_string);
        store::requeue_task(&self.inner.pool, &id_string, self.inner.clock.as_ref()).await?;
        let snapshot = self.get_task(id).await?;
        inner_emit(
            &self.inner,
            DownloadEvent::Resumed(DownloadResumedEvent {
                task_id: id_string,
                files_done: snapshot.completed_files,
                files_total: snapshot.total_files,
                bytes_done: snapshot.done_bytes,
                bytes_total: snapshot.total_bytes,
            }),
        );
        self.inner.scheduler_wake.notify_one();
        Ok(snapshot)
    }

    pub async fn cancel(&self, id: TaskId) -> Result<DownloadTaskSnapshot, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
        if status.is_terminal() {
            return Ok(task_snapshot(&self.inner, &row));
        }

        let runtime = lock(&self.inner.runtimes).get(&id_string).cloned();
        if let Some(runtime) = runtime {
            runtime.control.cancel();
            let status = wait_for_status(&runtime, Duration::from_secs(20)).await;
            if status != TaskStatus::Cancelled {
                finish_cancelled(&self.inner, &id_string).await?;
            }
        } else {
            finish_cancelled(&self.inner, &id_string).await?;
        }
        self.get_task(id).await
    }

    pub async fn retry(&self, id: TaskId) -> Result<DownloadTaskSnapshot, NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
        if !matches!(
            status,
            TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Paused
        ) {
            return Err(NexusError::conflict(
                "retry is only available for failed, cancelled or paused tasks",
            ));
        }

        // A checksum failure must never resume stale bytes.
        let files = store::list_files(&self.inner.pool, &id_string).await?;
        for file in &files {
            if file.error_code.as_deref() == Some("DOWNLOAD_HASH_MISMATCH")
                || file.error_code.as_deref() == Some("DOWNLOAD_SIZE_MISMATCH")
            {
                staging::remove_part(&self.inner.fs, &id_string, &file.id);
                staging::remove_meta(&self.inner.fs, &id_string, &file.id);
            }
        }

        lock(&self.inner.runtimes).remove(&id_string);
        store::reset_for_retry(&self.inner.pool, &id_string, self.inner.clock.as_ref()).await?;
        let snapshot = self.get_task(id).await?;
        inner_emit(
            &self.inner,
            DownloadEvent::Resumed(DownloadResumedEvent {
                task_id: id_string,
                files_done: snapshot.completed_files,
                files_total: snapshot.total_files,
                bytes_done: snapshot.done_bytes,
                bytes_total: snapshot.total_bytes,
            }),
        );
        self.inner.scheduler_wake.notify_one();
        Ok(snapshot)
    }

    pub async fn remove(&self, id: TaskId) -> Result<(), NexusError> {
        let id_string = id.to_string_lower();
        let row = store::get_task(&self.inner.pool, &id_string)
            .await?
            .ok_or_else(|| NexusError::not_found("download task not found"))?;
        let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
        if !status.is_terminal() {
            return Err(NexusError::conflict(
                "cancel an active task before removing it",
            ));
        }
        staging::remove_task_staging(&self.inner.fs, &id_string);
        store::delete_task(&self.inner.pool, &id_string).await?;
        Ok(())
    }

    pub async fn clear_finished(&self) -> Result<u64, NexusError> {
        let tasks = store::list_tasks(&self.inner.pool, None).await?;
        for task in &tasks {
            if matches!(task.status.as_str(), "completed" | "cancelled") {
                staging::remove_task_staging(&self.inner.fs, &task.id);
            }
        }
        store::clear_finished(&self.inner.pool).await
    }

    pub async fn errors(
        &self,
        id: TaskId,
        limit: u32,
    ) -> Result<Vec<super::types::DownloadErrorRecord>, NexusError> {
        store::list_errors(&self.inner.pool, &id.to_string_lower(), limit).await
    }
}

fn validate_task(inner: &EngineInner, request: &NewDownloadTask) -> Result<(), NexusError> {
    let label = request.label.trim();
    if label.is_empty() || label.chars().count() > 256 {
        return Err(NexusError::invalid_argument(
            "download label must contain between 1 and 256 characters",
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(NexusError::invalid_argument(
            "download label contains control characters",
        ));
    }
    if !(-1_000..=10_000).contains(&request.priority) {
        return Err(NexusError::invalid_argument(
            "priority must be between -1000 and 10000",
        ));
    }
    if request.files.is_empty() {
        return Err(NexusError::invalid_argument(
            "download task must contain at least one file",
        ));
    }
    if request.files.len() > 50_000 {
        return Err(NexusError::invalid_argument(
            "download task contains too many files",
        ));
    }
    if let Some(job_id) = &request.job_id
        && (job_id.is_empty() || job_id.len() > 64)
    {
        return Err(NexusError::invalid_argument("job id is invalid"));
    }
    let _ = inner;
    Ok(())
}

struct ValidatedFile {
    id: String,
    url: String,
    root_kind: &'static str,
    rel_path: String,
    path_key: String,
    size_bytes: i64,
    sha1: Option<String>,
    sha256: Option<String>,
    required: bool,
}

fn validate_file(
    inner: &EngineInner,
    request: &NewDownloadTask,
    file: &super::types::NewDownloadFile,
    seen_paths: &mut HashSet<String>,
) -> Result<ValidatedFile, NexusError> {
    let _ = request;
    if file.url.len() > 4096 || file.url.chars().any(char::is_control) {
        return Err(NexusError::invalid_argument("download URL is invalid"));
    }
    inner.policy.validate(&file.url, UrlPurpose::Download)?;

    let relative = SafeRelativePath::parse(&file.relative_path)?;
    let rel_path = relative.to_string();
    if rel_path.is_empty() || rel_path.len() > 4096 {
        return Err(NexusError::invalid_path(
            "download relative path is invalid",
        ));
    }

    let root = file.root;
    if root == DownloadRoot::Downloads {
        let first = relative
            .as_path()
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase());
        if first.as_deref() == Some("staging") {
            return Err(NexusError::invalid_path(
                "downloads/staging is reserved for unfinished transfers",
            ));
        }
    }
    let destination = SafeRelativePath::parse(format!("{}/{}", root.prefix(), rel_path))?;
    let path_key = destination.to_string().to_lowercase();
    if !seen_paths.insert(path_key.clone()) {
        return Err(NexusError::invalid_path(
            "two files in one task resolve to the same destination",
        ));
    }

    let size = file.size_bytes.unwrap_or(0);
    if size > inner.config.max_file_bytes {
        return Err(NexusError::file_too_large(
            "file exceeds the engine size limit",
        ));
    }
    let size_bytes = i64::try_from(size)
        .map_err(|_| NexusError::invalid_argument("file size does not fit in the database"))?;

    let sha256 = normalize_sha256(file.sha256.as_deref())?;
    let sha1 = normalize_sha1(file.sha1.as_deref())?;

    if let Some(sha256) = &sha256
        && let Some(size) = file.size_bytes
        && size == 0
    {
        // An empty file with an explicit SHA-256 is valid; nothing to do.
        let _ = sha256;
    }

    Ok(ValidatedFile {
        id: FileId::new().to_string_lower(),
        url: file.url.trim().to_owned(),
        root_kind: root.as_db_str(),
        rel_path,
        path_key,
        size_bytes,
        sha1,
        sha256,
        required: file.required,
    })
}

fn normalize_sha256(raw: Option<&str>) -> Result<Option<String>, NexusError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            if value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(NexusError::invalid_argument(
                    "SHA-256 must be a 64-character hex string",
                ));
            }
            Ok(Some(value))
        }
    }
}

fn normalize_sha1(raw: Option<&str>) -> Result<Option<String>, NexusError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            if value.len() != 40 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(NexusError::invalid_argument(
                    "SHA-1 must be a 40-character hex string",
                ));
            }
            Ok(Some(value))
        }
    }
}

async fn scheduler_loop(inner: Arc<EngineInner>) {
    loop {
        if inner.shutdown.is_cancelled() {
            break;
        }
        if let Err(error) = schedule_once(&inner).await {
            tracing::warn!(code = %error.code(), "download scheduler tick failed");
        }
        tokio::select! {
            _ = inner.scheduler_wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(750)) => {}
            _ = inner.shutdown.cancelled() => break,
        }
    }
}

async fn schedule_once(inner: &Arc<EngineInner>) -> Result<(), NexusError> {
    let tasks = store::list_active_tasks(&inner.pool).await?;
    for task in tasks {
        if inner.shutdown.is_cancelled() {
            break;
        }
        let should_start = {
            let mut running = lock(&inner.running);
            if running.contains(&task.id) {
                false
            } else {
                running.insert(task.id.clone());
                true
            }
        };
        if !should_start {
            continue;
        }
        let inner = inner.clone();
        let task_id = task.id.clone();
        tokio::spawn(async move {
            run_task(inner.clone(), task_id.clone()).await;
            lock(&inner.running).remove(&task_id);
            inner.scheduler_wake.notify_one();
        });
    }
    Ok(())
}

async fn run_task(inner: Arc<EngineInner>, task_id: String) {
    let row = match store::get_task(&inner.pool, &task_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(task_id, code = %error.code(), "cannot load download task");
            return;
        }
    };
    let status = TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued);
    if !matches!(status, TaskStatus::Queued | TaskStatus::Running) {
        return;
    }

    if !store::prepare_task_run(&inner.pool, &task_id, inner.clock.as_ref())
        .await
        .unwrap_or(false)
    {
        return;
    }

    let files = match store::list_files(&inner.pool, &task_id).await {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(task_id, code = %error.code(), "cannot load download files");
            return;
        }
    };

    if files.is_empty() {
        let _ =
            store::set_task_status(&inner.pool, &task_id, "completed", inner.clock.as_ref()).await;
        inner_emit(
            &inner,
            DownloadEvent::Completed(DownloadCompletedEvent {
                task_id,
                files_done: 0,
                files_total: 0,
                files_skipped: 0,
                bytes_downloaded: 0,
                duration_ms: 0,
            }),
        );
        return;
    }

    let (meter, _file_meters) = TaskMeter::from_files(&files);
    let control = TaskControl::new();
    let (status_tx, _) = watch::channel(TaskStatus::Running);
    let runtime = Arc::new(TaskRuntime {
        control: control.clone(),
        status: status_tx,
        meter: meter.clone(),
    });
    lock(&inner.runtimes).insert(task_id.clone(), runtime.clone());

    let now = inner.clock.now_ms();
    let started_at = row.started_at.unwrap_or(now);
    let initial = meter.snapshot();
    inner_emit(
        &inner,
        DownloadEvent::Started(DownloadStartedEvent {
            task_id: task_id.clone(),
            kind: row.kind.clone(),
            label: row.label.clone(),
            started_at,
            files_total: initial.files_total,
            bytes_total: initial.bytes_total,
        }),
    );
    emit_progress(&inner, &task_id, TaskStatus::Running, &initial);

    let mut pending: VecDeque<FileRow> = files
        .iter()
        .filter(|file| {
            let status = FileStatus::parse_db(&file.status).unwrap_or(FileStatus::Queued);
            !status.is_done()
        })
        .cloned()
        .collect();
    let required_by_id: HashMap<String, bool> = files
        .iter()
        .map(|file| (file.id.clone(), file.required()))
        .collect();

    let mut set: JoinSet<FileOutcome> = JoinSet::new();
    let mut control_rx = control.subscribe();
    let per_task = inner
        .max_concurrent
        .load(Ordering::Acquire)
        .saturating_mul(2)
        .clamp(1, 16);
    let mut ticker = tokio::time::interval(inner.config.progress_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_persist = Instant::now();
    let mut failures: Vec<DownloadFailure> = Vec::new();
    let mut stop_requested = false;

    loop {
        if !stop_requested && control.command() == TaskCommand::Run {
            while set.len() < per_task {
                let Some(file) = pending.pop_front() else {
                    break;
                };
                let Some(file_meter) = meter.file(&file.id).cloned() else {
                    continue;
                };
                let ctx = worker_context(&inner);
                let task_id = task_id.clone();
                let control = control.clone();
                let meter = meter.clone();
                set.spawn(async move {
                    worker::download_file(&ctx, &task_id, file, &control, meter, file_meter).await
                });
            }
        }

        if set.is_empty() && (pending.is_empty() || stop_requested) {
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                let progress = meter.snapshot();
                if control.command() == TaskCommand::Run {
                    emit_progress(&inner, &task_id, TaskStatus::Running, &progress);
                }
                if last_persist.elapsed() >= inner.config.persist_interval {
                    persist_progress(&inner, &task_id, &meter).await;
                    last_persist = Instant::now();
                }
            }
            joined = set.join_next() => {
                match joined {
                    Some(Ok(outcome)) => {
                        match outcome {
                            FileOutcome::Completed { .. } | FileOutcome::Paused { .. } | FileOutcome::Cancelled { .. } => {}
                            FileOutcome::Failed { error, .. } => {
                                let required = error
                                    .file_id
                                    .as_ref()
                                    .and_then(|file_id| required_by_id.get(file_id))
                                    .copied()
                                    .unwrap_or(true);
                                if required && error.code != "DOWNLOAD_CANCELLED" {
                                    failures.push(error);
                                }
                            }
                        }
                    }
                    Some(Err(error)) => {
                        tracing::warn!(%error, "download worker panicked");
                        failures.push(DownloadFailure::new(
                            "DOWNLOAD_FAILED",
                            "Внутренняя ошибка обработчика загрузки.",
                            true,
                        ));
                    }
                    None => {}
                }
            }
            changed = control_rx.changed() => {
                stop_requested = changed.is_err() || control.command() != TaskCommand::Run;
            }
        }
    }

    // Give cooperative workers a bounded amount of time to flush and exit.
    if !set.is_empty() {
        let drain = async { while set.join_next().await.is_some() {} };
        if tokio::time::timeout(Duration::from_secs(10), drain)
            .await
            .is_err()
        {
            set.shutdown().await;
        }
    }

    persist_progress(&inner, &task_id, &meter).await;
    let final_progress = meter.snapshot();
    let command = control.command();
    let result = if command == TaskCommand::Cancel {
        finish_cancelled(&inner, &task_id).await
    } else if command == TaskCommand::Pause {
        finish_paused(&inner, &task_id, &final_progress).await
    } else if let Some(failure) = failures.first() {
        finish_failed(&inner, &task_id, failure.clone()).await
    } else {
        finish_completed(&inner, &task_id, &meter).await
    };

    if let Err(error) = result {
        tracing::warn!(task_id, code = %error.code(), "cannot finalize download task");
    }
    let _ = runtime;
    lock(&inner.runtimes).remove(&task_id);
}

async fn finish_paused(
    inner: &Arc<EngineInner>,
    task_id: &str,
    progress: &super::types::TaskProgress,
) -> Result<(), NexusError> {
    let _ = store::set_task_status(&inner.pool, task_id, "paused", inner.clock.as_ref()).await;
    if let Some(runtime) = lock(&inner.runtimes).get(task_id) {
        runtime.status.send_replace(TaskStatus::Paused);
    }
    inner_emit(
        inner,
        DownloadEvent::Paused(DownloadPausedEvent {
            task_id: task_id.to_owned(),
            files_done: progress.files_done,
            files_total: progress.files_total,
            bytes_done: progress.bytes_done,
            bytes_total: progress.bytes_total,
        }),
    );
    Ok(())
}

async fn finish_cancelled(inner: &Arc<EngineInner>, task_id: &str) -> Result<(), NexusError> {
    staging::remove_task_staging(&inner.fs, task_id);
    store::mark_task_cancelled(&inner.pool, task_id, inner.clock.as_ref()).await?;
    let row = store::get_task(&inner.pool, task_id).await?;
    let (done, total) = row
        .as_ref()
        .map(|row| (row.done_bytes.max(0) as u64, row.total_bytes.max(0) as u64))
        .unwrap_or((0, 0));
    if let Some(runtime) = lock(&inner.runtimes).get(task_id) {
        runtime.status.send_replace(TaskStatus::Cancelled);
    }
    inner_emit(
        inner,
        DownloadEvent::Cancelled(DownloadCancelledEvent {
            task_id: task_id.to_owned(),
            bytes_done: done,
            bytes_total: total,
        }),
    );
    Ok(())
}

async fn finish_failed(
    inner: &Arc<EngineInner>,
    task_id: &str,
    failure: DownloadFailure,
) -> Result<(), NexusError> {
    let _ = store::record_error(
        &inner.pool,
        task_id,
        failure.file_id.as_deref(),
        0,
        &failure.code,
        &failure.message,
        failure.http_status,
        failure.retryable,
        inner.clock.as_ref(),
    )
    .await;
    store::set_task_failed(
        &inner.pool,
        task_id,
        &failure.code,
        &failure.message,
        inner.clock.as_ref(),
    )
    .await?;
    staging::remove_staging_dir_if_empty(&inner.fs, task_id);
    let row = store::get_task(&inner.pool, task_id).await?;
    let (files_done, files_total, bytes_done, bytes_total) = row
        .as_ref()
        .map(|row| {
            (
                row.completed_files.max(0) as u64,
                row.total_files.max(0) as u64,
                row.done_bytes.max(0) as u64,
                row.total_bytes.max(0) as u64,
            )
        })
        .unwrap_or((0, 0, 0, 0));
    if let Some(runtime) = lock(&inner.runtimes).get(task_id) {
        runtime.status.send_replace(TaskStatus::Failed);
    }
    inner_emit(
        inner,
        DownloadEvent::Failed(DownloadFailedEvent {
            task_id: task_id.to_owned(),
            error: failure,
            files_done,
            files_total,
            bytes_done,
            bytes_total,
        }),
    );
    Ok(())
}

async fn finish_completed(
    inner: &Arc<EngineInner>,
    task_id: &str,
    meter: &Arc<TaskMeter>,
) -> Result<(), NexusError> {
    let files = store::list_files(&inner.pool, task_id).await?;
    if let Some(failed) = files.iter().find(|file| {
        file.required()
            && FileStatus::parse_db(&file.status)
                .map(|status| !status.is_done())
                .unwrap_or(true)
    }) {
        let failure = DownloadFailure::new(
            "DOWNLOAD_FAILED",
            "Не все обязательные файлы были загружены.",
            true,
        )
        .with_file(&failed.id);
        return finish_failed(inner, task_id, failure).await;
    }

    store::recalculate_task(&inner.pool, task_id, inner.clock.as_ref()).await?;
    let _ = store::set_task_status(&inner.pool, task_id, "completed", inner.clock.as_ref()).await;
    staging::remove_task_staging(&inner.fs, task_id);

    let row = store::get_task(&inner.pool, task_id).await?;
    let (files_done, files_total, files_skipped, duration_ms) = match &row {
        Some(row) => {
            let files = store::list_files(&inner.pool, task_id).await?;
            let skipped = files.iter().filter(|file| file.status == "skipped").count() as u64;
            let duration = row
                .started_at
                .map(|started| (row.updated_at - started).max(0) as u64)
                .unwrap_or(0);
            (
                row.completed_files.max(0) as u64,
                row.total_files.max(0) as u64,
                skipped,
                duration,
            )
        }
        None => (0, 0, 0, 0),
    };
    let final_progress = meter.snapshot();
    emit_progress(inner, task_id, TaskStatus::Completed, &final_progress);
    if let Some(runtime) = lock(&inner.runtimes).get(task_id) {
        runtime.status.send_replace(TaskStatus::Completed);
    }
    inner_emit(
        inner,
        DownloadEvent::Completed(DownloadCompletedEvent {
            task_id: task_id.to_owned(),
            files_done,
            files_total,
            files_skipped,
            bytes_downloaded: meter.session_bytes(),
            duration_ms,
        }),
    );
    Ok(())
}

async fn persist_progress(inner: &Arc<EngineInner>, task_id: &str, meter: &Arc<TaskMeter>) {
    for file in meter.files() {
        let status = file.status();
        if status == FileStatus::Running
            && file.bytes_done() > 0
            && let Err(error) = store::update_file_progress(
                &inner.pool,
                &file.file_id,
                file.bytes_done(),
                inner.clock.as_ref(),
            )
            .await
        {
            tracing::warn!(file_id = %file.file_id, code = %error.code(), "cannot persist file progress");
        }
    }
    if let Err(error) = store::recalculate_task(&inner.pool, task_id, inner.clock.as_ref()).await {
        tracing::warn!(task_id, code = %error.code(), "cannot recalculate task progress");
    }
}

fn emit_progress(
    inner: &Arc<EngineInner>,
    task_id: &str,
    status: TaskStatus,
    progress: &super::types::TaskProgress,
) {
    inner_emit(
        inner,
        DownloadEvent::Progress(DownloadProgressEvent {
            task_id: task_id.to_owned(),
            status,
            files_done: progress.files_done,
            files_total: progress.files_total,
            bytes_done: progress.bytes_done,
            bytes_total: progress.bytes_total,
            bytes_total_known: progress.bytes_total_known,
            speed_bps: progress.speed_bps,
            eta_seconds: progress.eta_seconds,
            files: progress
                .files
                .iter()
                .map(|file| DownloadFileProgress {
                    file_id: file.file_id.clone(),
                    relative_path: file.relative_path.clone(),
                    status: file.status,
                    bytes_done: file.bytes_done,
                    bytes_total: file.bytes_total,
                    speed_bps: file.speed_bps,
                    eta_seconds: file.eta_seconds,
                })
                .collect(),
        }),
    );
}

fn worker_context(inner: &Arc<EngineInner>) -> WorkerContext {
    WorkerContext {
        pool: inner.pool.clone(),
        clock: inner.clock.clone(),
        fs: inner.fs.clone(),
        policy: inner.policy.clone(),
        http: DownloadHttpConfig {
            connect_timeout: inner.config.connect_timeout,
            read_timeout: inner.config.read_timeout,
            max_redirects: inner.config.max_redirects,
            user_agent: inner.config.user_agent.clone(),
        },
        limiter: inner.limiter.clone(),
        concurrency: inner.concurrency.clone(),
        max_attempts: inner.config.max_attempts,
        max_file_bytes: inner.config.max_file_bytes,
        retry_base: inner.config.retry_base,
        retry_max: inner.config.retry_max,
    }
}

fn task_snapshot(inner: &Arc<EngineInner>, row: &TaskRow) -> DownloadTaskSnapshot {
    let runtime = lock(&inner.runtimes).get(&row.id).cloned();
    let live = runtime.as_ref().map(|runtime| runtime.meter.snapshot());
    let status = runtime
        .as_ref()
        .map(|runtime| *runtime.status.borrow())
        .unwrap_or_else(|| TaskStatus::parse_db(&row.status).unwrap_or(TaskStatus::Queued));
    let bytes_total_known = live
        .as_ref()
        .map(|progress| progress.bytes_total_known)
        .unwrap_or(row.total_bytes > 0 || status == TaskStatus::Completed);

    DownloadTaskSnapshot {
        id: row.id.clone(),
        job_id: row.job_id.clone(),
        kind: row.kind.clone(),
        label: row.label.clone(),
        status,
        priority: row.priority,
        total_files: live
            .as_ref()
            .map(|progress| progress.files_total)
            .unwrap_or_else(|| row.total_files.max(0) as u64),
        completed_files: live
            .as_ref()
            .map(|progress| progress.files_done)
            .unwrap_or_else(|| row.completed_files.max(0) as u64),
        total_bytes: live
            .as_ref()
            .map(|progress| progress.bytes_total)
            .unwrap_or_else(|| row.total_bytes.max(0) as u64),
        done_bytes: live
            .as_ref()
            .map(|progress| progress.bytes_done)
            .unwrap_or_else(|| row.done_bytes.max(0) as u64),
        bytes_total_known,
        speed_bps: live
            .as_ref()
            .map(|progress| progress.speed_bps)
            .unwrap_or(0),
        eta_seconds: live.as_ref().and_then(|progress| progress.eta_seconds),
        error_code: row.error_code.clone(),
        error_message: row.error_message.clone(),
        created_at: row.created_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        updated_at: row.updated_at,
        staging_path: if matches!(
            status,
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused
        ) && row.total_files > 0
        {
            Some(format!("downloads/staging/{}", row.id))
        } else {
            None
        },
    }
}

fn file_snapshots(
    inner: &Arc<EngineInner>,
    task_row: &TaskRow,
    files: &[FileRow],
) -> Vec<DownloadFileSnapshot> {
    let runtime = lock(&inner.runtimes).get(&task_row.id).cloned();
    let progress = runtime.as_ref().map(|runtime| runtime.meter.snapshot());
    files
        .iter()
        .map(|file| {
            let live = progress
                .as_ref()
                .and_then(|progress| progress.files.iter().find(|entry| entry.file_id == file.id));
            DownloadFileSnapshot {
                id: file.id.clone(),
                task_id: file.task_id.clone(),
                url: file.url.clone(),
                root: file.root_kind.clone(),
                relative_path: file.rel_path.clone(),
                size_bytes: live
                    .map(|entry| entry.bytes_total)
                    .unwrap_or_else(|| file.size_bytes_u64()),
                bytes_done: live
                    .map(|entry| entry.bytes_done)
                    .unwrap_or_else(|| file.bytes_done_u64()),
                sha1: file.sha1.clone(),
                sha256: file.sha256.clone(),
                required: file.required(),
                status: live.map(|entry| entry.status).unwrap_or_else(|| {
                    FileStatus::parse_db(&file.status).unwrap_or(FileStatus::Queued)
                }),
                attempts: file.attempts.max(0) as u64,
                etag: file.etag.clone(),
                last_modified: file.last_modified.clone(),
                content_length: file
                    .content_length
                    .and_then(|value| u64::try_from(value).ok()),
                last_http_status: file
                    .last_http_status
                    .and_then(|value| u16::try_from(value).ok()),
                error_code: file.error_code.clone(),
                speed_bps: live.map(|entry| entry.speed_bps).unwrap_or(0),
                eta_seconds: live.and_then(|entry| entry.eta_seconds),
                verified_at: file.verified_at,
                updated_at: file.updated_at,
            }
        })
        .collect()
}

async fn wait_for_status(runtime: &TaskRuntime, timeout: Duration) -> TaskStatus {
    let mut receiver = runtime.status.subscribe();
    let wait = async {
        loop {
            let status = *receiver.borrow();
            if status.is_terminal() || status == TaskStatus::Paused {
                return status;
            }
            if receiver.changed().await.is_err() {
                return *receiver.borrow();
            }
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .unwrap_or_else(|_| *runtime.status.borrow())
}

fn inner_emit(inner: &Arc<EngineInner>, event: DownloadEvent) {
    if let Some(events) = &inner.events {
        events.emit(event);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
