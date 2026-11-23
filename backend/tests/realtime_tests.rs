//! Realtime-канал: адресация событий, счётчик соединений, лимит (PLAN §11).

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use common::TestEnv;
use futures_util::SinkExt;
use futures_util::StreamExt;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::realtime::{Event, Hub, MAX_CONNECTIONS_PER_USER};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn user_events_are_addressed_to_owner() {
    let hub = Hub::new();
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();
    let mut rx = hub.subscribe();

    hub.publish_user(
        "upload.progress",
        alice,
        serde_json::json!({ "uploaded": 10 }),
    );

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("событие")
        .expect("канал открыт");
    assert_eq!(event.kind, "upload.progress");
    assert_eq!(event.user_id, Some(alice));
    assert_ne!(event.user_id, Some(bob));
    assert_eq!(event.payload["uploaded"], 10);
}

#[tokio::test]
async fn system_events_have_no_owner() {
    let hub = Hub::new();
    let mut rx = hub.subscribe();
    hub.publish_system("system.metrics.updated", serde_json::json!({ "cpu": 12.5 }));
    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("событие")
        .expect("канал открыт");
    assert_eq!(event.kind, "system.metrics.updated");
    assert!(event.user_id.is_none());
}

#[tokio::test]
async fn publish_without_subscribers_does_not_fail() {
    let hub = Hub::new();
    hub.publish(Event::system("test", serde_json::json!({})));
    // Никакой паники и блокировки — событие просто отброшено.
    assert_eq!(hub.active(), 0);
}

#[test]
fn connection_limit_is_documented() {
    const { assert!(MAX_CONNECTIONS_PER_USER >= 1) };
    assert_eq!(MAX_CONNECTIONS_PER_USER, 3);
}

#[tokio::test]
async fn active_connections_reported_in_health() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.realtime.active(), 0);
    // Метрика доступна через RequestMetrics и Hub.
    assert_eq!(env.state.requests.active_connections(), 0);
    assert_eq!(env.state.requests.active_downloads(), 0);
}

#[tokio::test]
async fn upload_progress_uses_real_bytes() {
    let env = TestEnv::new().await;
    env.add_volume("realtime-vol").await;
    let user = env.create_user("realtime", "SecretPass1!").await;

    let mut rx = env.state.realtime.subscribe();
    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "rt.bin".to_owned(),
            parent_id: None,
            size: 8,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        axum::body::Body::from(b"1234".to_vec()),
    )
    .await
    .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("событие")
        .expect("канал открыт");
    assert_eq!(event.kind, "upload.progress");
    assert_eq!(event.user_id, Some(user));
    assert_eq!(event.payload["uploaded"], 4, "реально принятые байты");
    assert_eq!(event.payload["size"], 8);
}

#[tokio::test]
async fn job_progress_reaches_completion() {
    let env = TestEnv::new().await;

    // Создаём фоновую задачу и выполняем её напрямую.
    let user = env.create_user("jobuser", "SecretPass1!").await;
    let job_id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, created_by)
         values ('system.storage.check', 'pending', '{}'::jsonb, true, $1)
         returning id",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();

    let mut rx = env.state.realtime.subscribe();
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    // Собираем события прогресса до завершения задачи.
    let mut saw_progress = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(event)) if event.kind == "job.progress" => {
                assert_eq!(event.payload["jobId"], job_id.to_string());
                saw_progress = true;
                if event.payload["progress"].as_f64() == Some(100.0) {
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(saw_progress, "прогресс задачи публикуется");

    let status: String = sqlx::query_scalar("select status from background_jobs where id = $1")
        .bind(job_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(status, "completed");
    let progress: f64 = sqlx::query_scalar("select progress from background_jobs where id = $1")
        .bind(job_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(progress, 100.0);
}

/* --------------------------- D1: upload.failed --------------------------- */

async fn wait_kind(
    rx: &mut broadcast::Receiver<Event>,
    kind: &str,
    wait: Duration,
) -> Option<Event> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(event)) if event.kind == kind => return Some(event),
            Ok(Ok(_)) => continue,
            _ => return None,
        }
    }
}

async fn upload_cloud_file(env: &TestEnv, user: Uuid, name: &str, payload: &[u8]) -> Uuid {
    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: name.to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("сессия загрузки");
    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк");
    nexus::cloud::uploads::complete_upload(
        &env.state,
        session.id,
        user,
        nexus::cloud::uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect("фиксация")
    .id
}

#[tokio::test]
async fn upload_failed_on_complete_error_reaches_owner() {
    let env = TestEnv::new().await;
    env.add_volume("fail-complete-vol").await;
    let user = env.create_user("failcomplete", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "half.bin".to_owned(),
            parent_id: None,
            size: 16,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    let mut rx = env.state.realtime.subscribe();
    // Данные не записаны — завершение возвращает UPLOAD_DATA_MISSING.
    let error = nexus::cloud::uploads::complete_upload(
        &env.state,
        session.id,
        user,
        nexus::cloud::uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect_err("завершение пустой сессии невозможно");
    assert_eq!(error.code, "UPLOAD_DATA_MISSING");

    let event = wait_kind(&mut rx, "upload.failed", Duration::from_secs(1))
        .await
        .expect("upload.failed");
    assert_eq!(event.user_id, Some(user), "событие адресовано владельцу");
    assert_eq!(event.payload["uploadId"], json!(session.id));
    assert_eq!(event.payload["entryName"], "half.bin");
    assert_eq!(event.payload["code"], "UPLOAD_DATA_MISSING");
    assert_eq!(event.payload["uploaded"], 0);
    assert_eq!(event.payload["size"], 16);
    let text = event.payload.to_string();
    assert!(!text.contains("root_path"), "без внутренних путей");
    assert!(!text.contains("select "), "без SQL");
}

#[tokio::test]
async fn upload_failed_on_abort_reaches_owner() {
    let env = TestEnv::new().await;
    env.add_volume("fail-abort-vol").await;
    let user = env.create_user("failabort", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "cancelled.bin".to_owned(),
            parent_id: None,
            size: 8,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"1234".to_vec()),
    )
    .await
    .unwrap();

    let mut rx = env.state.realtime.subscribe();
    nexus::cloud::uploads::abort_upload(&env.state, session.id, user)
        .await
        .unwrap();

    let event = wait_kind(&mut rx, "upload.failed", Duration::from_secs(1))
        .await
        .expect("upload.failed");
    assert_eq!(event.user_id, Some(user));
    assert_eq!(event.payload["code"], "UPLOAD_ABORTED");
    assert_eq!(event.payload["uploaded"], 4);
    assert_eq!(event.payload["size"], 8);
}

#[tokio::test]
async fn upload_failed_on_ttl_expiry_in_cleanup_job() {
    let env = TestEnv::new().await;
    env.add_volume("fail-ttl-vol").await;
    let user = env.create_user("failttl", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "expired.bin".to_owned(),
            parent_id: None,
            size: 10,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"12345".to_vec()),
    )
    .await
    .unwrap();
    sqlx::query("update upload_sessions set expires_at = now() - interval '1 hour' where id = $1")
        .bind(session.id)
        .execute(&env.pool)
        .await
        .unwrap();

    let mut rx = env.state.realtime.subscribe();
    let removed = nexus::cloud::uploads::cleanup_expired(&env.state)
        .await
        .unwrap();
    assert_eq!(removed, 1);

    let event = wait_kind(&mut rx, "upload.failed", Duration::from_secs(1))
        .await
        .expect("upload.failed");
    assert_eq!(event.user_id, Some(user));
    assert_eq!(event.payload["code"], "UPLOAD_EXPIRED");
    assert_eq!(event.payload["uploaded"], 5);
    assert_eq!(event.payload["size"], 10);

    let status: String = sqlx::query_scalar("select status from upload_sessions where id = $1")
        .bind(session.id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(status, "expired");
}

#[tokio::test]
async fn chunk_write_error_closes_session_and_publishes_failed() {
    let env = TestEnv::new().await;
    let volume = env.add_volume("fail-write-vol").await;
    let user = env.create_user("failwrite", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "broken.bin".to_owned(),
            parent_id: None,
            size: 4,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    let root: String = sqlx::query_scalar("select root_path from storage_volumes where id = $1")
        .bind(volume)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let root = std::path::PathBuf::from(root);
    let backup = root.with_extension("disabled");
    std::fs::rename(&root, &backup).unwrap();
    // На месте каталога тома теперь файл: подготовка staging невозможна.
    std::fs::write(&root, b"not a directory").unwrap();

    let mut rx = env.state.realtime.subscribe();
    let error = nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"data".to_vec()),
    )
    .await
    .expect_err("запись чанка невозможна");
    assert_eq!(error.code, "INTERNAL_ERROR");

    let event = wait_kind(&mut rx, "upload.failed", Duration::from_secs(1))
        .await
        .expect("upload.failed");
    assert_eq!(event.user_id, Some(user));
    assert_eq!(event.payload["code"], "INTERNAL_ERROR");
    let text = event.payload.to_string();
    assert!(!text.contains("staging"), "без внутренних путей");

    let status: String = sqlx::query_scalar("select status from upload_sessions where id = $1")
        .bind(session.id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(status, "aborted", "сессия закрыта после ошибки записи");

    let _ = std::fs::remove_file(&root);
    let _ = std::fs::rename(&backup, &root);
}

/* --------------------- D2: прогресс и завершение скачивания --------------------- */

#[tokio::test]
async fn download_progress_grows_and_reaches_100_percent() {
    let env = TestEnv::new().await;
    env.add_volume("download-progress-vol").await;
    let user = env.create_user("downloadprogress", "SecretPass1!").await;

    let payload: Vec<u8> = (0..512 * 1024u32)
        .map(|index| (index % 251) as u8)
        .collect();
    let entry_id = upload_cloud_file(&env, user, "progress.bin", &payload).await;

    let mut rx = env.state.realtime.subscribe();
    let headers = axum::http::HeaderMap::new();
    let response = nexus::cloud::files::stream_file_with_state(
        Some(&env.state),
        &env.state.db,
        user,
        entry_id,
        &headers,
        false,
    )
    .await
    .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(body.len(), payload.len());

    let mut transferred = Vec::new();
    let mut completed = None;
    for _ in 0..32 {
        let Some(event) = wait_kind_any(
            &mut rx,
            &["download.progress", "download.completed"],
            Duration::from_secs(1),
        )
        .await
        else {
            break;
        };
        match event.kind.as_str() {
            "download.progress" => {
                assert_eq!(event.user_id, Some(user));
                assert_eq!(event.payload["entryId"], json!(entry_id));
                assert_eq!(event.payload["name"], "progress.bin");
                assert_eq!(event.payload["size"], payload.len() as i64);
                transferred.push(event.payload["transferred"].as_i64().unwrap());
            }
            "download.completed" => {
                completed = Some(event);
                break;
            }
            _ => {}
        }
    }

    assert!(transferred.len() >= 2, "прогресс публикуется несколько раз");
    assert!(
        transferred.windows(2).all(|pair| pair[1] >= pair[0]),
        "transferred растёт: {transferred:?}"
    );
    let completed = completed.expect("download.completed");
    assert_eq!(completed.user_id, Some(user));
    assert_eq!(completed.payload["transferred"], payload.len() as i64);
    assert_eq!(completed.payload["percent"], 100.0);

    // Счётчик активных скачиваний уменьшается на Drop.
    for _ in 0..100 {
        if env.state.requests.active_downloads() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.requests.active_downloads(), 0);
}

async fn wait_kind_any(
    rx: &mut broadcast::Receiver<Event>,
    kinds: &[&str],
    wait: Duration,
) -> Option<Event> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(event)) if kinds.contains(&event.kind.as_str()) => return Some(event),
            Ok(Ok(_)) => continue,
            _ => return None,
        }
    }
}

#[tokio::test]
async fn download_failed_on_interrupted_stream() {
    use http_body_util::BodyExt;

    let env = TestEnv::new().await;
    env.add_volume("download-fail-vol").await;
    let user = env.create_user("downloadfail", "SecretPass1!").await;

    let payload: Vec<u8> = (0..1024 * 1024u32)
        .map(|index| (index % 251) as u8)
        .collect();
    let entry_id = upload_cloud_file(&env, user, "interrupted.bin", &payload).await;

    let mut rx = env.state.realtime.subscribe();
    let headers = axum::http::HeaderMap::new();
    let response = nexus::cloud::files::stream_file_with_state(
        Some(&env.state),
        &env.state.db,
        user,
        entry_id,
        &headers,
        false,
    )
    .await
    .unwrap();
    let mut body = response.into_body();
    // Читаем только первый кадр и обрываем поток.
    let frame = body.frame().await.expect("кадр").expect("данные");
    assert!(frame.data_ref().is_some_and(|data| !data.is_empty()));
    drop(body);

    let event = wait_kind(&mut rx, "download.failed", Duration::from_secs(1))
        .await
        .expect("download.failed");
    assert_eq!(event.user_id, Some(user));
    assert_eq!(event.payload["entryId"], json!(entry_id));
    assert_eq!(event.payload["code"], "DOWNLOAD_INTERRUPTED");
    assert!(
        event.payload["transferred"].as_i64().unwrap() < payload.len() as i64,
        "обрыв до полной передачи"
    );

    for _ in 0..100 {
        if env.state.requests.active_downloads() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.requests.active_downloads(), 0);
}

/* ------------------- D3: события томов и публикации сборок ------------------- */

#[tokio::test]
async fn storage_check_publishes_volume_state_change() {
    let env = TestEnv::new().await;
    let volume = env.add_volume("volume-state-vol").await;
    let user = env.create_user("volumestate", "SecretPass1!").await;

    let root: String = sqlx::query_scalar("select root_path from storage_volumes where id = $1")
        .bind(volume)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    let job_id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, created_by)
         values ('system.storage.check', 'pending', '{}'::jsonb, true, $1)
         returning id",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();

    let mut rx = env.state.realtime.subscribe();
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    let event = wait_kind(&mut rx, "storage.volume.updated", Duration::from_secs(2))
        .await
        .expect("storage.volume.updated");
    assert!(event.user_id.is_none(), "системное событие");
    assert_eq!(event.payload["volumeId"], json!(volume));
    assert_eq!(event.payload["state"], "offline");
    assert_eq!(event.payload["previousState"], "online");
}

#[tokio::test]
async fn storage_check_does_not_publish_without_state_change() {
    let env = TestEnv::new().await;
    env.add_volume("volume-same-vol").await;
    let user = env.create_user("volumesame", "SecretPass1!").await;

    let job_id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, created_by)
         values ('system.storage.check', 'pending', '{}'::jsonb, true, $1)
         returning id",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();

    let mut rx = env.state.realtime.subscribe();
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    assert!(
        wait_kind(
            &mut rx,
            "storage.volume.updated",
            Duration::from_millis(400)
        )
        .await
        .is_none(),
        "при том же состоянии событие не публикуется"
    );
}

#[tokio::test]
async fn build_publish_failure_is_published() {
    let env = TestEnv::new().await;
    env.add_volume("publish-fail-vol").await;
    let user = env.create_user("publishfail", "SecretPass1!").await;

    let build = nexus::minecraft::builds::create(
        &env.pool,
        user,
        nexus::minecraft::builds::CreateBuildRequest {
            name: "Publish Fail".to_owned(),
            slug: Some("publish-fail".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
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
    let release = nexus::minecraft::releases::create(
        &env.pool,
        user,
        build.id,
        nexus::minecraft::releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();

    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/minecraft/builds/{}/releases/{}/publish",
            build.id, release.id
        ))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"acceptWarnings":true}"#))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));

    let mut rx = env.state.realtime.subscribe();
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    // Пустая сборка не проходит валидацию — публикация отклонена.
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let event = wait_kind(&mut rx, "build.publish.failed", Duration::from_secs(1))
        .await
        .expect("build.publish.failed");
    assert!(event.user_id.is_none(), "системное событие");
    assert_eq!(event.payload["buildId"], json!(build.id));
    assert_eq!(event.payload["releaseId"], json!(release.id));
    assert!(
        !event.payload["code"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "код ошибки присутствует"
    );
    assert!(
        !event.payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("select "),
        "без SQL"
    );
}

/* -------------- Полнота каталога §11: успешные upload/build события -------------- */

#[tokio::test]
async fn upload_completed_is_published_to_owner() {
    let env = TestEnv::new().await;
    env.add_volume("upload-completed-vol").await;
    let user = env.create_user("uploadcompleted", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "complete.bin".to_owned(),
            parent_id: None,
            size: 4,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"data".to_vec()),
    )
    .await
    .unwrap();

    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/cloud/uploads/{}/complete", session.id))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"strategy":"auto"}"#))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_001))));

    let mut rx = env.state.realtime.subscribe();
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    assert_eq!(response.status(), StatusCode::OK);

    let event = wait_kind(&mut rx, "upload.completed", Duration::from_secs(1))
        .await
        .expect("upload.completed");
    assert_eq!(event.user_id, Some(user), "событие адресовано владельцу");
    assert_eq!(event.payload["uploadId"], json!(session.id));
    assert_eq!(event.payload["name"], "complete.bin");
    assert_eq!(event.payload["size"], 4);
    assert!(
        event.payload["entryId"].as_str().is_some(),
        "идентификатор созданной записи"
    );
}

#[tokio::test]
async fn build_validation_completed_is_published_as_system_event() {
    let env = TestEnv::new().await;
    env.add_volume("validation-event-vol").await;
    let user = env.create_user("validationevent", "SecretPass1!").await;

    let build = nexus::minecraft::builds::create(
        &env.pool,
        user,
        nexus::minecraft::builds::CreateBuildRequest {
            name: "Validation Event".to_owned(),
            slug: Some("validation-event".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
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

    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/minecraft/builds/{}/validate", build.id))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", csrf)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_002))));

    let mut rx = env.state.realtime.subscribe();
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    assert_eq!(response.status(), StatusCode::OK);

    let event = wait_kind(
        &mut rx,
        "build.validation.completed",
        Duration::from_secs(1),
    )
    .await
    .expect("build.validation.completed");
    assert!(event.user_id.is_none(), "системное событие");
    assert_eq!(event.payload["buildId"], json!(build.id));
    assert_eq!(event.payload["valid"], false, "пустая сборка невалидна");
    assert!(event.payload["errors"].as_i64().unwrap_or(0) > 0);
    assert!(event.payload["warnings"].as_i64().is_some());
}

#[tokio::test]
async fn build_published_is_published_as_system_event() {
    let env = TestEnv::new().await;
    env.add_volume("build-published-vol").await;
    let user = env.create_user("buildpublished", "SecretPass1!").await;

    let build = nexus::minecraft::builds::create(
        &env.pool,
        user,
        nexus::minecraft::builds::CreateBuildRequest {
            name: "Published Event".to_owned(),
            slug: Some("published-event".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
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

    let upload = nexus::minecraft::uploads::create_upload(
        &env.state,
        user,
        build.id,
        nexus::minecraft::uploads::CreateUploadRequest {
            path: "mods/a.jar".to_owned(),
            size: 7,
            category: Some("mods".to_owned()),
            side: Some("client".to_owned()),
            required: Some(true),
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::minecraft::uploads::append_stream(
        &env.state,
        upload.id,
        user,
        0,
        Body::from(b"payload".to_vec()),
    )
    .await
    .unwrap();
    nexus::minecraft::uploads::complete_upload(&env.state, upload.id, user)
        .await
        .unwrap();

    let release = nexus::minecraft::releases::create(
        &env.pool,
        user,
        build.id,
        nexus::minecraft::releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();

    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/minecraft/builds/{}/releases/{}/publish",
            build.id, release.id
        ))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"acceptWarnings":true}"#))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_003))));

    let mut rx = env.state.realtime.subscribe();
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    assert_eq!(response.status(), StatusCode::OK);

    let event = wait_kind(&mut rx, "build.published", Duration::from_secs(1))
        .await
        .expect("build.published");
    assert!(event.user_id.is_none(), "системное событие");
    assert_eq!(event.payload["buildId"], json!(build.id));
    assert_eq!(event.payload["releaseId"], json!(release.id));
    assert_eq!(event.payload["version"], "1.0.0");
}

/* ------------------------- WebSocket: живой сервер ------------------------- */

type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct TestServer {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(state: std::sync::Arc<nexus::state::AppState>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("порт");
        let addr = listener.local_addr().unwrap();
        let app = nexus::build_router(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, task }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn create_plain_user(env: &TestEnv, username: &str) -> Uuid {
    let hash = nexus::auth::password::hash_password("SecretPass1!".to_owned())
        .await
        .unwrap();
    sqlx::query_scalar(
        "insert into users (username, display_name, password_hash, status)
         values ($1, $1, $2, 'active') returning id",
    )
    .bind(username)
    .bind(hash)
    .fetch_one(&env.pool)
    .await
    .unwrap()
}

async fn session_cookie(env: &TestEnv, user: Uuid) -> String {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    format!("{SESSION_COOKIE}={token}")
}

async fn connect_client(server: &TestServer, cookie: &str) -> Client {
    let mut request = format!("ws://{}/api/v1/ws", server.addr)
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("cookie", cookie.parse().expect("cookie header"));
    let (socket, response) = connect_async(request).await.expect("websocket upgrade");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SWITCHING_PROTOCOLS
    );
    socket
}

async fn next_ws_json(socket: &mut Client, wait: Duration) -> Option<Value> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                return serde_json::from_str(text.as_str()).ok();
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => {
                return None;
            }
            Ok(Some(Ok(_))) => continue,
        }
    }
}

async fn next_ws_kind(socket: &mut Client, kind: &str, wait: Duration) -> Option<Value> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let value = next_ws_json(socket, left).await?;
        if value["kind"] == kind {
            return Some(value);
        }
    }
}

#[tokio::test]
async fn websocket_filters_events_by_owner_and_permission() {
    let env = TestEnv::new().await;
    let admin = env.create_user("wsadmin", "SecretPass1!").await;
    let plain = create_plain_user(&env, "wsplain").await;
    let server = TestServer::start(env.state.clone()).await;

    let admin_cookie = session_cookie(&env, admin).await;
    let plain_cookie = session_cookie(&env, plain).await;
    let mut admin_ws = connect_client(&server, &admin_cookie).await;
    let mut plain_ws = connect_client(&server, &plain_cookie).await;

    let admin_hello = next_ws_kind(&mut admin_ws, "connected", Duration::from_secs(2))
        .await
        .expect("admin connected");
    assert_eq!(admin_hello["system"], true);
    let plain_hello = next_ws_kind(&mut plain_ws, "connected", Duration::from_secs(2))
        .await
        .expect("plain connected");
    assert_eq!(plain_hello["system"], false);

    // Пользовательское событие приходит только владельцу.
    env.state.realtime.publish_user(
        "upload.progress",
        admin,
        json!({ "uploaded": 42, "size": 100 }),
    );
    let own = next_ws_kind(&mut admin_ws, "upload.progress", Duration::from_secs(2))
        .await
        .expect("своё событие");
    assert_eq!(own["payload"]["uploaded"], 42);
    assert!(
        next_ws_kind(&mut plain_ws, "upload.progress", Duration::from_millis(350))
            .await
            .is_none(),
        "чужое пользовательское событие не приходит"
    );

    // Системные события — только при system.monitoring.view.
    env.state
        .realtime
        .publish_system("system.metrics.updated", json!({ "cpu": 12.5 }));
    let system = next_ws_kind(
        &mut admin_ws,
        "system.metrics.updated",
        Duration::from_secs(2),
    )
    .await
    .expect("администратор получает системные события");
    assert!(system["userId"].is_null());
    assert!(
        next_ws_kind(
            &mut plain_ws,
            "system.metrics.updated",
            Duration::from_millis(350)
        )
        .await
        .is_none(),
        "без system.monitoring.view системные события не приходят"
    );
}

#[tokio::test]
async fn websocket_connection_counter_and_limit() {
    let env = TestEnv::new().await;
    let user = env.create_user("wscounter", "SecretPass1!").await;
    let server = TestServer::start(env.state.clone()).await;
    let cookie = session_cookie(&env, user).await;

    assert_eq!(env.state.realtime.active(), 0);

    // Счётчик уменьшается после отключения.
    let mut first = connect_client(&server, &cookie).await;
    let hello = next_ws_kind(&mut first, "connected", Duration::from_secs(2))
        .await
        .expect("hello");
    assert_eq!(hello["activeConnections"], 1);
    first.close(None).await.ok();
    drop(first);
    for _ in 0..200 {
        if env.state.realtime.active() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.realtime.active(), 0, "счётчик уменьшился");

    // Три соединения допустимы, четвёртое закрывает самое старое.
    let mut oldest = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut oldest, "connected", Duration::from_secs(2))
        .await
        .expect("hello 1");
    let mut second = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut second, "connected", Duration::from_secs(2))
        .await
        .expect("hello 2");
    let mut third = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut third, "connected", Duration::from_secs(2))
        .await
        .expect("hello 3");
    assert_eq!(env.state.realtime.active(), 3);

    let mut fourth = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut fourth, "connected", Duration::from_secs(2))
        .await
        .expect("hello 4");

    let closed = next_ws_kind(&mut oldest, "closed", Duration::from_secs(2))
        .await
        .expect("первое соединение закрыто");
    assert_eq!(closed["reason"], "connection_limit");
    assert_eq!(
        env.state.realtime.active(),
        MAX_CONNECTIONS_PER_USER as u64,
        "лимит соединений"
    );

    second.close(None).await.ok();
    third.close(None).await.ok();
    fourth.close(None).await.ok();
}

/* ------------------------------------------------------------------------- */
/* I4: отказ без сессии, ping/pong и независимость вкладок.                  */
/* ------------------------------------------------------------------------- */

async fn expect_ws_rejected(server: &TestServer, cookie: Option<&str>) -> StatusCode {
    let mut request = format!("ws://{}/api/v1/ws", server.addr)
        .into_client_request()
        .expect("ws request");
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert("cookie", cookie.parse().expect("cookie header"));
    }
    match connect_async(request).await {
        Err(WsError::Http(response)) => response.status(),
        Ok((mut socket, response)) => {
            socket.close(None).await.ok();
            response.status()
        }
        Err(error) => panic!("неожиданная ошибка websocket: {error}"),
    }
}

async fn wait_active(env: &TestEnv, expected: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if env.state.realtime.active() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "активных соединений не {expected}, а {}",
            env.state.realtime.active()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn next_pong(socket: &mut Client, wait: Duration) -> Option<Vec<u8>> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, socket.next()).await {
            Ok(Some(Ok(Message::Pong(payload)))) => return Some(payload.to_vec()),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) | Err(_) => return None,
        }
    }
}

#[tokio::test]
async fn websocket_without_session_is_rejected_before_upgrade() {
    let env = TestEnv::new().await;
    let server = TestServer::start(env.state.clone()).await;

    assert_eq!(
        expect_ws_rejected(&server, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        env.state.realtime.active(),
        0,
        "соединение не зарегистрировано"
    );
}

#[tokio::test]
async fn websocket_with_invalid_session_is_rejected_before_upgrade() {
    let env = TestEnv::new().await;
    let server = TestServer::start(env.state.clone()).await;

    assert_eq!(
        expect_ws_rejected(&server, Some("nexus_session=not-a-real-token")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(env.state.realtime.active(), 0);
}

#[tokio::test]
async fn websocket_answers_ping_with_pong_and_stays_usable() {
    let env = TestEnv::new().await;
    let user = env.create_user("wsping", "SecretPass1!").await;
    let server = TestServer::start(env.state.clone()).await;
    let cookie = session_cookie(&env, user).await;

    let mut socket = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut socket, "connected", Duration::from_secs(2))
        .await
        .expect("hello");

    socket
        .send(Message::Ping(b"probe".to_vec().into()))
        .await
        .expect("ping отправлен");
    assert_eq!(
        next_pong(&mut socket, Duration::from_secs(2))
            .await
            .as_deref(),
        Some(b"probe".as_slice())
    );

    // Соединение остаётся рабочим после ping/pong.
    env.state
        .realtime
        .publish_user("upload.progress", user, json!({ "uploaded": 7 }));
    let event = next_ws_kind(&mut socket, "upload.progress", Duration::from_secs(2))
        .await
        .expect("событие после ping");
    assert_eq!(event["payload"]["uploaded"], 7);
}

#[tokio::test]
async fn closing_one_tab_does_not_affect_another_tab() {
    let env = TestEnv::new().await;
    let user = env.create_user("wstabs", "SecretPass1!").await;
    let server = TestServer::start(env.state.clone()).await;
    let cookie = session_cookie(&env, user).await;

    let mut first = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut first, "connected", Duration::from_secs(2))
        .await
        .expect("hello 1");
    let mut second = connect_client(&server, &cookie).await;
    let _ = next_ws_kind(&mut second, "connected", Duration::from_secs(2))
        .await
        .expect("hello 2");
    assert_eq!(env.state.realtime.active(), 2);

    first.close(None).await.ok();
    drop(first);
    wait_active(&env, 1).await;

    env.state
        .realtime
        .publish_user("upload.progress", user, json!({ "uploaded": 99 }));
    let event = next_ws_kind(&mut second, "upload.progress", Duration::from_secs(2))
        .await
        .expect("вторая вкладка получает событие");
    assert_eq!(event["payload"]["uploaded"], 99);
    assert_eq!(env.state.realtime.active(), 1);
    second.close(None).await.ok();
}
