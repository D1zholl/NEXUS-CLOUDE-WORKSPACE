#![recursion_limit = "512"]

pub mod ai;
pub mod api;
pub mod auth;
pub mod cloud;
pub mod config;
pub mod core;
pub mod database;
pub mod launcher;
pub mod middleware;
pub mod minecraft;
pub mod permissions;
pub mod realtime;
pub mod state;
pub mod storage;
pub mod system;
pub mod users;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::from_fn;
use axum::middleware::from_fn_with_state;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::core::error::ApiError;
use crate::state::AppState;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
img-src 'self' data: blob:; media-src 'self' blob:; \
style-src 'self' 'unsafe-inline'; script-src 'self'; \
connect-src 'self' ws: wss:; font-src 'self' data:; \
object-src 'none'; frame-src 'none'; frame-ancestors 'none'; base-uri 'self'; \
form-action 'self'; worker-src 'self' blob:";

/// Технический максимум тела для модулей с возобновляемыми загрузками.
/// Фактический размер чанка проверяется в обработчике по `limits.chunk_max_mb`.
const UPLOAD_BODY_LIMIT: usize = crate::config::MAX_CHUNK_BYTES as usize;

pub fn build_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/health", get(system::routes::health))
        .route("/ws", get(realtime::ws_handler))
        .nest("/auth", auth::routes::router())
        .nest("/users", users::routes::router())
        .nest("/roles", permissions::routes::roles_router())
        .nest("/permissions", permissions::routes::permissions_router())
        .nest("/me", system::settings::me_router())
        .nest(
            "/public/appearance",
            system::settings::public_router(),
        )
        .nest("/storage", storage::routes::router())
        .nest(
            "/minecraft",
            minecraft::routes::router().layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .nest("/launcher", launcher::routes::router())
        .nest("/admin/launcher", launcher::routes::admin_router())
        .nest("/monitoring", system::monitoring_routes::router())
        .nest("/jobs", system::jobs_routes::router())
        .nest(
            "/cloud",
            cloud::routes::router().layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .nest("/public/shares", cloud::routes::public_router())
        .nest("/system", system::routes::router())
        .nest("/ai", ai::routes::router().layer(DefaultBodyLimit::max(16 * 1024 * 1024)))
        .merge(system::routes::openapi_router())
        .fallback(api_not_found)
        // Отключение модуля блокирует его API. Гейт стоит после CSRF, чтобы
        // ошибка отключённого модуля не подменяла ошибку безопасности.
        .layer(from_fn_with_state(state.clone(), middleware::module_guard))
        .layer(from_fn_with_state(state.clone(), middleware::csrf_guard));

    let mut app: Router<Arc<AppState>> = Router::new().nest("/api/v1", api).merge(
        launcher::repository::router()
            .layer(from_fn_with_state(state.clone(), middleware::module_guard)),
    );

    app = match state.config.resolved_static_dir() {
        Some(dir) => {
            let index_path = dir.join("index.html");
            app.fallback_service(ServeDir::new(dir).fallback(tower::service_fn(
                move |req: axum::http::Request<axum::body::Body>| {
                    let index_path = index_path.clone();
                    async move {
                        let path = req.uri().path();
                        let missing_asset = path
                            .rsplit('/')
                            .next()
                            .is_some_and(|last| last.contains('.'));
                        let index_html = if missing_asset {
                            None
                        } else {
                            tokio::fs::read_to_string(&index_path).await.ok()
                        };
                        match index_html {
                            Some(html) => Ok((
                                [
                                    (header::CACHE_CONTROL, "no-cache"),
                                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                                ],
                                html,
                            )
                                .into_response()),
                            None => Ok(
                                (axum::http::StatusCode::NOT_FOUND, "Not Found").into_response()
                            ),
                        }
                    }
                },
            )))
        }
        None => app.fallback(frontend_missing),
    };

    if let Some(cors) = build_cors(&state) {
        app = app.layer(cors);
    }

    app.layer(CompressionLayer::new())
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(from_fn(middleware::request_context))
        .layer(from_fn_with_state(
            state.clone(),
            middleware::request_metrics,
        ))
        .with_state(state)
}

fn build_cors(state: &AppState) -> Option<CorsLayer> {
    let origins: Vec<HeaderValue> = state
        .config
        .security
        .allowed_origins
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect();
    if origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_credentials(true)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::CONTENT_TYPE,
                header::ACCEPT,
                header::AUTHORIZATION,
                HeaderName::from_static("x-csrf-token"),
                HeaderName::from_static("x-request-id"),
            ])
            .expose_headers([HeaderName::from_static("x-request-id")])
            .max_age(Duration::from_secs(600)),
    )
}

async fn api_not_found() -> ApiError {
    ApiError::not_found("Метод API не найден.")
}

async fn frontend_missing() -> Html<&'static str> {
    Html(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <title>NEXUS WORKSPACE</title></head><body style=\"background:#050b16;color:#edf5ff;\
         font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0\">\
         <div style=\"text-align:center;max-width:560px;padding:32px\">\
         <h1 style=\"font-size:20px\">NEXUS WORKSPACE</h1>\
         <p style=\"color:#91a5c0;line-height:1.6\">Frontend ещё не собран. Для разработки запустите \
         <code>npm run dev</code> в каталоге <code>frontend</code>, для production — \
         <code>npm run build</code>. После сборки эта страница будет заменена приложением.</p>\
         <p><a href=\"/api/v1/health\" style=\"color:#58d4ff\">Проверить API</a></p>\
         </div></body></html>",
    )
}
