//! Ограничение частоты запросов Launcher API (PLAN §7).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestEnv;
use nexus::auth::rate_limit::TokenBucketLimiter;
use nexus::launcher::{catalog, tokens};
use nexus::minecraft::{builds, releases, uploads};
use nexus::state::AppState;
use nexus::system::monitoring::MetricsService;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

async fn issue(env: &TestEnv) -> String {
    issue_as(env, "ratelimit").await
}

async fn issue_as(env: &TestEnv, username: &str) -> String {
    tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "test".to_owned(),
            username: username.to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: None,
            expires_in_hours: Some(24),
        },
    )
    .await
    .expect("токен")
    .token
}

fn state_with_limits(env: &TestEnv, rpm: u32, artifacts: u32, parallel: usize) -> Arc<AppState> {
    let mut config = env.state.config.clone();
    config.launcher.requests_per_minute = rpm;
    config.launcher.artifacts_per_minute = artifacts;
    config.launcher.max_parallel_downloads = parallel;
    AppState::new(env.pool.clone(), config, MetricsService::new())
}

async fn catalog_request(app: &axum::Router, token: &str) -> axum::response::Response {
    let request = Request::builder()
        .uri("/api/v1/launcher/catalog")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("запрос собирается");
    app.clone().oneshot(request).await.expect("роутер отвечает")
}

fn number_header(response: &axum::response::Response, name: &str) -> u64 {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("заголовок {name} отсутствует или не число"))
}

#[tokio::test]
async fn request_above_limit_returns_retry_after() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let token = issue(&env).await;
    let _ = token;

    // Ёмкость 3, пополнение 60/мин: четвёртый запрос отклоняется.
    let limiter = TokenBucketLimiter::new(3, 60);
    assert!(limiter.try_acquire("t1").is_ok());
    assert!(limiter.try_acquire("t1").is_ok());
    assert!(limiter.try_acquire("t1").is_ok());
    let retry = limiter.try_acquire("t1").expect_err("лимит исчерпан");
    assert!(retry >= 1, "Retry-After задан в секундах");
}

#[tokio::test]
async fn limits_are_isolated_per_token() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let limiter = TokenBucketLimiter::new(1, 60);
    assert!(limiter.try_acquire("token-a").is_ok());
    assert!(limiter.try_acquire("token-a").is_err());
    // Второй токен не затронут первым.
    assert!(limiter.try_acquire("token-b").is_ok());
}

#[tokio::test]
async fn window_refill_restores_access() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    // Очень быстрое пополнение: 60 000/мин = 1000/с.
    let limiter = TokenBucketLimiter::new(1, 60_000);
    assert!(limiter.try_acquire("key").is_ok());
    assert!(limiter.try_acquire("key").is_err());
    // Через 10 мс токен пополнится (1000/с * 0.01 ≈ 10 токенов, c ограничением ёмкости 1).
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(limiter.try_acquire("key").is_ok(), "окно восстановилось");
}

#[tokio::test]
async fn real_catalog_responds_within_limit() {
    let env = TestEnv::new().await;
    let viewer = env.create_user("ratelimit", "SecretPass1!").await;
    // Каталог пуст, но запрос выполняется без ошибок.
    let items = catalog::catalog(&env.pool, viewer).await.unwrap();
    assert!(items.is_empty());
}

#[tokio::test]
async fn sustained_overrun_is_tracked_for_audit() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let limiter = TokenBucketLimiter::new(1, 1);
    assert!(limiter.try_acquire("t").is_ok());
    for _ in 0..10 {
        let _ = limiter.try_acquire("t");
    }
    // 10 подряд превышений — повод для записи в журнал.
    assert!(limiter.overruns("t") >= 10);
}

/// F2: `AppState` строит лимитеры из `[launcher]`, а не из старых констант.
#[tokio::test]
async fn app_state_uses_configured_launcher_limits() {
    let env = TestEnv::new().await;
    let state = state_with_limits(&env, 7, 11, 3);
    assert_eq!(state.launcher_limiter.limit(), 7);
    assert_eq!(state.artifact_limiter.limit(), 11);
    assert_eq!(state.downloads.max_parallel(), 3);
}

/// F1: успешный ответ отдаёт `X-RateLimit-*`, остаток уменьшается, а запрос
/// сверх лимита — 429 с `Retry-After` и обнулённым остатком.
#[tokio::test]
async fn success_headers_decrease_and_exhaustion_returns_retry_after() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let token = issue(&env).await;
    let app = nexus::build_router(state_with_limits(&env, 3, 600, 8));

    let first = catalog_request(&app, &token).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(number_header(&first, "x-ratelimit-limit"), 3);
    assert_eq!(number_header(&first, "x-ratelimit-remaining"), 2);

    let second = catalog_request(&app, &token).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(number_header(&second, "x-ratelimit-remaining"), 1);

    let third = catalog_request(&app, &token).await;
    assert_eq!(third.status(), StatusCode::OK);
    assert_eq!(number_header(&third, "x-ratelimit-remaining"), 0);

    let limited = catalog_request(&app, &token).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(number_header(&limited, "x-ratelimit-limit"), 3);
    assert_eq!(number_header(&limited, "x-ratelimit-remaining"), 0);
    assert!(number_header(&limited, "retry-after") >= 1);
    assert!(number_header(&limited, "x-ratelimit-reset") >= 1);

    let body = axum::body::to_bytes(limited.into_body(), 64 * 1024)
        .await
        .expect("тело ошибки");
    assert!(String::from_utf8_lossy(&body).contains("RATE_LIMITED"));
}

/* ------------------------ F5: артефакты под лимитом ------------------------ */

async fn published_artifact(env: &TestEnv, username: &str) -> (Uuid, String) {
    let user = env.create_user(username, "SecretPass1!").await;
    env.add_volume("rate-artifacts").await;
    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: "Rate Pack".to_owned(),
            slug: Some("rate-pack".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
            visibility: "public".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: Some(17),
            java_min: Some(17),
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();
    let session = uploads::create_upload(
        &env.state,
        user,
        build.id,
        uploads::CreateUploadRequest {
            path: "mods/a.jar".to_owned(),
            size: 4,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"data".to_vec()),
    )
    .await
    .unwrap();
    uploads::complete_upload(&env.state, session.id, user)
        .await
        .unwrap();
    let release = releases::create(
        &env.pool,
        user,
        build.id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, user, "tester", build.id, release.id, true)
        .await
        .unwrap();

    let artifact_id: Uuid =
        sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
            .bind(release.id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    (artifact_id, issue_as(env, username).await)
}

fn artifact_request(artifact_id: Uuid, token: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/launcher/artifacts/{artifact_id}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("запрос собирается")
}

/// F5: скачивание артефакта отдаёт те же `X-RateLimit-*`, а сверх лимита —
/// 429 `RATE_LIMITED` с `Retry-After`.
#[tokio::test]
async fn artifact_headers_decrease_and_exhaustion_returns_retry_after() {
    let env = TestEnv::new().await;
    let (artifact_id, token) = published_artifact(&env, "artifactlimit").await;
    let app = nexus::build_router(state_with_limits(&env, 600, 2, 8));

    let first = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(number_header(&first, "x-ratelimit-limit"), 2);
    assert_eq!(number_header(&first, "x-ratelimit-remaining"), 1);
    assert!(first.headers().get("x-file-sha256").is_some());
    drop(first);

    let second = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(number_header(&second, "x-ratelimit-remaining"), 0);
    drop(second);

    let limited = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(number_header(&limited, "x-ratelimit-limit"), 2);
    assert_eq!(number_header(&limited, "x-ratelimit-remaining"), 0);
    assert!(number_header(&limited, "retry-after") >= 1);
    assert!(number_header(&limited, "x-ratelimit-reset") >= 1);
    let body = axum::body::to_bytes(limited.into_body(), 64 * 1024)
        .await
        .expect("тело ошибки");
    assert!(String::from_utf8_lossy(&body).contains("RATE_LIMITED"));
}

/// F5: квоты каталога и артефактов независимы — исчерпание одной не блокирует
/// другую.
#[tokio::test]
async fn catalog_and_artifact_limits_are_independent() {
    let env = TestEnv::new().await;
    let (artifact_id, token) = published_artifact(&env, "separatelimit").await;
    let app = nexus::build_router(state_with_limits(&env, 1, 1, 8));

    // Исчерпываем лимит каталога.
    let catalog_first = catalog_request(&app, &token).await;
    assert_eq!(catalog_first.status(), StatusCode::OK);
    let catalog_second = catalog_request(&app, &token).await;
    assert_eq!(catalog_second.status(), StatusCode::TOO_MANY_REQUESTS);

    // Артефакт по-прежнему доступен: у него своя корзина.
    let artifact_first = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(artifact_first.status(), StatusCode::OK);
    drop(artifact_first);

    let artifact_second = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(artifact_second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(number_header(&artifact_second, "retry-after") >= 1);
}

/// F5: при исчерпании параллельных скачиваний ответ 503 содержит `Retry-After`,
/// а слот освобождается после завершения потока.
#[tokio::test]
async fn parallel_download_limit_returns_retry_after_and_recovers() {
    let env = TestEnv::new().await;
    let (artifact_id, token) = published_artifact(&env, "parallellimit").await;
    let app = nexus::build_router(state_with_limits(&env, 600, 600, 1));

    let held = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(held.status(), StatusCode::OK);

    let limited = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(limited.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(number_header(&limited, "retry-after") >= 1);
    let body = axum::body::to_bytes(limited.into_body(), 64 * 1024)
        .await
        .expect("тело ошибки");
    assert!(String::from_utf8_lossy(&body).contains("DOWNLOAD_LIMIT_REACHED"));

    drop(held);
    let again = app
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(again.status(), StatusCode::OK);
}

/// Приёмка F1: после 120 успешных запросов 121-й получает 429 + `Retry-After`.
///
/// Остальные 119 токенов корзины снимаются напрямую через тот же лимитер,
/// чтобы тест не зависел от скорости 121 HTTP-запроса и пополнения корзины.
#[tokio::test]
async fn request_121_is_rate_limited_with_retry_after() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let token = issue(&env).await;
    let app = nexus::build_router(env.state.clone());

    let first = catalog_request(&app, &token).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(number_header(&first, "x-ratelimit-limit"), 120);
    assert_eq!(number_header(&first, "x-ratelimit-remaining"), 119);

    let token_id: Uuid = sqlx::query_scalar("select id from access_tokens limit 1")
        .fetch_one(&env.pool)
        .await
        .expect("токен в БД");
    let key = token_id.to_string();
    for _ in 0..119 {
        env.state
            .launcher_limiter
            .try_acquire(&key)
            .expect("очередной запрос в пределах лимита");
    }

    let limited = catalog_request(&app, &token).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(number_header(&limited, "x-ratelimit-limit"), 120);
    assert_eq!(number_header(&limited, "x-ratelimit-remaining"), 0);
    let retry_after = number_header(&limited, "retry-after");
    assert!(retry_after >= 1, "Retry-After задан в секундах");
    assert!(number_header(&limited, "x-ratelimit-reset") >= 1);
}

/* ------------------------------------------------------------------------- */
/* I4: отказ до учёта квоты и изоляция HTTP-лимитов по строке токена.        */
/* ------------------------------------------------------------------------- */

async fn token_with_scopes(env: &TestEnv, scopes: Vec<&str>) -> String {
    tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "scoped".to_owned(),
            username: "ratelimit".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: Some(scopes.into_iter().map(ToOwned::to_owned).collect()),
            expires_in_hours: Some(24),
        },
    )
    .await
    .expect("токен")
    .token
}

async fn request_with_token(
    app: &axum::Router,
    uri: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("ответ")
}

#[tokio::test]
async fn missing_token_does_not_consume_catalog_quota() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let token = issue(&env).await;
    let app = nexus::build_router(state_with_limits(&env, 2, 600, 8));

    let anonymous = request_with_token(&app, "/api/v1/launcher/catalog", None).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let first = catalog_request(&app, &token).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        number_header(&first, "x-ratelimit-remaining"),
        1,
        "401 не израсходовал квоту"
    );
}

#[tokio::test]
async fn artifact_scope_failure_does_not_consume_catalog_quota() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let token = token_with_scopes(&env, vec![tokens::SCOPE_CATALOG]).await;
    let app = nexus::build_router(state_with_limits(&env, 2, 600, 8));

    let denied = request_with_token(
        &app,
        "/api/v1/launcher/artifacts/00000000-0000-0000-0000-000000000000",
        Some(&token),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(denied.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["error"]["details"]["requiredScope"],
        "launcher.artifacts"
    );

    let catalog = catalog_request(&app, &token).await;
    assert_eq!(catalog.status(), StatusCode::OK);
    assert_eq!(
        number_header(&catalog, "x-ratelimit-remaining"),
        1,
        "403 по scope не израсходовал квоту каталога"
    );
}

#[tokio::test]
async fn http_rate_limit_is_isolated_between_tokens() {
    let env = TestEnv::new().await;
    env.create_user("ratelimit", "SecretPass1!").await;
    let first_token = issue(&env).await;
    let second_token = issue_as(&env, "ratelimit").await;
    let app = nexus::build_router(state_with_limits(&env, 1, 600, 8));

    let first = catalog_request(&app, &first_token).await;
    assert_eq!(first.status(), StatusCode::OK);
    let exhausted = catalog_request(&app, &first_token).await;
    assert_eq!(exhausted.status(), StatusCode::TOO_MANY_REQUESTS);

    let other = catalog_request(&app, &second_token).await;
    assert_eq!(
        other.status(),
        StatusCode::OK,
        "второй токен того же пользователя имеет собственное ведро"
    );
    assert_eq!(number_header(&other, "x-ratelimit-remaining"), 0);
}
