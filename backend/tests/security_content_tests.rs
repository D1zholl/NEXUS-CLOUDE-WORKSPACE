//! I4: контентная безопасность предпросмотра для активных типов.
//!
//! H4 уже проверяет HTML/JS и подделку `Content-Type`; здесь — SVG, XHTML/XML,
//! Range-обход и заголовок `Content-Disposition` с потенциально опасным именем.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::uploads;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

async fn credentials(env: &TestEnv, user: Uuid) -> (String, String) {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    (
        format!("{SESSION_COOKIE}={token}"),
        csrf_token(&env.state.config.security.secret, &token),
    )
}

async fn upload(env: &TestEnv, user: Uuid, name: &str, content_type: &str, payload: &[u8]) -> Uuid {
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: name.to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: content_type.to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .unwrap();
    uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn preview(
    env: &TestEnv,
    cookie: &str,
    entry_id: Uuid,
    range: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/cloud/entries/{entry_id}/preview"))
        .header(header::COOKIE, cookie);
    if let Some(range) = range {
        builder = builder.header(header::RANGE, range);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn text(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn assert_inert_headers(headers: &HeaderMap) {
    assert_eq!(text(headers, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert_eq!(text(headers, header::X_FRAME_OPTIONS), "DENY");
    assert_eq!(
        text(headers, header::REFERRER_POLICY),
        "strict-origin-when-cross-origin"
    );
    let csp = text(
        headers,
        header::HeaderName::from_static("content-security-policy"),
    );
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(csp.contains("object-src 'none'"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
}

#[tokio::test]
async fn svg_preview_is_forced_to_download() {
    let env = TestEnv::new().await;
    env.add_volume("security-svg").await;
    let user = env.create_user("security-svg", "SecretPass1!").await;
    let (cookie, _) = credentials(&env, user).await;
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    let entry = upload(&env, user, "logo.svg", "image/svg+xml", svg).await;

    let (status, headers, body) = preview(&env, &cookie, entry, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text(&headers, header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment"));
    assert_inert_headers(&headers);
    assert_eq!(
        body,
        svg.as_slice(),
        "байты отдаются без изменений, но не inline"
    );
}

#[tokio::test]
async fn html_preview_with_range_does_not_bypass_attachment() {
    let env = TestEnv::new().await;
    env.add_volume("security-range").await;
    let user = env.create_user("security-range", "SecretPass1!").await;
    let (cookie, _) = credentials(&env, user).await;
    let html = b"<html><body><script>alert(document.cookie)</script></body></html>";
    let entry = upload(&env, user, "page.html", "text/html", html).await;

    let (status, headers, body) = preview(&env, &cookie, entry, Some("bytes=0-9")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        text(&headers, header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment"));
    assert_eq!(body, &html[..10]);
    assert_inert_headers(&headers);
}

#[tokio::test]
async fn xhtml_and_xml_are_not_served_as_active_documents() {
    let env = TestEnv::new().await;
    env.add_volume("security-xml").await;
    let user = env.create_user("security-xml", "SecretPass1!").await;
    let (cookie, _) = credentials(&env, user).await;

    let xhtml = upload(
        &env,
        user,
        "page.xhtml",
        "application/xhtml+xml",
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><script>alert(1)</script></html>",
    )
    .await;
    let (status, headers, _) = preview(&env, &cookie, xhtml, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text(&headers, header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment"));
    assert_inert_headers(&headers);

    let xml = upload(
        &env,
        user,
        "config.xml",
        "application/xml",
        b"<?xml version=\"1.0\"?><root><script>alert(1)</script></root>",
    )
    .await;
    let (status, headers, body) = preview(&env, &cookie, xml, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text(&headers, header::CONTENT_TYPE),
        "text/plain; charset=utf-8",
        "XML показывается только как текст"
    );
    assert!(text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"));
    assert!(!String::from_utf8_lossy(&body).contains("text/html"));
    assert_inert_headers(&headers);
}

#[tokio::test]
async fn content_disposition_sanitizes_dangerous_file_names() {
    let env = TestEnv::new().await;
    env.add_volume("security-disposition").await;
    let user = env
        .create_user("security-disposition", "SecretPass1!")
        .await;
    let (cookie, _) = credentials(&env, user).await;
    let name = "report; echo pwned (1).txt";
    let entry = upload(&env, user, name, "text/plain", b"safe").await;

    let (status, headers, _) = preview(&env, &cookie, entry, None).await;
    assert_eq!(status, StatusCode::OK);
    let disposition = text(&headers, header::CONTENT_DISPOSITION);
    assert!(!disposition.contains('\r'), "нет CR в заголовке");
    assert!(!disposition.contains('\n'), "нет LF в заголовке");
    assert!(
        !disposition.contains("; echo pwned"),
        "имя не инжектит параметры"
    );
    let ascii = disposition
        .split("filename=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("ASCII fallback");
    assert!(ascii
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ' | '(' | ')')));
    assert!(
        disposition.contains("filename*=UTF-8''report%3B%20echo%20pwned%20%281%29.txt"),
        "{disposition}"
    );
}

#[tokio::test]
async fn json_with_script_payload_stays_plain_text() {
    let env = TestEnv::new().await;
    env.add_volume("security-json").await;
    let user = env.create_user("security-json", "SecretPass1!").await;
    let (cookie, _) = credentials(&env, user).await;
    let payload = br#"{"html":"<script>alert(1)</script>"}"#;
    let entry = upload(&env, user, "data.json", "text/html", payload).await;

    let (status, headers, body) = preview(&env, &cookie, entry, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text(&headers, header::CONTENT_TYPE),
        "text/plain; charset=utf-8",
        "поддельный Content-Type не делает JSON активным"
    );
    assert_eq!(text(&headers, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert!(text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"));
    assert_eq!(body, payload.as_slice());
    assert_inert_headers(&headers);

    let details = nexus::cloud::files::entry_details(&env.pool, user, entry)
        .await
        .unwrap()
        .0;
    assert_eq!(details["preview"], Value::from("text/plain; charset=utf-8"));
}
