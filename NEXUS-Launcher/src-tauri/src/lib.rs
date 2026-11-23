//! NEXUS Launcher desktop shell.
//!
//! Composition root only: the domain and storage logic live in `nexus-core`
//! and `nexus-platform`. Tauri commands are thin adapters that validate DTOs,
//! call services and convert errors into a stable [`nexus_core::ErrorDto`].

pub mod commands;
pub mod dto;
pub mod events;
pub mod state;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexus_core::config;
use nexus_core::time::SystemClock;
use nexus_core::{Clock, DataPaths, NexusError};
use nexus_platform::SettingsService;
use nexus_platform::db::{Db, DbOptions};
use nexus_platform::download::{DownloadEngine, DownloadEngineConfig, DownloadEventSink};
use nexus_platform::logging;
use nexus_platform::security::{KeyringSecretStore, SafeFs, SecretStore, UrlPolicy};
use tauri::{Manager, RunEvent};
use tracing::error;

use crate::events::TauriEventSink;
use crate::state::AppState;

async fn bootstrap(
    app: tauri::AppHandle,
    paths: DataPaths,
    config: config::BootstrapConfig,
    clock: Arc<dyn Clock>,
    logging_guard: logging::LoggingGuard,
) -> Result<AppState, NexusError> {
    let (db, init) = Db::open_initialized(paths.db_path(), DbOptions::default(), clock.as_ref())
        .await
        .map_err(NexusError::from)?;
    let db = Arc::new(db);

    let events = Arc::new(TauriEventSink::new(app));
    let settings = Arc::new(SettingsService::with_base_and_events(
        db.pool().clone(),
        clock.clone(),
        config.settings_base(),
        events.clone(),
    ));
    let effective = settings.get().await?;

    let data_fs = SafeFs::open(paths.root())?;
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringSecretStore::new("com.mws.nexus.launcher"));

    let download_events: Arc<dyn DownloadEventSink> = events.clone();
    let download_config = DownloadEngineConfig {
        max_attempts: u32::from(config.downloads.retries).saturating_add(1),
        connect_timeout: Duration::from_secs(config.network.connect_timeout_secs),
        read_timeout: Duration::from_secs(config.network.request_timeout_secs),
        max_redirects: config.network.max_redirects,
        ..DownloadEngineConfig::default()
    };
    let downloads = Arc::new(DownloadEngine::new(
        db.pool().clone(),
        clock.clone(),
        data_fs.clone(),
        UrlPolicy::for_nexus_config(&config.security),
        Some(download_events),
        download_config,
        usize::from(effective.downloads.max_concurrent),
        u64::from(effective.downloads.max_speed_kbps) * 1024,
    ));
    downloads.start().await?;

    Ok(AppState {
        db,
        settings,
        downloads,
        paths,
        config,
        clock,
        init,
        data_fs,
        secrets,
        _logging: Mutex::new(Some(logging_guard)),
    })
}

fn setup_error(error: impl std::fmt::Display) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(error.to_string()))
}

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::app::app_bootstrap,
            commands::app::db_health,
            commands::settings::settings_get,
            commands::settings::settings_update,
            commands::settings::settings_reset_group,
            commands::downloads::downloads_enqueue,
            commands::downloads::downloads_list,
            commands::downloads::downloads_get,
            commands::downloads::downloads_files,
            commands::downloads::downloads_pause,
            commands::downloads::downloads_resume,
            commands::downloads::downloads_cancel,
            commands::downloads::downloads_retry,
            commands::downloads::downloads_remove,
            commands::downloads::downloads_clear_finished,
            commands::downloads::downloads_errors,
            commands::downloads::downloads_limits,
        ])
        .setup(|app| {
            let paths = DataPaths::resolve_default().map_err(setup_error)?;
            paths.ensure_layout().map_err(setup_error)?;

            let loaded = config::load_or_init(&paths.config_path()).map_err(setup_error)?;
            if let Some(quarantined) = &loaded.quarantined {
                tracing::warn!(
                    path = %quarantined.display(),
                    "previous config.toml was quarantined"
                );
            }

            let logging_guard =
                logging::init(&paths, &loaded.config.logging).map_err(setup_error)?;
            let clock: Arc<dyn Clock> = Arc::new(SystemClock);
            let handle = app.handle().clone();

            let state = tauri::async_runtime::block_on(bootstrap(
                handle,
                paths,
                loaded.config,
                clock,
                logging_guard,
            ))
            .map_err(setup_error)?;

            tracing::info!(
                schema_version = state.init.health.schema_version,
                db_ok = state.init.health.ok,
                unclean_shutdown = state.init.recovery.unclean_shutdown,
                "NEXUS Launcher storage initialized"
            );

            app.manage(state);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build NEXUS Launcher window");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::Exit)
            && let Some(state) = app_handle.try_state::<AppState>()
        {
            let db = state.db.clone();
            let downloads = state.downloads.clone();
            tauri::async_runtime::block_on(async move {
                downloads.shutdown().await;
                if let Err(error) = db.shutdown().await {
                    error!(%error, "failed to close the database cleanly");
                }
            });
        }
    });
}
