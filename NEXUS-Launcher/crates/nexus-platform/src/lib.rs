//! Local infrastructure for NEXUS Launcher.
//!
//! Responsibilities:
//! * one SQLite connection pool with WAL, foreign keys and busy timeout;
//! * versioned SQLx migrations, schema-version checks and pre-migration backup;
//! * startup recovery after an unclean shutdown;
//! * typed persistence of application settings;
//! * capability-based filesystem operations, archive extraction and SHA-256;
//! * URL/SSRF policy and a bounded HTTP client;
//! * secret-store abstraction (tokens never enter SQLite);
//! * file-based logging with redaction-friendly defaults.

pub mod db;
pub mod download;
pub mod logging;
pub mod repositories;
pub mod security;
pub mod settings;

pub use db::{Db, DbError, DbHealth, DbOptions, InitReport, MigrationReport, RecoveryReport};
pub use nexus_core::SettingsChange;
pub use settings::{SettingsEventSink, SettingsService};
