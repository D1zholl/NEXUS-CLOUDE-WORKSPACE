//! Идемпотентность опасных операций (PLAN §8.7).

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::uploads;
use nexus::core::idempotency::{self, Outcome};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

async fn upload_session(env: &TestEnv, user: Uuid, name: &str, payload: &[u8]) -> Uuid {
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
    .expect("создание сессии");
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        axum::body::Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк");
    session.id
}

#[tokio::test]
async fn repeated_complete_with_same_key_replays_result() {
    let env = TestEnv::new().await;
    env.add_volume("idem-vol").await;
    let user = env.create_user("idemuser", "SecretPass1!").await;
    let session_id = upload_session(&env, user, "once.txt", b"payload").await;

    let scope = "cloud.upload.complete";
    let key = "key-abc-1";
    let hash = idempotency::hash_request(&serde_json::json!({ "uploadId": session_id }));

    // Первая операция.
    match idempotency::begin(&env.pool, user, scope, key, &hash)
        .await
        .unwrap()
    {
        Outcome::Fresh => {}
        _ => panic!("первый ключ должен быть новым"),
    }
    let entry = uploads::complete_upload(
        &env.state,
        session_id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect("фиксация");
    let payload = serde_json::to_value(&entry).unwrap();
    idempotency::complete(&env.pool, user, scope, key, 200, &payload).await;

    // Повтор: возвращается сохранённый ответ, новой записи нет.
    match idempotency::begin(&env.pool, user, scope, key, &hash)
        .await
        .unwrap()
    {
        Outcome::Replay { status, body } => {
            assert_eq!(status, 200);
            assert_eq!(body["id"], payload["id"]);
        }
        _ => panic!("ожидался replay"),
    }

    let entries: i64 = sqlx::query_scalar("select count(*) from cloud_entries")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(entries, 1, "повтор не создаёт вторую запись");
}

#[tokio::test]
async fn same_key_with_different_body_is_rejected() {
    let env = TestEnv::new().await;
    let user = env.create_user("idemconflict", "SecretPass1!").await;
    let scope = "cloud.upload.complete";
    let key = "key-conflict";

    let first = idempotency::hash_request(&serde_json::json!({ "name": "a" }));
    let second = idempotency::hash_request(&serde_json::json!({ "name": "b" }));

    assert!(matches!(
        idempotency::begin(&env.pool, user, scope, key, &first)
            .await
            .unwrap(),
        Outcome::Fresh
    ));
    match idempotency::begin(&env.pool, user, scope, key, &second)
        .await
        .unwrap()
    {
        Outcome::Conflict(error) => {
            assert_eq!(error.code, "IDEMPOTENCY_KEY_REUSED");
            assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        }
        _ => panic!("ожидался конфликт хэша"),
    }
}

#[tokio::test]
async fn concurrent_in_progress_key_returns_conflict() {
    let env = TestEnv::new().await;
    let user = env.create_user("idemprogress", "SecretPass1!").await;
    let scope = "cloud.upload.complete";
    let key = "key-progress";
    let hash = idempotency::hash_request(&serde_json::json!({ "x": 1 }));

    assert!(matches!(
        idempotency::begin(&env.pool, user, scope, key, &hash)
            .await
            .unwrap(),
        Outcome::Fresh
    ));
    // Вторая попытка, пока операция не завершена.
    match idempotency::begin(&env.pool, user, scope, key, &hash)
        .await
        .unwrap()
    {
        Outcome::Conflict(error) => {
            assert_eq!(error.code, "IDEMPOTENCY_IN_PROGRESS");
            assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        }
        _ => panic!("ожидался 409"),
    }

    // Освобождение ключа позволяет повторить операцию.
    idempotency::release(&env.pool, user, scope, key).await;
    assert!(matches!(
        idempotency::begin(&env.pool, user, scope, key, &hash)
            .await
            .unwrap(),
        Outcome::Fresh
    ));
}

#[tokio::test]
async fn key_is_scoped_per_user() {
    let env = TestEnv::new().await;
    let alice = env.create_user("alice", "SecretPass1!").await;
    let bob = env.create_user("bob", "SecretPass1!").await;
    let scope = "cloud.upload.complete";
    let key = "shared-key";
    let hash = idempotency::hash_request(&serde_json::json!({ "x": 1 }));

    assert!(matches!(
        idempotency::begin(&env.pool, alice, scope, key, &hash)
            .await
            .unwrap(),
        Outcome::Fresh
    ));
    // Тот же ключ другого пользователя — независимая операция.
    assert!(matches!(
        idempotency::begin(&env.pool, bob, scope, key, &hash)
            .await
            .unwrap(),
        Outcome::Fresh
    ));
}

#[tokio::test]
async fn concurrent_begin_grants_exactly_one_operation() {
    let env = TestEnv::new().await;
    let user = env.create_user("idemrace", "SecretPass1!").await;
    let scope = "cloud.share.create";
    let key = "race-key";
    let hash = idempotency::hash_request(&serde_json::json!({ "x": 1 }));

    let (first, second) = tokio::join!(
        idempotency::begin(&env.pool, user, scope, key, &hash),
        idempotency::begin(&env.pool, user, scope, key, &hash)
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    let fresh = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Fresh))
        .count();
    let in_progress = outcomes
        .iter()
        .filter(|outcome| {
            matches!(outcome, Outcome::Conflict(error) if error.code == "IDEMPOTENCY_IN_PROGRESS")
        })
        .count();
    assert_eq!(fresh, 1, "ровно один запрос выполняет операцию");
    assert_eq!(in_progress, 1, "параллельный повтор получает 409");
}

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

async fn send_complete(
    env: &TestEnv,
    cookie: &str,
    csrf: &str,
    upload_id: Uuid,
    key: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/minecraft/uploads/{upload_id}/complete"))
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("тело ответа читается");
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("ответ — JSON")
    };
    (status, payload)
}

#[tokio::test]
async fn minecraft_complete_replays_and_rejects_other_request() {
    let env = TestEnv::new().await;
    env.add_volume("idem-minecraft").await;
    let user = env.create_user("idemmc", "SecretPass1!").await;
    let build = nexus::minecraft::builds::create(
        &env.pool,
        user,
        nexus::minecraft::builds::CreateBuildRequest {
            name: "Idempotent Build".to_owned(),
            slug: Some("idempotent-build".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "mods".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "fabric".to_owned(),
            loader_version: "0.15.0".to_owned(),
            java_major: Some(17),
            java_min: Some(17),
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();

    let session = nexus::minecraft::uploads::create_upload(
        &env.state,
        user,
        build.id,
        nexus::minecraft::uploads::CreateUploadRequest {
            path: "mods/first.jar".to_owned(),
            size: 4,
            category: Some("mods".to_owned()),
            side: None,
            required: Some(true),
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::minecraft::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"data".to_vec()),
    )
    .await
    .unwrap();

    let (cookie, csrf) = api_credentials(&env, user).await;
    let key = "minecraft-complete-key";
    let (status, first) = send_complete(&env, &cookie, &csrf, session.id, key).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["data"]["path"], "mods/first.jar");

    let (status, replay) = send_complete(&env, &cookie, &csrf, session.id, key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], first["data"]["id"]);
    let files: i64 = sqlx::query_scalar(
        "select count(*) from minecraft_build_files where build_id = $1 and kind = 'file'",
    )
    .bind(build.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(files, 1, "replay не создаёт второй файл");
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where event_type = 'minecraft.artifact.uploaded' and actor_id = $1",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    // Другая сессия с тем же ключом — другой хэш запроса, 422.
    let other = nexus::minecraft::uploads::create_upload(
        &env.state,
        user,
        build.id,
        nexus::minecraft::uploads::CreateUploadRequest {
            path: "mods/second.jar".to_owned(),
            size: 4,
            category: Some("mods".to_owned()),
            side: None,
            required: Some(true),
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .unwrap();
    let (status, failure) = send_complete(&env, &cookie, &csrf, other.id, key).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(failure["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
}

#[tokio::test]
async fn cleanup_removes_old_keys() {
    let env = TestEnv::new().await;
    let user = env.create_user("idemclean", "SecretPass1!").await;
    let hash = idempotency::hash_request(&serde_json::json!({}));
    idempotency::begin(&env.pool, user, "test", "old-key", &hash)
        .await
        .unwrap();
    sqlx::query(
        "update idempotency_keys set created_at = now() - interval '2 days' where key = 'old-key'",
    )
    .execute(&env.pool)
    .await
    .unwrap();

    let removed = idempotency::cleanup(&env.pool).await.unwrap();
    assert_eq!(removed, 1);
    let left: i64 = sqlx::query_scalar("select count(*) from idempotency_keys")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
