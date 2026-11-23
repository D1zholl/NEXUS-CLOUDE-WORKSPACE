//! Устойчивость: отказ диска, перезапуск, скачивание и публичные ссылки E2E.

mod common;

use axum::body::Body;
use common::TestEnv;
use nexus::cloud::{files, service, shares, uploads};
use uuid::Uuid;

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
    .expect("сессия");
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк");
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
    .expect("фиксация")
    .id
}

#[tokio::test]
async fn offline_volume_blocks_upload_and_recovers() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("resilience-vol").await;
    let user = env.create_user("resilient", "SecretPass1!").await;

    // Удаляем каталог тома — том становится недоступным.
    let root: String = sqlx::query_scalar("select root_path from storage_volumes where id = $1")
        .bind(volume_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    let outcome = nexus::storage::service::check_volume(&env.pool, volume_id)
        .await
        .unwrap();
    assert_eq!(outcome.row.state, "offline");

    // Новая загрузка отклоняется: нет подходящего тома.
    let error = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "x.bin".to_owned(),
            parent_id: None,
            size: 10,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect_err("том отключён");
    assert_eq!(error.code, "NO_VOLUME_AVAILABLE");

    // Восстанавливаем каталог — том снова онлайн.
    std::fs::create_dir_all(&root).unwrap();
    let outcome = nexus::storage::service::check_volume(&env.pool, volume_id)
        .await
        .unwrap();
    assert_eq!(outcome.row.state, "online");
    assert!(upload(&env, user, "ok.bin", b"data").await.is_ok());
}

async fn upload(
    env: &TestEnv,
    user: Uuid,
    name: &str,
    payload: &[u8],
) -> Result<Uuid, nexus::core::error::ApiError> {
    Ok(upload_file(env, user, name, payload).await)
}

#[tokio::test]
async fn session_survives_new_app_state() {
    let env = TestEnv::new().await;
    env.add_volume("restart-vol").await;
    let user = env.create_user("restartuser", "SecretPass1!").await;

    // Создаём сессию и проверяем, что она валидна после «перезапуска»
    // (новый AppState на том же пуле и той же БД).
    let (token, _) = nexus::auth::session::create_session(&env.pool, user, 24, "127.0.0.1", "test")
        .await
        .unwrap();
    let state2 = nexus::state::AppState::new(
        env.pool.clone(),
        env.state.config.clone(),
        nexus::system::monitoring::MetricsService::new(),
    );
    let user2 = nexus::auth::session::load_user_for_token(&state2, &token)
        .await
        .expect("сессия валидна после перезапуска");
    assert_eq!(user2.id, user);

    // Объекты, созданные до перезапуска, читаются новым состоянием.
    let entry_id = upload_file(&env, user, "keep.txt", b"survives restart").await;
    let resolved = files::resolve_object(&state2.db, user, entry_id)
        .await
        .expect("объект читается");
    let bytes = tokio::fs::read(&resolved.location.absolute).await.unwrap();
    assert_eq!(bytes, b"survives restart");
}

#[tokio::test]
async fn download_returns_content_length_and_range() {
    let env = TestEnv::new().await;
    env.add_volume("download-vol").await;
    let user = env.create_user("downloade2e", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "range.txt", b"0123456789").await;

    let headers = axum::http::HeaderMap::new();
    let response = files::stream_file(&env.pool, user, entry_id, &headers, false)
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .unwrap(),
        "10"
    );

    let mut ranged = axum::http::HeaderMap::new();
    ranged.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=2-5"),
    );
    let response = files::stream_file(&env.pool, user, entry_id, &ranged, false)
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_RANGE)
            .unwrap(),
        "bytes 2-5/10"
    );
    let body = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(&body[..], b"2345");
}

#[tokio::test]
async fn public_share_full_cycle() {
    let env = TestEnv::new().await;
    env.add_volume("public-vol").await;
    let user = env.create_user("sharee2e", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "shared.bin", b"share bytes").await;

    let (_, token) = shares::create_share(
        &env.state,
        user,
        entry_id,
        shares::CreateShareRequest {
            expires_in_hours: Some(1),
            password: Some("pass1234".to_owned()),
            max_downloads: Some(1),
            auth_only: false,
        },
    )
    .await
    .unwrap();

    // Без пароля скачать нельзя.
    assert!(
        !shares::check_share_password(&env.pool, &token, "wrong")
            .await
            .unwrap()
    );
    assert!(
        shares::check_share_password(&env.pool, &token, "pass1234")
            .await
            .unwrap()
    );

    // Скачивание по ссылке: файл отдаётся с Range.
    let info = shares::resolve_public_share(&env.pool, &token)
        .await
        .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=0-4"),
    );
    let response = files::stream_file(&env.pool, info.owner_id, info.entry_id, &headers, false)
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);

    // Исчерпание лимита делает ссылку недоступной.
    shares::register_download(&env.pool, info.share_id)
        .await
        .unwrap();
    assert!(
        shares::resolve_public_share(&env.pool, &token)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn interrupted_upload_resumes_from_server_offset() {
    let env = TestEnv::new().await;
    env.add_volume("resume-vol").await;
    let user = env.create_user("resumeuser", "SecretPass1!").await;

    let payload: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "resume.bin".to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    // Первый чанк, затем «обрыв» — сервер отдаёт фактический offset.
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload[..1000].to_vec()),
    )
    .await
    .unwrap();
    let status = uploads::session_offset(&env.pool, session.id, user)
        .await
        .unwrap();
    assert_eq!(status.received_bytes, 1000);

    // Продолжение с серверного offset не портит файл.
    let mut offset = status.received_bytes;
    while (offset as usize) < payload.len() {
        let end = ((offset as usize) + 1000).min(payload.len());
        let written = uploads::append_stream(
            &env.state,
            session.id,
            user,
            offset,
            Body::from(payload[offset as usize..end].to_vec()),
        )
        .await
        .unwrap();
        offset = written;
    }

    let entry = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap();
    let resolved = files::resolve_object(&env.pool, user, entry.id)
        .await
        .unwrap();
    let bytes = tokio::fs::read(&resolved.location.absolute).await.unwrap();
    assert_eq!(bytes, payload, "продолжение не повредило данные");
}

#[tokio::test]
async fn tampered_sha_is_detected_by_validation() {
    let env = TestEnv::new().await;
    env.add_volume("tamper-vol").await;
    let user = env.create_user("tamper", "SecretPass1!").await;

    let build = nexus::minecraft::builds::create(
        &env.pool,
        user,
        nexus::minecraft::builds::CreateBuildRequest {
            name: "Tamper".to_owned(),
            slug: Some("tamper".to_owned()),
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

    let session = nexus::minecraft::uploads::create_upload(
        &env.state,
        user,
        build.id,
        nexus::minecraft::uploads::CreateUploadRequest {
            path: "mods/x.jar".to_owned(),
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
    nexus::minecraft::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"data".to_vec()),
    )
    .await
    .unwrap();
    nexus::minecraft::uploads::complete_upload(&env.state, session.id, user)
        .await
        .unwrap();

    // Подмена SHA в БД должна обнаруживаться проверкой сборки.
    sqlx::query("update minecraft_build_files set sha256 = $1 where build_id = $2")
        .bind("0".repeat(64))
        .bind(build.id)
        .execute(&env.pool)
        .await
        .unwrap();

    let report = nexus::minecraft::checks::validate(&env.pool, build.id)
        .await
        .unwrap();
    assert!(!report.valid);
    assert!(
        report
            .errors
            .iter()
            .any(|issue| issue.code == "SHA256_MISMATCH")
    );
}

#[tokio::test]
async fn deleting_entry_does_not_remove_shared_object() {
    let env = TestEnv::new().await;
    env.add_volume("refcount-vol").await;
    let user = env.create_user("refcount", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "kept.bin", b"keep me").await;

    let object_id: Uuid = sqlx::query_scalar("select object_id from cloud_entries where id = $1")
        .bind(entry_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();

    // Искусственно добавляем второго владельца объекта (например, релиз сборки).
    sqlx::query(
        "insert into storage_object_refs (object_id, owner_module, owner_id, display_name)
         values ($1, 'minecraft', gen_random_uuid(), 'artifact')",
    )
    .bind(object_id)
    .execute(&env.pool)
    .await
    .unwrap();
    sqlx::query("update storage_objects set ref_count = 2 where id = $1")
        .bind(object_id)
        .execute(&env.pool)
        .await
        .unwrap();

    service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    service::purge_entry(&env.pool, user, entry_id)
        .await
        .unwrap();

    // Объект остался: на него ссылается другой владелец.
    let exists: Option<Uuid> = sqlx::query_scalar("select id from storage_objects where id = $1")
        .bind(object_id)
        .fetch_optional(&env.pool)
        .await
        .unwrap();
    assert!(exists.is_some(), "файл не теряется при чужой ссылке");
}
