use std::net::IpAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;
use uuid::Uuid;

use crate::auth::session::{CSRF_HEADER, SESSION_COOKIE, cookie_value, verify_csrf};
use crate::core::error::ApiError;
use crate::state::AppState;

/// Присваивает request id, возвращает его в заголовке и в теле ошибок.
pub async fn request_context(req: Request, next: Next) -> Response {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 128
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut response = next.run(req).await;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }

    if response.status().is_client_error() || response.status().is_server_error() {
        let is_json = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(false);
        if is_json {
            let (parts, body) = response.into_parts();
            if let Ok(bytes) = to_bytes(body, 256 * 1024).await {
                let patched = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .filter(|v| v.get("success") == Some(&Value::Bool(false)))
                    .map(|mut v| {
                        v["request_id"] = Value::String(request_id.clone());
                        v
                    });
                let body_bytes = patched
                    .and_then(|v| serde_json::to_vec(&v).ok())
                    .unwrap_or_else(|| bytes.to_vec());
                return Response::from_parts(parts, Body::from(body_bytes));
            }
            return Response::from_parts(parts, Body::empty());
        }
    }

    response
}

/// Проверка CSRF для всех изменяющих запросов API.
pub async fn csrf_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    if matches!(
        method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) || path.ends_with("/auth/login")
        || path.ends_with("/auth/setup-status")
        || path.ends_with("/health")
        || path.ends_with("/launcher/auth/token")
        || path.ends_with("/launcher/auth/revoke")
        // Runtime не имеет браузерной сессии: enrollment аутентифицируется
        // одноразовым кодом в теле запроса.
        || path.ends_with("/ai/runtime/enroll")
        // Публичная ссылка не имеет сессии панели: пароль проверяется в
        // `unlock`, CSRF-токена у анонимного посетителя быть не может.
        || (path.contains("/public/shares/") && path.ends_with("/unlock"))
    {
        return Ok(next.run(req).await);
    }

    let Some(session_token) = cookie_value(req.headers(), SESSION_COOKIE) else {
        return Err(ApiError::session_expired());
    };
    let Some(provided) = req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err(ApiError::csrf_invalid());
    };
    if !verify_csrf(&state.config.security.secret, &session_token, provided) {
        return Err(ApiError::csrf_invalid());
    }
    Ok(next.run(req).await)
}

pub fn client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<IpAddr>,
    trust_proxy: bool,
) -> String {
    if trust_proxy {
        if let Some(value) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(first) = value.split(',').next() {
                let first = first.trim();
                if !first.is_empty() {
                    return first.to_owned();
                }
            }
        }
        if let Some(value) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    peer.map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

pub fn is_success(status: StatusCode) -> bool {
    status.is_success()
}

/// Считает реальные показатели API: RPS, среднюю задержку и долю ошибок.
/// Дополнительно ведёт счётчик in-flight запросов («активные соединения»).
pub async fn request_metrics(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let started = std::time::Instant::now();
    state.requests.begin_request();
    let response = next.run(req).await;
    state.requests.end_request();
    let status = response.status();
    // Служебные опросы здоровья не искажают метрики приложения.
    state
        .requests
        .record(started.elapsed(), status.is_server_error());
    response
}

/// Определяет модуль по пути запроса.
///
/// Возвращает `None` для путей, которые обязаны работать всегда: авторизация,
/// системное управление, мониторинг, здоровье, публичные ссылки и realtime.
pub fn module_for_path(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("/api/v1").unwrap_or(path);
    // Репозиторий MyLauncher живёт вне /api/v1, но относится к minecraft.
    if rest == "/api/packs" || rest.starts_with("/api/packs/") || rest.starts_with("/api/files/") {
        return Some("minecraft");
    }
    let segment = rest.trim_start_matches('/').split('/').next().unwrap_or("");
    match segment {
        "cloud" => Some("cloud"),
        "ai" => Some("ai"),
        "minecraft" | "launcher" | "admin" => Some("minecraft"),
        _ => None,
    }
}

/// Серверный гейт отключённых модулей (PLAN §9).
///
/// Отключение модуля блокирует его API, а не только скрывает ссылку в интерфейсе.
pub async fn module_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_owned();
    if let Some(module) = module_for_path(&path) {
        if !state.module_enabled(module).await {
            // Лаунчеру не раскрываем причину: для внешнего клиента это 404.
            // Внутри `nest("/api/v1")` URI уже без префикса, поэтому проверяем
            // оба представления пути.
            if path.starts_with("/api/v1/launcher")
                || path.starts_with("/launcher")
                || path.starts_with("/api/packs")
                || path.starts_with("/api/files")
            {
                tracing::info!(module, path = %path, "запрос к отключённому модулю (launcher)");
                return Err(ApiError::not_found("Ресурс не найден."));
            }
            tracing::info!(module, path = %path, "запрос к отключённому модулю");
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "MODULE_DISABLED",
                "Модуль отключён администратором.",
            )
            .with_details(serde_json::json!({ "module": module })));
        }
    }
    Ok(next.run(req).await)
}
