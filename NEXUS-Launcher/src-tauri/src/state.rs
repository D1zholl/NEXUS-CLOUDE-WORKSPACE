use std::sync::{Arc, Mutex};

use nexus_core::config::BootstrapConfig;
use nexus_core::{Clock, DataPaths};
use nexus_platform::SettingsService;
use nexus_platform::db::{Db, InitReport};
use nexus_platform::download::DownloadEngine;
use nexus_platform::logging::LoggingGuard;
use nexus_platform::security::{SafeFs, SecretStore};

/// Application-wide state created once during `setup`.
///
/// `Db` is kept in an `Arc` so commands and the shutdown handler share the same
/// pool. The explicit shutdown path in `lib.rs` marks the database clean and
/// closes the pool.
///
/// `data_fs` is the only filesystem capability handed to domain services.
/// `secrets` is intentionally not exposed through Tauri commands: tokens are
/// read by Rust code only and never cross the IPC boundary.
pub struct AppState {
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub downloads: Arc<DownloadEngine>,
    pub paths: DataPaths,
    pub config: BootstrapConfig,
    pub clock: Arc<dyn Clock>,
    pub init: InitReport,
    pub data_fs: SafeFs,
    pub secrets: Arc<dyn SecretStore>,
    pub _logging: Mutex<Option<LoggingGuard>>,
}
