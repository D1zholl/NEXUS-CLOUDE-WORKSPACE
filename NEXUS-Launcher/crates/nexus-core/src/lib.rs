//! Core primitives shared by every NEXUS Launcher crate.
//!
//! This crate is deliberately free of Tauri, SQLx and HTTP dependencies. It
//! defines stable identifiers, the error envelope, data paths, bootstrap
//! configuration (TOML) and the typed application settings model.

pub mod config;
pub mod error;
pub mod ids;
pub mod paths;
pub mod settings;
pub mod time;

pub use error::{ErrorCode, ErrorDto, NexusError};
pub use ids::{
    AccountId, BackupId, BuildId, FileId, InstanceId, ModId, RuntimeId, ServerId, SourceId, TaskId,
    offline_player_uuid,
};
pub use paths::DataPaths;
pub use settings::{
    BackgroundQuality, DownloadSettings, DownloadSettingsPatch, EffectiveSettings, Language,
    LogLevel, LoggingSettings, LoggingSettingsPatch, PathSettings, PathSettingsPatch,
    SettingsChange, SettingsGroup, SettingsPatch, Theme, UiSettings, UiSettingsPatch,
};
pub use time::{Clock, FixedClock, SystemClock};
