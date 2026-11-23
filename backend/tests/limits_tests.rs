//! B3/B4: границы чанков и политика входа через HTTP-слой.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use chrono::Utc;
use common::TestEnv;
use nexus::auth::password::verify_password;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::uploads;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn put_setting(env: &TestEnv, key: &str, value: Value) {
    nexus::system::settings::apply_system_setting(
        &env.state,
        key,
        value,
        None,
        "test",
        "127.0.0.1",
    )
    .await
    .expect("настройка применяется");
}

/// Отправляет запрос через реальный роутер (middleware, guards, обработчики).
async fn send(env: &TestEnv, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = request;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("тело ответа читается");
    (status, headers, bytes.to_vec())
}

fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("запрос собирается")
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("ответ — JSON")
}

/// Создаёт сессию напрямую и возвращает cookie + CSRF для API-вызовов.
async fn api_credentials(env: &TestEnv, user: Uuid) -> (String, String) {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .expect("сессия создаётся");
    let cookie = format!("{SESSION_COOKIE}={token}");
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    (cookie, csrf)
}

async fn login(
    env: &TestEnv,
    username: &str,
    password: &str,
) -> (StatusCode, Option<(String, String)>, Vec<u8>) {
    let response = send(
        env,
        json_request(
            Method::POST,
            "/api/v1/auth/login",
            json!({ "username": username, "password": password }),
        ),
    )
    .await;
    let (status, headers, bytes) = response;
    let credentials = if status.is_success() {
        let session_cookie = headers
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(ToOwned::to_owned)
            .expect("cookie сессии");
        let body = parse(&bytes);
        let csrf = body["data"]["csrfToken"]
            .as_str()
            .expect("CSRF в ответе")
            .to_owned();
        Some((session_cookie, csrf))
    } else {
        None
    };
    (status, credentials, bytes)
}

#[tokio::test]
async fn chunk_limit_over_http_reports_details_and_recovers() {
    let env = TestEnv::new().await;
    env.add_volume("limits-chunk").await;
    let user = env.create_user("limitchunk", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    put_setting(&env, "limits.chunk_max_mb", json!(1)).await;

    // Эффективный лимит виден клиенту до загрузки.
    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cloud/config")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parse(&bytes)["data"]["chunkMaxBytes"],
        json!(1024 * 1024),
        "config отдаёт эффективный размер чанка"
    );

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "big.bin".to_owned(),
            parent_id: None,
            size: 64 * 1024 * 1024,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("сессия загрузки");

    let chunk_uri = format!("/api/v1/cloud/uploads/{}", session.id);
    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::PATCH)
            .uri(&chunk_uri)
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header("upload-offset", "0")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(vec![0u8; 2 * 1024 * 1024]))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let error = &parse(&bytes)["error"];
    assert_eq!(error["code"], "CHUNK_TOO_LARGE");
    assert_eq!(error["details"]["limitBytes"], json!(1024 * 1024));
    assert_eq!(error["details"]["receivedBytes"], json!(2 * 1024 * 1024));

    // Возврат лимита к 64 МБ: чанк ровно 64 МБ принимается.
    put_setting(&env, "limits.chunk_max_mb", json!(64)).await;
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "full.bin".to_owned(),
            parent_id: None,
            size: 64 * 1024 * 1024,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("вторая сессия загрузки");
    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::PATCH)
            .uri(format!("/api/v1/cloud/uploads/{}", session.id))
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header("upload-offset", "0")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(vec![0u8; 64 * 1024 * 1024]))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse(&bytes)["data"]["offset"], json!(64 * 1024 * 1024));
}

#[tokio::test]
async fn session_ttl_from_settings_applies_to_login() {
    let env = TestEnv::new().await;
    env.create_user("ttluser", "SecretPass1!").await;
    put_setting(&env, "security.session_ttl_hours", json!(1)).await;

    let (status, _, bytes) = login(&env, "ttluser", "SecretPass1!").await;
    assert_eq!(status, StatusCode::OK);
    let body = parse(&bytes);
    let expires_at = body["data"]["session"]["expiresAt"]
        .as_str()
        .expect("expiresAt в ответе")
        .parse::<chrono::DateTime<Utc>>()
        .expect("expiresAt в ISO-8601");
    let hours = (expires_at - Utc::now()).num_minutes() as f64 / 60.0;
    assert!(
        hours > 0.95 && hours <= 1.01,
        "сессия должна истекать примерно через час, получено {hours} ч"
    );
}

#[tokio::test]
async fn login_threshold_from_settings_locks_account() {
    let env = TestEnv::new().await;
    env.create_user("threshold", "SecretPass1!").await;
    put_setting(&env, "security.login_max_failures", json!(3)).await;

    for attempt in 1..=2 {
        let (status, _, bytes) = login(&env, "threshold", "wrong-password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "попытка {attempt}");
        assert_eq!(parse(&bytes)["error"]["code"], "INVALID_CREDENTIALS");
    }

    let (status, _, bytes) = login(&env, "threshold", "wrong-password").await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(parse(&bytes)["error"]["code"], "ACCOUNT_LOCKED");

    // Даже верный пароль не обходит серверную блокировку.
    let (status, _, bytes) = login(&env, "threshold", "SecretPass1!").await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(parse(&bytes)["error"]["code"], "ACCOUNT_LOCKED");
}

#[tokio::test]
async fn password_change_uses_effective_minimum_length() {
    let env = TestEnv::new().await;
    let user = env.create_user("passlength", "CurrentPass1!").await;
    put_setting(&env, "security.password_min_length", json!(8)).await;

    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/auth/setup-status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse(&bytes)["data"]["minPasswordLength"], json!(8));

    let (status, credentials, _) = login(&env, "passlength", "CurrentPass1!").await;
    assert_eq!(status, StatusCode::OK);
    let (cookie, csrf) = credentials.expect("сессия");

    let change = |new_password: &str| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/password")
            .header(header::COOKIE, cookie.clone())
            .header("x-csrf-token", csrf.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "currentPassword": "CurrentPass1!",
                    "newPassword": new_password,
                })
                .to_string(),
            ))
            .unwrap()
    };

    // 7 символов меньше эффективного минимума 8.
    let (status, _, bytes) = send(&env, change("Short1!")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(parse(&bytes)["error"]["code"], "VALIDATION_ERROR");

    // Ровно 8 символов — достаточно.
    let (status, _, bytes) = send(&env, change("Short1!A")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse(&bytes)["success"], json!(true));

    let hash: String = sqlx::query_scalar("select password_hash from users where id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert!(verify_password("Short1!A".to_owned(), hash).await);
}
