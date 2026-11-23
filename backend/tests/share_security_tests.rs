//! Безопасность публичных ссылок: тикеты, лимит скачиваний, rate-limit пароля.
//!
//! Тесты используют реальный роутер и PostgreSQL (`nexus_test`), как остальные
//! интеграционные тесты проекта.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::cloud::shares::{self, CreateShareRequest};
use nexus::cloud::uploads;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn body_from(bytes: Vec<u8>) -> Body {
    Body::from(bytes)
}

async fn upload_file(env: &TestEnv, user: Uuid, name: &str, payload: &[u8]) -> Uuid {
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: name.to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .expect("создание сессии загрузки");

    uploads::append_stream(&env.state, session.id, user, 0, body_from(payload.to_vec()))
        .await
        .expect("запись чанка");
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
    .expect("завершение загрузки")
    .id
}

async fn create_share(
    env: &TestEnv,
    user: Uuid,
    entry: Uuid,
    password: Option<&str>,
    max_downloads: Option<i32>,
) -> String {
    let (_, token) = shares::create_share(
        &env.state,
        user,
        entry,
        CreateShareRequest {
            expires_in_hours: Some(1),
            password: password.map(ToOwned::to_owned),
            max_downloads,
            auth_only: false,
        },
    )
    .await
    .expect("ссылка создаётся");
    token
}

async fn public_request(
    env: &TestEnv,
    method: Method,
    uri: &str,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("запрос собирается");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("тело читается")
        .to_vec();
    (status, headers, bytes)
}

async fn unlock(env: &TestEnv, token: &str, password: &str) -> (StatusCode, HeaderMap, Value) {
    let (status, headers, bytes) = public_request(
        env,
        Method::POST,
        &format!("/api/v1/public/shares/{token}/unlock"),
        Some(json!({ "password": password })),
        &[],
    )
    .await;
    let payload = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, payload)
}

async fn download_count(env: &TestEnv, token: &str) -> i32 {
    let token_hash = shares::hash_share_token(token);
    sqlx::query_scalar("select download_count from cloud_shares where token_hash = $1")
        .bind(token_hash)
        .fetch_one(&env.pool)
        .await
        .expect("ссылка существует")
}

#[tokio::test]
async fn ticket_allows_range_without_extra_download_count() {
    let env = TestEnv::new().await;
    env.add_volume("share-ticket").await;
    let user = env.create_user("share-ticket", "SecretPass1!").await;
    let entry = upload_file(&env, user, "ticket.txt", b"abcdefghij").await;
    let token = create_share(&env, user, entry, Some("share-pass"), Some(2)).await;

    let (status, _, payload) = unlock(&env, &token, "wrong").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(payload["error"]["code"], "SHARE_PASSWORD_INVALID");

    let (status, _, payload) = unlock(&env, &token, "share-pass").await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    let ticket = payload["data"]["ticket"]
        .as_str()
        .expect("тикет в ответе")
        .to_owned();
    assert_eq!(
        download_count(&env, &token).await,
        1,
        "unlock учитывает скачивание"
    );

    let (status, headers, bytes) = public_request(
        &env,
        Method::GET,
        &format!("/api/v1/public/shares/{token}/download?ticket={ticket}"),
        None,
        &[("range", "bytes=0-3")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(bytes, b"abcd");
    assert_eq!(
        download_count(&env, &token).await,
        1,
        "Range по тикету не увеличивает счётчик"
    );

    let (status, _, _) = unlock(&env, &token, "share-pass").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(download_count(&env, &token).await, 2);

    let (status, _, payload) = unlock(&env, &token, "share-pass").await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(payload["error"]["code"], "DOWNLOAD_LIMIT_REACHED");

    let (status, _, payload) = public_request(
        &env,
        Method::GET,
        &format!("/api/v1/public/shares/{token}/download"),
        None,
        &[("x-share-password", "share-pass")],
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "старый путь не обходит лимит");
    let payload: Value = serde_json::from_slice(&payload).expect("ответ — JSON");
    assert_eq!(payload["error"]["code"], "DOWNLOAD_LIMIT_REACHED");
}

#[tokio::test]
async fn password_failures_are_rate_limited_with_retry_after() {
    let env = TestEnv::new().await;
    env.add_volume("share-bruteforce").await;
    let user = env.create_user("share-bruteforce", "SecretPass1!").await;
    let entry = upload_file(&env, user, "brute.txt", b"secret").await;
    let token = create_share(&env, user, entry, Some("share-pass"), None).await;

    for attempt in 1..=5 {
        let (status, _, _) = unlock(&env, &token, "wrong").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "попытка {attempt}");
    }

    let (status, headers, payload) = unlock(&env, &token, "wrong").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(payload["error"]["code"], "RATE_LIMITED");
    assert!(
        headers.get("retry-after").is_some(),
        "Retry-After обязателен"
    );

    let (status, _, _) = unlock(&env, &token, "share-pass").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "верный пароль не снимает блокировку досрочно"
    );
}

#[tokio::test]
async fn ticket_is_invalidated_when_password_hash_changes() {
    let env = TestEnv::new().await;
    env.add_volume("share-ticket-password").await;
    let user = env
        .create_user("share-ticket-password", "SecretPass1!")
        .await;
    let entry = upload_file(&env, user, "rotate.txt", b"rotate").await;
    let token = create_share(&env, user, entry, Some("share-pass"), None).await;

    let (status, _, payload) = unlock(&env, &token, "share-pass").await;
    assert_eq!(status, StatusCode::OK);
    let ticket = payload["data"]["ticket"].as_str().unwrap().to_owned();

    let new_hash = nexus::auth::password::hash_password("share-pass-2".to_owned())
        .await
        .expect("хэш нового пароля");
    sqlx::query("update cloud_shares set password_hash = $2 where token_hash = $1")
        .bind(shares::hash_share_token(&token))
        .bind(new_hash)
        .execute(&env.pool)
        .await
        .expect("пароль ссылки обновлён");

    let (status, _, payload) = public_request(
        &env,
        Method::GET,
        &format!("/api/v1/public/shares/{token}/download?ticket={ticket}"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let payload: Value = serde_json::from_slice(&payload).expect("ответ — JSON");
    assert_eq!(payload["error"]["code"], "SHARE_TICKET_INVALID");
}

#[tokio::test]
async fn ticket_is_bound_to_its_share_token() {
    let env = TestEnv::new().await;
    env.add_volume("share-ticket-binding").await;
    let user = env
        .create_user("share-ticket-binding", "SecretPass1!")
        .await;
    let first_entry = upload_file(&env, user, "first.txt", b"first").await;
    let second_entry = upload_file(&env, user, "second.txt", b"second").await;
    let first = create_share(&env, user, first_entry, None, None).await;
    let second = create_share(&env, user, second_entry, None, None).await;

    let (status, _, payload) = unlock(&env, &first, "").await;
    assert_eq!(status, StatusCode::OK);
    let ticket = payload["data"]["ticket"].as_str().unwrap().to_owned();

    let (status, _, payload) = public_request(
        &env,
        Method::GET,
        &format!("/api/v1/public/shares/{second}/download?ticket={ticket}"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let payload: Value = serde_json::from_slice(&payload).expect("ответ — JSON");
    assert_eq!(payload["error"]["code"], "SHARE_TICKET_INVALID");
}
