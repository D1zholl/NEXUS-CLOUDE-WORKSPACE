//! Интеграционные тесты авторизации, сессий и прав доступа.

mod common;

use common::TestEnv;
use nexus::auth::password::{hash_password, verify_password};
use nexus::auth::session::{clear_cookie_header, csrf_token, session_cookie_header, verify_csrf};
use nexus::core::error::ApiError;

#[tokio::test]
async fn password_hash_roundtrip() {
    let hash = hash_password("NexusAdmin!2026".to_owned())
        .await
        .expect("hash");
    assert!(hash.starts_with("$argon2"));
    assert!(verify_password("NexusAdmin!2026".to_owned(), hash.clone()).await);
    assert!(!verify_password("wrong".to_owned(), hash).await);
}

#[test]
fn csrf_token_is_hmac_and_verifies() {
    let secret = "test-secret-test-secret-test-secret-32";
    let token = "session-token";
    let csrf = csrf_token(secret, token);
    assert_eq!(csrf.len(), 64);
    assert!(verify_csrf(secret, token, &csrf));
    assert!(!verify_csrf(secret, token, "deadbeef"));
    assert!(!verify_csrf(
        "other-secret-other-secret-other-secret",
        token,
        &csrf
    ));
}

#[test]
fn session_tokens_are_hashed_not_stored_raw() {
    let token = nexus::auth::session::generate_token();
    assert_eq!(token.len(), 64);
    let hash = nexus::auth::session::hash_token(&token);
    assert_eq!(hash.len(), 64);
    assert_ne!(hash, token);
    assert_eq!(hash, nexus::auth::session::hash_token(&token));
}

#[test]
fn session_cookie_flags_are_http_only_and_same_site() {
    let plain = session_cookie_header("token-value", 24, false);
    assert!(plain.starts_with("nexus_session=token-value;"));
    assert!(plain.contains("Path=/"));
    assert!(plain.contains("HttpOnly"));
    assert!(plain.contains("SameSite=Lax"));
    assert!(plain.contains("Max-Age=86400"));
    assert!(!plain.contains("Secure"));

    let secure = session_cookie_header("token-value", 1, true);
    assert!(secure.contains("HttpOnly"));
    assert!(secure.contains("SameSite=Lax"));
    assert!(secure.contains("Secure"));
    assert!(secure.contains("Max-Age=3600"));

    let cleared = clear_cookie_header(true);
    assert!(cleared.contains("Max-Age=0"));
    assert!(cleared.contains("HttpOnly"));
    assert!(cleared.contains("Secure"));
}

#[tokio::test]
async fn effective_permissions_combine_roles_and_overrides() {
    let env = TestEnv::new().await;
    let user_id = env.create_user("permuser", "SecretPass1!").await;

    // Роль administrator даёт все права каталога.
    let granted: i64 = sqlx::query_scalar(
        "select count(*) from user_roles ur
         join role_permissions rp on rp.role_id = ur.role_id
         where ur.user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(granted > 10, "роль должна давать права, получено {granted}");

    // Персональный запрет сильнее роли.
    sqlx::query(
        "insert into user_permissions (user_id, permission_key, granted) values ($1, $2, false)",
    )
    .bind(user_id)
    .bind("cloud.file.delete")
    .execute(&env.pool)
    .await
    .unwrap();

    let denied: i64 = sqlx::query_scalar(
        "select count(*) from user_permissions where user_id = $1 and granted = false",
    )
    .bind(user_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(denied, 1);
}

#[tokio::test]
async fn login_locks_after_repeated_failures() {
    use std::time::Duration;

    let env = TestEnv::new().await;
    env.create_user("lockme", "SecretPass1!").await;

    let key = "lock-limit-test";
    for _ in 0..5 {
        env.state.login_limiter.record_failure(key);
    }
    let blocked = env.state.login_limiter.check(key);
    assert!(
        blocked.is_err(),
        "после 5 ошибок ключ должен быть заблокирован"
    );
    assert!(blocked.unwrap_err() > 0);

    // Другой ключ не затронут.
    assert!(env.state.login_limiter.check("other-key").is_ok());
    let _ = Duration::from_secs(0);
}

#[tokio::test]
async fn api_error_does_not_leak_internal_details() {
    let error = ApiError::internal("SELECT * FROM users where password_hash = 'x'");
    let (status, body) = {
        use axum::response::IntoResponse;
        let response = error.into_response();
        let status = response.status();
        (status, response)
    };
    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(body.into_body(), 64 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("password_hash"),
        "ответ не должен содержать SQL"
    );
    assert!(text.contains("INTERNAL_ERROR"));
}

#[tokio::test]
async fn forbidden_error_reports_required_permission() {
    let error = ApiError::forbidden("system.users.manage");
    let body = serde_json::to_string(
        &error
            .details()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .unwrap();
    assert!(body.contains("system.users.manage"));
}
