use axum::Json;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub type ApiResult<T> = Result<T, ApiError>;

/// Единый тип ошибки API. Наружу отдаётся только безопасное сообщение,
/// технические детали пишутся в журнал.
///
/// Необязательные части (детали и заголовки) лежат в `Box`, чтобы `ApiResult`
/// оставался компактным: иначе каждый `Result` в горячем пути носил бы
/// крупную ошибку (clippy `result_large_err`).
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    extras: Option<Box<ApiErrorExtras>>,
}

#[derive(Debug, Default)]
struct ApiErrorExtras {
    details: Option<Value>,
    headers: HeaderMap,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            extras: None,
        }
    }

    fn extras_mut(&mut self) -> &mut ApiErrorExtras {
        self.extras
            .get_or_insert_with(|| Box::new(ApiErrorExtras::default()))
    }

    /// Детали ошибки (для API-клиентов и тестов).
    pub fn details(&self) -> Option<&Value> {
        self.extras
            .as_ref()
            .and_then(|extras| extras.details.as_ref())
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.extras_mut().details = Some(details);
        self
    }

    /// Добавляет заголовок к ответу об ошибке. Некорректные имена и значения
    /// игнорируются: ошибка API не должна превращаться в ошибку сборки ответа.
    pub fn with_header(mut self, name: &str, value: impl ToString) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value.to_string()),
        ) {
            self.extras_mut().headers.insert(name, value);
        }
        self
    }

    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.extras_mut().headers.extend(headers);
        self
    }

    pub fn unauthorized(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    pub fn session_expired() -> Self {
        Self::unauthorized(
            "SESSION_EXPIRED",
            "Сессия истекла. Войдите в систему снова.",
        )
    }

    pub fn invalid_credentials() -> Self {
        Self::unauthorized(
            "INVALID_CREDENTIALS",
            "Неверное имя пользователя или пароль.",
        )
    }

    pub fn forbidden(permission: &str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Недостаточно прав для выполнения операции.",
        )
        .with_details(json!({ "required_permission": permission }))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            message,
        )
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "Слишком много попыток. Повторите позже.",
        )
        .with_details(json!({ "retry_after_seconds": retry_after_secs }))
        .with_header("retry-after", retry_after_secs)
    }

    /// Временная недоступность из-за исчерпания серверного ресурса.
    pub fn unavailable(
        code: impl Into<String>,
        message: impl Into<String>,
        retry_after_secs: u64,
    ) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
            .with_details(json!({ "retry_after_seconds": retry_after_secs }))
            .with_header("retry-after", retry_after_secs)
    }

    pub fn insufficient_space() -> Self {
        Self::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "INSUFFICIENT_SPACE",
            "Недостаточно свободного места в выбранном хранилище.",
        )
    }

    pub fn csrf_invalid() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "CSRF_INVALID",
            "Запрос отклонён проверкой безопасности. Обновите страницу и повторите действие.",
        )
    }

    /// Внутренняя ошибка: детали только в лог, пользователю — общее сообщение.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "internal server error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "Внутренняя ошибка сервера. Попробуйте позже.",
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.status.as_u16(),
            self.code,
            self.message
        )
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let ApiError {
            status,
            code,
            message,
            extras,
        } = self;
        let (details, headers) = extras
            .map(|extras| (extras.details, extras.headers))
            .unwrap_or_default();
        let body = json!({
            "success": false,
            "error": {
                "code": code,
                "message": message,
                "details": details.unwrap_or(Value::Null),
            },
            // request_id подставляется middleware запроса
            "request_id": ""
        });
        let mut response = (status, Json(body)).into_response();
        if !headers.is_empty() {
            response.headers_mut().extend(headers);
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        Self::internal(value)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::internal(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_sets_retry_after_and_extra_headers() {
        let response = ApiError::rate_limited(7)
            .with_header("x-ratelimit-remaining", 0)
            .with_header("x-test", "yes")
            .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("7")
        );
        assert_eq!(
            response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );
        assert_eq!(
            response
                .headers()
                .get("x-test")
                .and_then(|value| value.to_str().ok()),
            Some("yes")
        );
    }

    #[test]
    fn unavailable_sets_retry_after_and_details() {
        let error = ApiError::unavailable("DOWNLOAD_LIMIT_REACHED", "Позже", 5);
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.details().unwrap()["retry_after_seconds"], 5);
        assert_eq!(
            error
                .into_response()
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("5")
        );
    }
}
