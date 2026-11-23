//! Типизированные ошибки AI-модуля. Коды согласованы с общим каталогом NEXUS.

use axum::http::StatusCode;

use crate::core::error::ApiError;

pub fn module_disabled() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "AI_MODULE_DISABLED",
        "AI-модуль отключён администратором.",
    )
}

pub fn provider_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "AI_PROVIDER_UNAVAILABLE",
        "Провайдер моделей недоступен. Проверьте PolyStrata и настройки провайдера.",
    )
}

pub fn runtime_offline() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "AI_RUNTIME_OFFLINE",
        "NEXUS AI Runtime не подключён. Запустите Runtime и повторите попытку.",
    )
}

pub fn runtime_revoked() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "AI_RUNTIME_REVOKED",
        "Доступ Runtime отозван администратором.",
    )
}

pub fn quota_exceeded(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "AI_QUOTA_EXCEEDED", message)
}

pub fn task_limit_reached() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "AI_TASK_LIMIT_REACHED",
        "Достигнут лимит одновременных AI-задач. Дождитесь завершения текущих задач.",
    )
}

pub fn task_not_cancellable() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "AI_TASK_NOT_CANCELLABLE",
        "Задача уже завершена и не может быть отменена.",
    )
}

pub fn approval_expired() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "AI_APPROVAL_EXPIRED",
        "Подтверждение истекло или уже использовано. Запросите действие повторно.",
    )
}

pub fn tool_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "AI_TOOL_NOT_ALLOWED",
        "Инструмент не разрешён для этого пользователя, workspace или Runtime.",
    )
}

pub fn invalid_arguments(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "AI_TOOL_INVALID_ARGUMENTS",
        message,
    )
}

pub fn protocol_unsupported() -> ApiError {
    ApiError::new(
        StatusCode::UPGRADE_REQUIRED,
        "AI_PROTOCOL_VERSION_UNSUPPORTED",
        "Версия протокола Runtime не поддерживается. Обновите NEXUS AI Runtime.",
    )
}

pub fn generation_in_progress() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "AI_GENERATION_IN_PROGRESS",
        "В этом диалоге уже выполняется генерация.",
    )
}

pub fn max_context() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "AI_CONTEXT_LIMIT_REACHED",
        "Сообщение или история превышают допустимый размер контекста.",
    )
}
