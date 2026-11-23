use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Stable machine-readable error codes.
///
/// Codes are part of the IPC contract and must not be renamed after a release.
/// Human-readable messages may change freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    InvalidPath,
    PathOutsideRoot,
    ReservedName,
    PathCollision,
    ConfigInvalid,
    ConfigWriteFailed,
    SettingsInvalid,
    SettingsReadOnly,
    SettingsWriteFailed,
    DbUnavailable,
    DbBusy,
    DbMigrationFailed,
    DbSchemaTooNew,
    DbIntegrity,
    DbBackupFailed,
    SecretStoreUnavailable,
    ArchiveInvalid,
    SymlinkRejected,
    ReparsePointRejected,
    ChecksumMismatch,
    FileTooLarge,
    QuotaExceeded,
    UrlInvalid,
    UrlBlocked,
    TlsRequired,
    RedirectRejected,
    ResponseTooLarge,
    NetworkTimeout,
    NotFound,
    Conflict,
    PermissionDenied,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::InvalidPath => "INVALID_PATH",
            Self::PathOutsideRoot => "PATH_OUTSIDE_ROOT",
            Self::ReservedName => "RESERVED_NAME",
            Self::PathCollision => "PATH_COLLISION",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ConfigWriteFailed => "CONFIG_WRITE_FAILED",
            Self::SettingsInvalid => "SETTINGS_INVALID",
            Self::SettingsReadOnly => "SETTINGS_READ_ONLY",
            Self::SettingsWriteFailed => "SETTINGS_WRITE_FAILED",
            Self::DbUnavailable => "DB_UNAVAILABLE",
            Self::DbBusy => "DB_BUSY",
            Self::DbMigrationFailed => "DB_MIGRATION_FAILED",
            Self::DbSchemaTooNew => "DB_SCHEMA_TOO_NEW",
            Self::DbIntegrity => "DB_INTEGRITY",
            Self::DbBackupFailed => "DB_BACKUP_FAILED",
            Self::SecretStoreUnavailable => "SECRET_STORE_UNAVAILABLE",
            Self::ArchiveInvalid => "ARCHIVE_INVALID",
            Self::SymlinkRejected => "SYMLINK_REJECTED",
            Self::ReparsePointRejected => "REPARSE_POINT_REJECTED",
            Self::ChecksumMismatch => "CHECKSUM_MISMATCH",
            Self::FileTooLarge => "FILE_TOO_LARGE",
            Self::QuotaExceeded => "QUOTA_EXCEEDED",
            Self::UrlInvalid => "URL_INVALID",
            Self::UrlBlocked => "URL_BLOCKED",
            Self::TlsRequired => "TLS_REQUIRED",
            Self::RedirectRejected => "REDIRECT_REJECTED",
            Self::ResponseTooLarge => "RESPONSE_TOO_LARGE",
            Self::NetworkTimeout => "NETWORK_TIMEOUT",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Domain error with a stable code and a safe user-facing message.
///
/// `detail` is intended for logs only. It is never serialized into [`ErrorDto`]
/// and must not contain secrets. Callers should log it through the diagnostics
/// layer, not return it to the frontend.
#[derive(Debug, Clone)]
pub struct NexusError {
    code: ErrorCode,
    message: &'static str,
    retryable: bool,
    detail: Option<String>,
    details: Option<Value>,
}

impl NexusError {
    pub fn new(code: ErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: false,
            detail: None,
            details: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub fn invalid_argument(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, "Некорректный аргумент запроса.").with_detail(detail)
    }

    pub fn invalid_path(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidPath, "Некорректный путь.").with_detail(detail)
    }

    pub fn config_invalid(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ConfigInvalid,
            "Файл конфигурации повреждён или имеет неподдерживаемую версию.",
        )
        .with_detail(detail)
    }

    pub fn config_write(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ConfigWriteFailed,
            "Не удалось сохранить конфигурацию.",
        )
        .with_detail(detail)
    }

    pub fn settings_invalid(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SettingsInvalid,
            "Некорректное значение настройки.",
        )
        .with_detail(detail)
    }

    pub fn settings_read_only(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SettingsReadOnly,
            "Эта настройка доступна только для чтения.",
        )
        .with_detail(detail)
    }

    pub fn settings_write(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SettingsWriteFailed,
            "Не удалось сохранить настройки.",
        )
        .with_detail(detail)
    }

    pub fn db_unavailable(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::DbUnavailable,
            "Локальная база данных недоступна.",
        )
        .with_detail(detail)
    }

    pub fn db_busy(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::DbBusy, "База данных занята другой операцией.")
            .with_detail(detail)
            .retryable(true)
    }

    pub fn db_migration(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::DbMigrationFailed,
            "Не удалось обновить схему локальной базы данных.",
        )
        .with_detail(detail)
    }

    pub fn db_too_new(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::DbSchemaTooNew,
            "База данных создана более новой версией лаунчера. Обновите приложение.",
        )
        .with_detail(detail)
    }

    pub fn db_integrity(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::DbIntegrity,
            "Обнаружено повреждение локальной базы данных.",
        )
        .with_detail(detail)
    }

    pub fn db_backup(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::DbBackupFailed,
            "Не удалось создать резервную копию базы данных.",
        )
        .with_detail(detail)
    }

    pub fn secret_store(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SecretStoreUnavailable,
            "Защищённое хранилище секретов недоступно.",
        )
        .with_detail(detail)
    }

    pub fn archive_invalid(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ArchiveInvalid,
            "Архив повреждён или содержит недопустимые элементы.",
        )
        .with_detail(detail)
    }

    pub fn symlink_rejected(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SymlinkRejected,
            "Символические ссылки в этой операции запрещены.",
        )
        .with_detail(detail)
    }

    pub fn reparse_rejected(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ReparsePointRejected,
            "Точки повторной обработки и junctions запрещены.",
        )
        .with_detail(detail)
    }

    pub fn checksum_mismatch(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ChecksumMismatch,
            "Контрольная сумма файла не совпадает.",
        )
        .with_detail(detail)
    }

    pub fn file_too_large(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::FileTooLarge, "Файл превышает допустимый размер.").with_detail(detail)
    }

    pub fn quota_exceeded(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::QuotaExceeded,
            "Превышен общий лимит размера операции.",
        )
        .with_detail(detail)
    }

    pub fn url_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::UrlInvalid, "Некорректный адрес.").with_detail(detail)
    }

    pub fn url_blocked(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::UrlBlocked,
            "Адрес запрещён политикой сетевой безопасности.",
        )
        .with_detail(detail)
    }

    pub fn tls_required(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::TlsRequired,
            "Для сетевых запросов требуется защищённое соединение HTTPS.",
        )
        .with_detail(detail)
    }

    pub fn redirect_rejected(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::RedirectRejected,
            "Перенаправление отклонено политикой безопасности.",
        )
        .with_detail(detail)
    }

    pub fn response_too_large(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::ResponseTooLarge,
            "Ответ сервера превышает допустимый размер.",
        )
        .with_detail(detail)
    }

    pub fn network_timeout(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::NetworkTimeout,
            "Сетевой запрос превысил таймаут.",
        )
        .with_detail(detail)
    }

    pub fn permission_denied(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::PermissionDenied,
            "Недостаточно прав для операции.",
        )
        .with_detail(detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, "Объект не найден.").with_detail(detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, "Конфликт состояния.").with_detail(detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, "Внутренняя ошибка лаунчера.").with_detail(detail)
    }
}

impl fmt::Display for NexusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NexusError {}

/// Serializable error envelope returned by every Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub retryable: bool,
    pub request_id: Uuid,
}

impl ErrorDto {
    pub fn from_error(error: &NexusError) -> Self {
        Self {
            code: error.code(),
            message: error.message.to_owned(),
            details: error.details.clone(),
            retryable: error.is_retryable(),
            request_id: Uuid::new_v4(),
        }
    }
}

impl From<&NexusError> for ErrorDto {
    fn from(value: &NexusError) -> Self {
        Self::from_error(value)
    }
}

impl From<NexusError> for ErrorDto {
    fn from(value: NexusError) -> Self {
        Self::from_error(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_screaming_snake_case() {
        assert_eq!(ErrorCode::DbSchemaTooNew.as_str(), "DB_SCHEMA_TOO_NEW");
        assert_eq!(ErrorCode::SettingsInvalid.as_str(), "SETTINGS_INVALID");
        let json = serde_json::to_string(&ErrorCode::DbBackupFailed).unwrap();
        assert_eq!(json, "\"DB_BACKUP_FAILED\"");
    }

    #[test]
    fn dto_does_not_expose_internal_detail() {
        let error = NexusError::db_integrity("SQLITE_ERROR: SELECT * FROM secret_table")
            .with_details(serde_json::json!({ "table": "instances" }));
        let dto = ErrorDto::from(&error);
        assert_eq!(dto.code, ErrorCode::DbIntegrity);
        assert!(!dto.message.contains("SELECT"));
        assert_eq!(
            dto.details,
            Some(serde_json::json!({ "table": "instances" }))
        );
        assert_ne!(dto.request_id, Uuid::nil());
    }

    #[test]
    fn detail_is_available_for_logs_only() {
        let error = NexusError::config_invalid("line 4: expected value");
        assert_eq!(error.detail(), Some("line 4: expected value"));
        assert!(!error.to_string().contains("line 4"));
    }
}
