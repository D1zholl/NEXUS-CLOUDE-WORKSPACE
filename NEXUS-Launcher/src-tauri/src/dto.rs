use nexus_platform::db::{DbHealth, InitReport, RecoveryReport};
use serde::Serialize;

pub const IPC_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoDto {
    pub version: String,
    pub ipc_schema_version: u32,
    pub db_schema_version: i64,
    pub supported_db_schema_version: i64,
    pub data_dir_display: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub single_instance: bool,
    pub recovered_from_unclean_shutdown: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbHealthDto {
    pub ok: bool,
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub schema_version: i64,
    pub supported_schema_version: i64,
    pub integrity: String,
    pub applied_migrations: i64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryDto {
    pub unclean_shutdown: bool,
    pub paused_tasks: u64,
    pub paused_files: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationSummaryDto {
    pub from_version: i64,
    pub to_version: i64,
    pub applied: Vec<String>,
    pub backup_created: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapDto {
    pub app: AppInfoDto,
    pub settings: nexus_core::EffectiveSettings,
    pub database: DbHealthDto,
    pub recovery: RecoveryDto,
    pub migration: MigrationSummaryDto,
}

impl From<&DbHealth> for DbHealthDto {
    fn from(value: &DbHealth) -> Self {
        Self {
            ok: value.ok,
            journal_mode: value.journal_mode.clone(),
            foreign_keys: value.foreign_keys,
            schema_version: value.schema_version,
            supported_schema_version: value.supported_schema_version,
            integrity: value.integrity.clone(),
            applied_migrations: value.applied_migrations,
            size_bytes: value.size_bytes,
        }
    }
}

impl From<&RecoveryReport> for RecoveryDto {
    fn from(value: &RecoveryReport) -> Self {
        Self {
            unclean_shutdown: value.unclean_shutdown,
            paused_tasks: value.paused_tasks,
            paused_files: value.paused_files,
        }
    }
}

impl AppInfoDto {
    pub fn new(init: &InitReport, data_dir_display: String) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            ipc_schema_version: IPC_SCHEMA_VERSION,
            db_schema_version: init.health.schema_version,
            supported_db_schema_version: init.health.supported_schema_version,
            data_dir_display,
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            single_instance: true,
            recovered_from_unclean_shutdown: init.recovery.unclean_shutdown,
        }
    }
}

impl BootstrapDto {
    pub fn new(
        init: &InitReport,
        data_dir_display: String,
        settings: nexus_core::EffectiveSettings,
    ) -> Self {
        Self {
            app: AppInfoDto::new(init, data_dir_display),
            settings,
            database: DbHealthDto::from(&init.health),
            recovery: RecoveryDto::from(&init.recovery),
            migration: MigrationSummaryDto {
                from_version: init.migration.from_version,
                to_version: init.migration.to_version,
                applied: init.migration.applied.clone(),
                backup_created: init.migration.backup_path.is_some(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_schema_version_is_stable() {
        assert_eq!(IPC_SCHEMA_VERSION, 1);
    }

    #[test]
    fn dto_uses_camel_case() {
        let value = serde_json::to_value(DbHealthDto {
            ok: true,
            journal_mode: "wal".to_owned(),
            foreign_keys: true,
            schema_version: 3,
            supported_schema_version: 3,
            integrity: "ok".to_owned(),
            applied_migrations: 3,
            size_bytes: 1024,
        })
        .unwrap();
        assert!(value.get("journalMode").is_some());
        assert!(value.get("schemaVersion").is_some());
    }
}
