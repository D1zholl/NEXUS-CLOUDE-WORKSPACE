//! NEXUS Download Engine.
//!
//! One engine is shared by every module that needs files: Minecraft versions,
//! libraries, assets, Java runtimes, mods, modpacks, NEXUS builds and server
//! files. It provides a persistent queue, globally bounded concurrency, a
//! global speed limit, HTTP Range resume, per-file and task progress, pause,
//! resume, cancel, retry, backoff, staging and SHA-256 verification.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use nexus_platform::download::*;
//! # async fn example(engine: &DownloadEngine) -> Result<(), nexus_core::NexusError> {
//! let task = engine.enqueue(NewDownloadTask {
//!     kind: DownloadKind::Libraries,
//!     label: "Libraries for 1.21.1".to_owned(),
//!     job_id: None,
//!     priority: 100,
//!     files: vec![NewDownloadFile {
//!         url: "https://example.com/a.jar".to_owned(),
//!         root: DownloadRoot::SharedLibraries,
//!         relative_path: "com/example/a/1.0/a-1.0.jar".to_owned(),
//!         size_bytes: Some(1024),
//!         sha1: None,
//!         sha256: None,
//!         required: true,
//!         overwrite: true,
//!     }],
//! }).await?;
//! println!("{}", task.id);
//! # Ok(())
//! # }
//! ```

mod control;
mod engine;
mod http;
mod limiter;
mod progress;
mod staging;
mod store;
mod types;
mod worker;

pub use engine::{DownloadEngine, DownloadEngineConfig};
pub use types::{
    DownloadCancelledEvent, DownloadCompletedEvent, DownloadErrorRecord, DownloadEvent,
    DownloadEventSink, DownloadFailedEvent, DownloadFailure, DownloadFileProgress,
    DownloadFileSnapshot, DownloadKind, DownloadPausedEvent, DownloadProgressEvent,
    DownloadResumedEvent, DownloadRoot, DownloadStartedEvent, DownloadTaskDetail,
    DownloadTaskSnapshot, FileStatus, NewDownloadFile, NewDownloadTask, TaskStatus,
};
