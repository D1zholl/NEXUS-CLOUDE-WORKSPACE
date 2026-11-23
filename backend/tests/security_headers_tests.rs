//! H2: финальные заголовки безопасности и CSP на всех ответах.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestEnv;
use tower::ServiceExt;

fn header_text(headers: &axum::http::HeaderMap, name: &'static str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn security_headers_and_final_csp_are_present_everywhere() {
    let env = TestEnv::new().await;
    let app = nexus::build_router(env.state.clone());

    for (uri, expected) in [
        ("/api/v1/health", StatusCode::OK),
        ("/api/v1/not-found", StatusCode::NOT_FOUND),
        ("/", StatusCode::OK),
    ] {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.expect("роутер отвечает");
        assert_eq!(response.status(), expected, "проверяем {uri}");

        let headers = response.headers();
        assert_eq!(
            header_text(headers, "x-content-type-options"),
            "nosniff",
            "X-Content-Type-Options на {uri}"
        );
        assert_eq!(
            header_text(headers, "x-frame-options"),
            "DENY",
            "X-Frame-Options на {uri}"
        );
        assert_eq!(
            header_text(headers, "referrer-policy"),
            "strict-origin-when-cross-origin",
            "Referrer-Policy на {uri}"
        );
        assert_eq!(
            header_text(headers, "permissions-policy"),
            "camera=(), microphone=(), geolocation=()",
            "Permissions-Policy на {uri}"
        );

        let csp = header_text(headers, "content-security-policy");
        for directive in [
            "default-src 'self'",
            "script-src 'self'",
            "object-src 'none'",
            "frame-src 'none'",
            "frame-ancestors 'none'",
            "worker-src 'self' blob:",
            "base-uri 'self'",
            "form-action 'self'",
        ] {
            assert!(
                csp.contains(directive),
                "CSP на {uri} содержит {directive}: {csp}"
            );
        }
        assert!(!csp.contains("unsafe-eval"), "CSP запрещает eval: {csp}");
        assert!(
            !csp.contains("script-src 'self' 'unsafe-inline'"),
            "CSP не разрешает inline-скрипты: {csp}"
        );
    }
}

#[tokio::test]
async fn error_responses_keep_security_headers() {
    let env = TestEnv::new().await;
    let app = nexus::build_router(env.state.clone());

    let request = Request::builder()
        .uri("/api/v1/cloud/entries/00000000-0000-0000-0000-000000000000")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.expect("роутер отвечает");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let headers = response.headers();
    assert_eq!(header_text(headers, "x-content-type-options"), "nosniff");
    assert_eq!(header_text(headers, "x-frame-options"), "DENY");
    assert!(!header_text(headers, "content-security-policy").is_empty());
    assert_eq!(header_text(headers, "content-type"), "application/json");
}
