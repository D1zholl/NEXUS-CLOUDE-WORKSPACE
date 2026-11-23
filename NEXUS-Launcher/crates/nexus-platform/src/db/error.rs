use nexus_core::{ErrorCode, NexusError};

/// Low-level database error. `Display` is for logs only; user-facing text is
/// produced by the conversion into [`NexusError`].
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database cannot be opened: {0}")]
    Open(String),
    #[error("database is busy: {0}")]
    Busy(String),
    #[error("database migration failed: {0}")]
    Migration(String),
    #[error("schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error("database backup failed: {0}")]
    Backup(String),
    #[error("database query failed: {0}")]
    Query(String),
    #[error("database pool failed: {0}")]
    Pool(String),
}

impl DbError {
    pub fn from_sqlx(error: sqlx::Error) -> Self {
        match &error {
            sqlx::Error::PoolTimedOut => Self::Busy("pool acquire timeout".to_owned()),
            sqlx::Error::PoolClosed => Self::Pool("pool is closed".to_owned()),
            sqlx::Error::Database(database) => {
                let code = database.code();
                let transient = matches!(
                    code.as_deref(),
                    Some("5") | Some("6") | Some("SQLITE_BUSY") | Some("SQLITE_LOCKED")
                ) || database.message().contains("database is locked")
                    || database.message().contains("database table is locked");
                if transient {
                    Self::Busy(database.message().to_owned())
                } else {
                    Self::Query(database.message().to_owned())
                }
            }
            _ => Self::Query(error.to_string()),
        }
    }

    pub fn conflict_or_query(error: sqlx::Error, context: &str) -> NexusError {
        if let sqlx::Error::Database(database) = &error {
            let code = database.code();
            let constraint = matches!(
                code.as_deref(),
                Some("1555")
                    | Some("2067")
                    | Some("SQLITE_CONSTRAINT_PRIMARYKEY")
                    | Some("SQLITE_CONSTRAINT_UNIQUE")
            ) || database.message().contains("UNIQUE constraint failed")
                || database.message().contains("FOREIGN KEY constraint failed");
            if constraint {
                return NexusError::conflict(format!("{context}: {}", database.message()));
            }
        }
        NexusError::db_unavailable(format!("{context}: {}", DbError::from_sqlx(error)))
    }
}

impl From<DbError> for NexusError {
    fn from(value: DbError) -> Self {
        match value {
            DbError::Open(detail) | DbError::Pool(detail) => NexusError::db_unavailable(detail),
            DbError::Busy(detail) => NexusError::db_busy(detail),
            DbError::Migration(detail) => NexusError::db_migration(detail),
            DbError::SchemaTooNew { found, supported } => NexusError::db_too_new(format!(
                "database schema {found} is newer than supported {supported}"
            )),
            DbError::Integrity(detail) => NexusError::db_integrity(detail),
            DbError::Backup(detail) => NexusError::db_backup(detail),
            DbError::Query(detail) => NexusError::db_unavailable(detail),
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(value: sqlx::Error) -> Self {
        Self::from_sqlx(value)
    }
}

impl From<DbError> for ErrorCode {
    fn from(value: DbError) -> Self {
        NexusError::from(value).code()
    }
}

impl From<DbError> for nexus_core::ErrorDto {
    fn from(value: DbError) -> Self {
        NexusError::from(value).into()
    }
}
