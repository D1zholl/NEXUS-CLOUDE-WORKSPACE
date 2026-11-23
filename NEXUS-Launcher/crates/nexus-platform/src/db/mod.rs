//! SQLite connection, migrations, integrity checks and crash recovery.

mod error;

pub use error::DbError;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_core::{Clock, time::system_now_ms};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// All migrations are embedded at compile time. Files live in the workspace
/// `migrations/` directory and are named `NNNN_description.sql`.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub const MIGRATION_BACKUP_KEEP: usize = 2;

#[derive(Debug, Clone)]
pub struct DbOptions {
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub busy_timeout_ms: u64,
    pub create_if_missing: bool,
}

impl Default for DbOptions {
    fn default() -> Self {
        Self {
            max_connections: 8,
            acquire_timeout: Duration::from_secs(5),
            busy_timeout_ms: 5_000,
            create_if_missing: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: i64,
    pub to_version: i64,
    pub applied: Vec<String>,
    pub backup_path: Option<PathBuf>,
    pub integrity_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub unclean_shutdown: bool,
    pub paused_tasks: u64,
    pub paused_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbHealth {
    pub ok: bool,
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub schema_version: i64,
    pub supported_schema_version: i64,
    pub integrity: String,
    pub applied_migrations: i64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub migration: MigrationReport,
    pub recovery: RecoveryReport,
    pub health: DbHealth,
}

/// Owns the SQLite pool. Cloning is not implemented on purpose: application
/// state should hold a single `Db` (or an `Arc<Db>`) and close it explicitly.
pub struct Db {
    pool: SqlitePool,
    path: PathBuf,
}

impl Db {
    pub async fn open(path: impl AsRef<Path>, options: DbOptions) -> Result<Self, DbError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                DbError::Open(format!("cannot create {}: {error}", parent.display()))
            })?;
        }

        let connect_options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(options.create_if_missing)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_millis(options.busy_timeout_ms))
            .pragma("temp_store", "MEMORY")
            .pragma("journal_size_limit", "67108864")
            .pragma("trusted_schema", "OFF");

        let pool = SqlitePoolOptions::new()
            .max_connections(options.max_connections)
            .acquire_timeout(options.acquire_timeout)
            .connect_with(connect_options)
            .await
            .map_err(|error| DbError::Open(error.to_string()))?;

        let db = Self { pool, path };
        db.verify_pragmas().await?;
        Ok(db)
    }

    /// Opens the database and runs migrations plus startup recovery.
    pub async fn open_initialized(
        path: impl AsRef<Path>,
        options: DbOptions,
        clock: &dyn Clock,
    ) -> Result<(Self, InitReport), DbError> {
        let db = Self::open(path, options).await?;
        match db.initialize(clock).await {
            Ok(report) => Ok((db, report)),
            Err(error) => {
                let _ = db.pool.close().await;
                Err(error)
            }
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Checks that the file schema is not newer than the compiled migrations.
    pub async fn check_schema_version(&self) -> Result<i64, DbError> {
        let applied = self.applied_max_version().await?;
        let supported = compiled_max_version();
        if applied > supported {
            return Err(DbError::SchemaTooNew {
                found: applied,
                supported,
            });
        }
        Ok(applied)
    }

    /// Applies pending migrations. If user data already exists, a consistent
    /// `VACUUM INTO` backup is created first.
    pub async fn migrate(&self) -> Result<MigrationReport, DbError> {
        let applied_max = self.check_schema_version().await?;
        let supported = compiled_max_version();

        let pending: Vec<(i64, String)> = MIGRATOR
            .iter()
            .filter(|migration| migration.version > applied_max)
            .map(|migration| {
                (
                    migration.version,
                    format!("{:04}_{}", migration.version, migration.description),
                )
            })
            .collect();

        if pending.is_empty() {
            let integrity = self.quick_check().await?;
            if integrity != "ok" {
                return Err(DbError::Integrity(integrity));
            }
            return Ok(MigrationReport {
                from_version: applied_max,
                to_version: applied_max,
                applied: Vec::new(),
                backup_path: None,
                integrity_ok: true,
            });
        }

        let backup_path = if applied_max > 0 && self.path.is_file() {
            Some(self.create_pre_migration_backup(supported).await?)
        } else {
            None
        };

        MIGRATOR.run(&self.pool).await.map_err(|error| {
            DbError::Migration(format!(
                "failed to migrate from {applied_max} to {supported}: {error}"
            ))
        })?;

        let to_version = self.applied_max_version().await?;
        let integrity = self.quick_check().await?;
        if integrity != "ok" {
            return Err(DbError::Integrity(integrity));
        }

        Ok(MigrationReport {
            from_version: applied_max,
            to_version,
            applied: pending.into_iter().map(|(_, name)| name).collect(),
            backup_path,
            integrity_ok: true,
        })
    }

    /// Runs `migrate` and then repairs operational state left behind by a
    /// killed process (running downloads are paused, not silently completed).
    pub async fn initialize(&self, clock: &dyn Clock) -> Result<InitReport, DbError> {
        let migration = self.migrate().await?;
        let recovery = self.recover_after_unclean_shutdown(clock).await?;
        let now = clock.now_ms();
        self.set_meta("db.last_opened_by", env!("CARGO_PKG_VERSION"), now)
            .await?;
        self.set_meta("db.last_integrity_check", &now.to_string(), now)
            .await?;
        let health = self.health().await?;
        Ok(InitReport {
            migration,
            recovery,
            health,
        })
    }

    /// Detects an unclean shutdown through the `app_meta.db.clean_shutdown`
    /// flag. The flag is set to `0` while the launcher runs and to `1` by
    /// [`Db::close`].
    pub async fn recover_after_unclean_shutdown(
        &self,
        clock: &dyn Clock,
    ) -> Result<RecoveryReport, DbError> {
        let now = clock.now_ms();
        let previous: Option<String> =
            sqlx::query_scalar("SELECT value FROM app_meta WHERE key = 'db.clean_shutdown'")
                .fetch_optional(&self.pool)
                .await?;

        let unclean = previous.as_deref() != Some("1");
        if !unclean {
            self.set_meta("db.clean_shutdown", "0", now).await?;
            self.set_meta("db.last_open_at", &now.to_string(), now)
                .await?;
            return Ok(RecoveryReport {
                unclean_shutdown: false,
                paused_tasks: 0,
                paused_files: 0,
            });
        }

        let mut transaction = self.pool.begin().await?;
        let paused_tasks = sqlx::query(
            "UPDATE download_tasks
             SET status = 'paused', updated_at = ?
             WHERE status = 'running'",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let paused_files = sqlx::query(
            "UPDATE download_files
             SET status = 'paused', updated_at = ?
             WHERE status = 'running'",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let details = serde_json::json!({
            "pausedTasks": paused_tasks,
            "pausedFiles": paused_files,
        })
        .to_string();

        sqlx::query(
            "INSERT INTO activity_logs
                (at, kind, entity_kind, entity_id, summary, details_json, created_at, updated_at)
             VALUES (?, 'db.recovery', 'database', NULL, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind("Launcher recovered after an unclean shutdown.")
        .bind(details)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        upsert_meta(&mut transaction, "db.clean_shutdown", "0", now).await?;
        upsert_meta(&mut transaction, "db.last_open_at", &now.to_string(), now).await?;
        upsert_meta(
            &mut transaction,
            "db.last_recovered_at",
            &now.to_string(),
            now,
        )
        .await?;
        transaction.commit().await?;

        tracing::warn!(
            paused_tasks,
            paused_files,
            "previous launcher shutdown was unclean"
        );

        Ok(RecoveryReport {
            unclean_shutdown: true,
            paused_tasks,
            paused_files,
        })
    }

    pub async fn health(&self) -> Result<DbHealth, DbError> {
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await?;
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await?;
        let integrity = self.quick_check().await?;
        let schema_version = self.applied_max_version().await?;
        let supported_schema_version = compiled_max_version();
        let applied_migrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&self.pool)
            .await?;
        let size_bytes = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        Ok(DbHealth {
            ok: integrity == "ok" && foreign_keys == 1 && journal_mode.eq_ignore_ascii_case("wal"),
            journal_mode,
            foreign_keys: foreign_keys == 1,
            schema_version,
            supported_schema_version,
            integrity,
            applied_migrations,
            size_bytes,
        })
    }

    /// Full integrity check. This can take noticeable time on large databases;
    /// startup uses `quick_check` instead.
    pub async fn integrity_check(&self) -> Result<bool, DbError> {
        let result: String = sqlx::query_scalar("PRAGMA integrity_check(1)")
            .fetch_one(&self.pool)
            .await?;
        Ok(result == "ok")
    }

    /// Consistent SQLite backup using `VACUUM INTO`. Safe while WAL is active.
    pub async fn backup_to(&self, target: impl AsRef<Path>) -> Result<PathBuf, DbError> {
        let target = target.as_ref().to_path_buf();
        if target.exists() {
            return Err(DbError::Backup(format!(
                "backup target already exists: {}",
                target.display()
            )));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                DbError::Backup(format!("cannot create {}: {error}", parent.display()))
            })?;
        }

        let escaped = target.to_string_lossy().replace('\'', "''");
        sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{escaped}'")))
            .execute(&self.pool)
            .await
            .map_err(|error| DbError::Backup(error.to_string()))?;

        let metadata = fs::metadata(&target)
            .map_err(|error| DbError::Backup(format!("backup was not created: {error}")))?;
        if metadata.len() == 0 {
            return Err(DbError::Backup("backup file is empty".to_owned()));
        }
        Ok(target)
    }

    /// Creates a timestamped snapshot inside `directory`.
    pub async fn backup_snapshot(
        &self,
        directory: impl AsRef<Path>,
        tag: &str,
    ) -> Result<PathBuf, DbError> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory).map_err(|error| {
            DbError::Backup(format!("cannot create {}: {error}", directory.display()))
        })?;
        let tag: String = tag
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .collect();
        let target = directory.join(format!("launcher.db.{tag}-{}.bak", system_now_ms()));
        self.backup_to(target).await
    }

    /// Marks a clean shutdown and closes the pool without consuming `Db`.
    /// Used by the application exit handler, which holds an `Arc<Db>`.
    pub async fn shutdown(&self) -> Result<(), DbError> {
        let marked = self.mark_clean_shutdown().await;
        self.pool.close().await;
        marked
    }

    /// Marks a clean shutdown and closes the pool.
    pub async fn close(self) -> Result<(), DbError> {
        self.shutdown().await
    }

    pub async fn mark_clean_shutdown(&self) -> Result<(), DbError> {
        let now = system_now_ms();
        self.set_meta("db.clean_shutdown", "1", now).await?;
        self.set_meta("db.last_close_at", &now.to_string(), now)
            .await?;
        Ok(())
    }

    /// List of pre-migration backups, newest first.
    pub fn list_pre_migration_backups(&self) -> Result<Vec<PathBuf>, DbError> {
        let Some(parent) = self.path.parent() else {
            return Ok(Vec::new());
        };
        let prefix = format!(
            "{}.pre-migration-",
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "launcher.db".to_owned())
        );
        let mut entries: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(parent)
            .map_err(|error| DbError::Backup(error.to_string()))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(prefix.as_str())
            })
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        Ok(entries.into_iter().map(|(_, path)| path).collect())
    }

    /// Creates a consistent pre-migration backup and keeps the two newest
    /// copies. Public for the maintenance/diagnostics command; `migrate` uses it
    /// automatically.
    pub async fn create_pre_migration_backup(
        &self,
        target_version: i64,
    ) -> Result<PathBuf, DbError> {
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "launcher.db".to_owned());
        let target = self.path.with_file_name(format!(
            "{file_name}.pre-migration-v{target_version}-{}-{}.bak",
            system_now_ms(),
            uuid::Uuid::new_v4().simple()
        ));
        let path = self.backup_to(target).await?;
        self.prune_pre_migration_backups(MIGRATION_BACKUP_KEEP);
        Ok(path)
    }

    fn prune_pre_migration_backups(&self, keep: usize) {
        let Ok(backups) = self.list_pre_migration_backups() else {
            return;
        };
        for stale in backups.into_iter().skip(keep) {
            if let Err(error) = fs::remove_file(&stale) {
                tracing::warn!(path = %stale.display(), %error, "cannot remove old migration backup");
            }
        }
    }

    async fn applied_max_version(&self) -> Result<i64, DbError> {
        let table_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_one(&self.pool)
        .await?;

        if table_exists == 0 {
            return Ok(0);
        }

        let version: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
                .fetch_one(&self.pool)
                .await?;
        Ok(version)
    }

    async fn quick_check(&self) -> Result<String, DbError> {
        let result: String = sqlx::query_scalar("PRAGMA quick_check(1)")
            .fetch_one(&self.pool)
            .await?;
        Ok(result)
    }

    async fn verify_pragmas(&self) -> Result<(), DbError> {
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await?;
        if foreign_keys != 1 {
            return Err(DbError::Open(
                "foreign_keys pragma could not be enabled".to_owned(),
            ));
        }

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(DbError::Open(format!(
                "WAL journal mode is not available (got {journal_mode})"
            )));
        }
        Ok(())
    }

    async fn set_meta(&self, key: &str, value: &str, now: i64) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO app_meta (key, value, created_at, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn upsert_meta(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &str,
    value: &str,
    now: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO app_meta (key, value, created_at, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub fn compiled_max_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_embedded_and_ordered() {
        let versions: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
        assert!(versions.len() >= 3);
        assert_eq!(versions.first().copied(), Some(1));
        assert_eq!(versions.last().copied(), Some(compiled_max_version()));
        assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn schema_too_new_maps_to_stable_code() {
        let error = DbError::SchemaTooNew {
            found: 9,
            supported: 3,
        };
        assert_eq!(
            nexus_core::NexusError::from(error).code(),
            nexus_core::ErrorCode::DbSchemaTooNew
        );
    }
}
