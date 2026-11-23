//! Public Download Engine types.
//!
//! The engine is deliberately domain-neutral: Minecraft versions, libraries,
//! assets, Java runtimes, mods, modpacks, NEXUS builds and server files all
//! describe their downloads with the same [`NewDownloadTask`] request. The only
//! domain-specific part is [`DownloadKind`] (a label/DB category) and
//! [`DownloadRoot`] (which managed root receives the file).

use std::fmt;

use nexus_core::{ErrorCode, NexusError};
use serde::{Deserialize, Serialize};

/// Logical owner of a download task.
///
/// Several owners share a database category (`libraries` and `assets` are part
/// of a Minecraft installation), but they remain distinguishable in labels and
/// event payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadKind {
    Minecraft,
    Libraries,
    Assets,
    Java,
    Mods,
    Modpacks,
    Nexus,
    ServerFiles,
    Loader,
    Custom,
}

impl DownloadKind {
    /// Value accepted by `download_tasks.kind`.
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Minecraft | Self::Libraries | Self::Assets => "minecraft",
            Self::Java => "java",
            Self::Mods => "mod",
            Self::Modpacks => "modpack",
            Self::Nexus => "nexus",
            Self::ServerFiles => "server",
            Self::Loader => "loader",
            Self::Custom => "custom",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minecraft => "minecraft",
            Self::Libraries => "libraries",
            Self::Assets => "assets",
            Self::Java => "java",
            Self::Mods => "mods",
            Self::Modpacks => "modpacks",
            Self::Nexus => "nexus",
            Self::ServerFiles => "server_files",
            Self::Loader => "loader",
            Self::Custom => "custom",
        }
    }
}

/// Managed root that receives a downloaded file.
///
/// `relative_path` in a request is always relative to this root. It may contain
/// subdirectories (for example `instances/<uuid>/game/mods/example.jar` for
/// [`DownloadRoot::Instance`]), but it can never escape the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadRoot {
    SharedAssets,
    SharedLibraries,
    SharedVersions,
    Instance,
    Server,
    Java,
    Cache,
    Downloads,
}

impl DownloadRoot {
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::SharedAssets => "shared_assets",
            Self::SharedLibraries => "shared_libraries",
            Self::SharedVersions => "shared_versions",
            Self::Instance => "instance",
            Self::Server => "server",
            Self::Java => "java",
            Self::Cache => "cache",
            Self::Downloads => "downloads",
        }
    }

    /// Path prefix inside the data directory.
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::SharedAssets => "shared/assets",
            Self::SharedLibraries => "shared/libraries",
            Self::SharedVersions => "shared/versions",
            Self::Instance => "instances",
            Self::Server => "servers",
            Self::Java => "java",
            Self::Cache => "cache",
            Self::Downloads => "downloads",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "shared_assets" => Some(Self::SharedAssets),
            "shared_libraries" => Some(Self::SharedLibraries),
            "shared_versions" => Some(Self::SharedVersions),
            "instance" => Some(Self::Instance),
            "server" => Some(Self::Server),
            "java" => Some(Self::Java),
            "cache" => Some(Self::Cache),
            "downloads" => Some(Self::Downloads),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Skipped,
}

impl FileStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }

    pub const fn is_done(self) -> bool {
        matches!(self, Self::Completed | Self::Skipped)
    }
}

impl fmt::Display for FileStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One file to download.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDownloadFile {
    pub url: String,
    pub root: DownloadRoot,
    /// Relative to `root.prefix()`.
    pub relative_path: String,
    /// Expected size, when the manifest knows it. `None`/`0` means unknown.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Optional SHA-1 from a provider manifest. Stored for diagnostics; the
    /// engine verifies SHA-256 when it is available.
    #[serde(default)]
    pub sha1: Option<String>,
    /// Optional expected SHA-256. When present the staged file is verified
    /// before it is moved into the destination root.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default = "default_required")]
    pub required: bool,
    /// Replace an existing destination file when its content does not match.
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

pub const fn default_required() -> bool {
    true
}

pub const fn default_true() -> bool {
    true
}

/// A complete download request. The engine creates one UUID per task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDownloadTask {
    pub kind: DownloadKind,
    pub label: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
    pub files: Vec<NewDownloadFile>,
}

pub const fn default_priority() -> i64 {
    100
}

/// Task-level snapshot returned by commands and events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskSnapshot {
    pub id: String,
    pub job_id: Option<String>,
    pub kind: String,
    pub label: String,
    pub status: TaskStatus,
    pub priority: i64,
    pub total_files: u64,
    pub completed_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub bytes_total_known: bool,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
    pub staging_path: Option<String>,
}

/// File-level snapshot. `speed_bps`/`eta_seconds` are live values while the
/// engine is running; after a restart they are `0`/`None` until resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFileSnapshot {
    pub id: String,
    pub task_id: String,
    pub url: String,
    pub root: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub bytes_done: u64,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub required: bool,
    pub status: FileStatus,
    pub attempts: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    pub last_http_status: Option<u16>,
    pub error_code: Option<String>,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub verified_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskDetail {
    pub task: DownloadTaskSnapshot,
    pub files: Vec<DownloadFileSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DownloadErrorRecord {
    pub id: i64,
    pub task_id: String,
    pub file_id: Option<String>,
    pub attempt: i64,
    pub code: String,
    pub message: String,
    pub http_status: Option<i64>,
    pub retryable: bool,
    pub at: i64,
}

/// Stable, user-facing failure description used by the engine and events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

impl DownloadFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            file_id: None,
            http_status: None,
        }
    }

    pub fn with_file(mut self, file_id: impl Into<String>) -> Self {
        self.file_id = Some(file_id.into());
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    pub fn from_nexus(error: &NexusError, retryable: bool) -> Self {
        Self {
            code: download_code(error.code()).to_owned(),
            message: error.to_string(),
            retryable,
            file_id: None,
            http_status: None,
        }
    }
}

/// Maps shared error codes to download-specific stable codes.
pub fn download_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ChecksumMismatch => "DOWNLOAD_HASH_MISMATCH",
        ErrorCode::FileTooLarge => "DOWNLOAD_SIZE_MISMATCH",
        ErrorCode::ResponseTooLarge => "DOWNLOAD_SIZE_MISMATCH",
        ErrorCode::NetworkTimeout => "DOWNLOAD_TIMEOUT",
        ErrorCode::UrlInvalid => "DOWNLOAD_URL_REJECTED",
        ErrorCode::UrlBlocked => "DOWNLOAD_URL_REJECTED",
        ErrorCode::TlsRequired => "DOWNLOAD_URL_REJECTED",
        ErrorCode::RedirectRejected => "DOWNLOAD_URL_REJECTED",
        ErrorCode::PermissionDenied => "DOWNLOAD_PERMISSION_DENIED",
        ErrorCode::Conflict => "DOWNLOAD_CONFLICT",
        ErrorCode::NotFound => "DOWNLOAD_NOT_FOUND",
        ErrorCode::InvalidArgument | ErrorCode::InvalidPath => "DOWNLOAD_INVALID_REQUEST",
        _ => "DOWNLOAD_FAILED",
    }
}

/// Event payloads. `DownloadEvent` is untagged so the Tauri adapter can emit
/// the payload directly under the event name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStartedEvent {
    pub task_id: String,
    pub kind: String,
    pub label: String,
    pub started_at: i64,
    pub files_total: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFileProgress {
    pub file_id: String,
    pub relative_path: String,
    pub status: FileStatus,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgressEvent {
    pub task_id: String,
    pub status: TaskStatus,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_total_known: bool,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub files: Vec<DownloadFileProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPausedEvent {
    pub task_id: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResumedEvent {
    pub task_id: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCompletedEvent {
    pub task_id: String,
    pub files_done: u64,
    pub files_total: u64,
    pub files_skipped: u64,
    pub bytes_downloaded: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFailedEvent {
    pub task_id: String,
    pub error: DownloadFailure,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCancelledEvent {
    pub task_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DownloadEvent {
    Started(DownloadStartedEvent),
    Progress(DownloadProgressEvent),
    Paused(DownloadPausedEvent),
    Resumed(DownloadResumedEvent),
    Completed(DownloadCompletedEvent),
    Failed(DownloadFailedEvent),
    Cancelled(DownloadCancelledEvent),
}

impl DownloadEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Started(_) => "download:started",
            Self::Progress(_) => "download:progress",
            Self::Paused(_) => "download:paused",
            Self::Resumed(_) => "download:resumed",
            Self::Completed(_) => "download:completed",
            Self::Failed(_) => "download:failed",
            Self::Cancelled(_) => "download:cancelled",
        }
    }
}

/// Adapter implemented by `src-tauri` (and by tests) to receive events.
pub trait DownloadEventSink: Send + Sync {
    fn emit(&self, event: DownloadEvent);
}

/// Live per-file progress maintained by a running task.
#[derive(Debug, Clone)]
pub(crate) struct FileProgressEntry {
    pub file_id: String,
    pub relative_path: String,
    pub status: FileStatus,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
}

/// Live task progress.
#[derive(Debug, Clone)]
pub(crate) struct TaskProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_total_known: bool,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub files: Vec<FileProgressEntry>,
}
