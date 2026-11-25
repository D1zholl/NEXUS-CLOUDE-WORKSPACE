//! Интеграционные тесты Cloud Storage: загрузка чанками, Range, замена, корзина, ссылки.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::{exports, files, service, uploads};
use nexus::system::jobs;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// Представление `cloud_trash` совпадает с записями, помеченными `trashed_at`.
#[tokio::test]
async fn cloud_trash_view_matches_trashed_entries() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-trash-view").await;
    let user = env.create_user("trashview", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "trashable.txt", b"data").await;

    let before: i64 = sqlx::query_scalar("select count(*) from cloud_trash")
        .fetch_one(&env.pool)
        .await
        .expect("представление cloud_trash существует");
    assert_eq!(before, 0);

    service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("select count(*) from cloud_trash where id = $1")
        .bind(entry_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(after, 1, "запись видна в представлении корзины");

    service::restore_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    let restored: i64 = sqlx::query_scalar("select count(*) from cloud_trash where id = $1")
        .bind(entry_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        restored, 0,
        "восстановленная запись исчезает из представления"
    );
}

fn body_from(bytes: Vec<u8>) -> Body {
    Body::from(bytes)
}

async fn upload_file(env: &TestEnv, user: Uuid, name: &str, payload: &[u8]) -> Uuid {
    upload_file_with_type(env, user, name, payload, "text/plain").await
}

async fn upload_file_with_type(
    env: &TestEnv,
    user: Uuid,
    name: &str,
    payload: &[u8],
    content_type: &str,
) -> Uuid {
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
    .expect("создание сессии загрузки");

    let offset =
        uploads::append_stream(&env.state, session.id, user, 0, body_from(payload.to_vec()))
            .await
            .expect("запись чанка");
    assert_eq!(offset, payload.len() as i64);

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
    .expect("завершение загрузки");
    entry.id
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

async fn send_json(
    env: &TestEnv,
    method: Method,
    uri: &str,
    cookie: &str,
    csrf: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    let mut request = builder.body(Body::from(body.to_string())).unwrap();
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
async fn chunked_upload_creates_entry_with_sha() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-vol").await;
    let user = env.create_user("clouduser", "SecretPass1!").await;

    let payload = b"chunked upload payload";
    let entry_id = upload_file(&env, user, "note.txt", payload).await;

    let entry = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(entry.name, "note.txt");
    assert_eq!(entry.size_bytes, payload.len() as i64);
    assert_eq!(entry.entry_type, "file");

    let usage: (i64, i64) =
        sqlx::query_as("select used_bytes, file_count from storage_usage where user_id = $1")
            .bind(user)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(usage.0, payload.len() as i64);
    assert_eq!(usage.1, 1);
}

async fn preview_headers(
    env: &TestEnv,
    user: Uuid,
    entry_id: Uuid,
) -> (StatusCode, axum::http::HeaderMap) {
    let (cookie, _) = api_credentials(env, user).await;
    let request = Request::builder()
        .uri(format!("/api/v1/cloud/entries/{entry_id}/preview"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    (status, response.headers().clone())
}

fn header_text(headers: &axum::http::HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn content_type_comes_from_bytes_not_client_header() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-mime-text").await;
    let user = env.create_user("mimetext", "SecretPass1!").await;

    // Расширение и Content-Type обещают PNG, содержимое — обычный текст.
    let entry_id = upload_file_with_type(
        &env,
        user,
        "fake.png",
        b"this is plain text, not a png image",
        "image/png",
    )
    .await;

    let entry = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(entry.content_type, "text/plain");
    let object_type: String = sqlx::query_scalar(
        "select o.content_type from storage_objects o
         join cloud_entries e on e.object_id = o.id where e.id = $1",
    )
    .bind(entry_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(object_type, "text/plain");

    let (status, headers) = preview_headers(&env, user, entry_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, header::CONTENT_TYPE),
        "text/plain; charset=utf-8"
    );
    assert!(
        header_text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"),
        "безопасный текст открывается inline"
    );
}

#[tokio::test]
async fn html_is_never_served_inline() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-mime-html").await;
    let user = env.create_user("mimehtml", "SecretPass1!").await;

    let payload = b"<!DOCTYPE html><html><body><script>alert('xss')</script></body></html>";
    let entry_id = upload_file_with_type(&env, user, "page.html", payload, "text/html").await;

    let entry = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(entry.content_type, "text/html");

    let (status, headers) = preview_headers(&env, user, entry_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(
        header_text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment"),
        "HTML отдаётся только скачиванием"
    );

    // SVG с активным содержимым: расширение блокирует inline, даже если
    // сигнатуры нет и содержимое похоже на текст.
    let svg_id = upload_file_with_type(
        &env,
        user,
        "vector.svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"></svg>",
        "image/svg+xml",
    )
    .await;
    let (status, headers) = preview_headers(&env, user, svg_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(header_text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment"));

    // JavaScript отдаётся как обычный текст.
    let js_id = upload_file_with_type(
        &env,
        user,
        "app.js",
        b"alert(document.cookie);",
        "application/javascript",
    )
    .await;
    let (status, headers) = preview_headers(&env, user, js_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, header::CONTENT_TYPE),
        "text/plain; charset=utf-8"
    );
    assert!(header_text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"));
}

#[tokio::test]
async fn jpeg_signature_is_recognized_and_previewed() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-mime-jpeg").await;
    let user = env.create_user("mimejpeg", "SecretPass1!").await;

    // Небольшой, но валидный по сигнатуре JPEG; клиент заявляет text/plain.
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
    ];
    let entry_id = upload_file_with_type(&env, user, "photo.txt", &jpeg, "text/plain").await;

    let entry = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(entry.content_type, "image/jpeg");
    let (status, headers) = preview_headers(&env, user, entry_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/jpeg");
    assert!(header_text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"));
}

#[tokio::test]
async fn media_preview_allowlist_keeps_images_video_audio() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-mime-media").await;
    let user = env.create_user("mimemedia", "SecretPass1!").await;

    let png = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R',
    ];
    let mp4 = [
        0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00, 0x00,
        0x00,
    ];
    let mp3 = [b'I', b'D', b'3', 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    for (name, payload, expected) in [
        ("picture.png", png.as_slice(), "image/png"),
        ("clip.mp4", mp4.as_slice(), "video/mp4"),
        ("sound.mp3", mp3.as_slice(), "audio/mpeg"),
    ] {
        let entry_id =
            upload_file_with_type(&env, user, name, payload, "application/octet-stream").await;
        let entry = service::load_entry(&env.pool, user, entry_id)
            .await
            .unwrap();
        assert_eq!(
            entry.content_type, expected,
            "тип {name} определён по сигнатуре"
        );
        let (status, headers) = preview_headers(&env, user, entry_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header_text(&headers, header::CONTENT_TYPE), expected);
        assert!(
            header_text(&headers, header::CONTENT_DISPOSITION).starts_with("inline"),
            "{name} остаётся доступен для предпросмотра"
        );
    }
}

#[tokio::test]
async fn upload_in_chunks_matches_declared_size() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-chunks").await;
    let user = env.create_user("chunkuser", "SecretPass1!").await;

    let payload: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "big.bin".to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    let chunk = 3_000usize;
    let mut offset = 0usize;
    while offset < payload.len() {
        let end = (offset + chunk).min(payload.len());
        let written = uploads::append_stream(
            &env.state,
            session.id,
            user,
            offset as i64,
            body_from(payload[offset..end].to_vec()),
        )
        .await
        .unwrap();
        assert_eq!(written, end as i64);
        offset = end;
    }

    let status = uploads::session_offset(&env.pool, session.id, user)
        .await
        .unwrap();
    assert_eq!(status.received_bytes, payload.len() as i64);

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
    assert_eq!(entry.size_bytes, payload.len() as i64);
}

#[tokio::test]
async fn wrong_offset_is_rejected_without_data_loss() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-offset").await;
    let user = env.create_user("offsetuser", "SecretPass1!").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "offset.bin".to_owned(),
            parent_id: None,
            size: 100,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    uploads::append_stream(&env.state, session.id, user, 0, body_from(vec![7u8; 40]))
        .await
        .unwrap();

    // Повтор с нуля должен быть отвергнут: сервер уже принял 40 байт.
    let error = uploads::append_stream(&env.state, session.id, user, 0, body_from(vec![9u8; 10]))
        .await
        .expect_err("несовпадение смещения");
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "UPLOAD_OFFSET_MISMATCH");
    let details = error.details().cloned().unwrap();
    assert_eq!(details["offset"], 40);

    let status = uploads::session_offset(&env.pool, session.id, user)
        .await
        .unwrap();
    assert_eq!(status.received_bytes, 40, "принятые данные не теряются");
}

#[tokio::test]
async fn interrupted_patch_is_truncated_before_resume() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("cloud-interrupted").await;
    let user = env.create_user("interruptuser", "SecretPass1!").await;

    let payload: Vec<u8> = (0..5_000u32).map(|i| (i % 239) as u8).collect();
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "interrupted.bin".to_owned(),
            parent_id: None,
            size: payload.len() as i64,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    // Имитируем оборванный PATCH: часть тела уже записана в staging,
    // но received_bytes не обновлён (клиент закрыл соединение).
    let root: String = sqlx::query_scalar("select root_path from storage_volumes where id = $1")
        .bind(volume_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let staging =
        nexus::storage::engine::staging_path(&std::path::PathBuf::from(&root), session.id);
    if let Some(parent) = staging.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(&staging, &payload[..100]).await.unwrap();

    // Продолжение с подтверждённого offset должно выровнять staging,
    // а не дописать полный файл поверх «хвоста».
    let written =
        uploads::append_stream(&env.state, session.id, user, 0, body_from(payload.clone()))
            .await
            .unwrap();
    assert_eq!(written, payload.len() as i64);

    let on_disk = tokio::fs::read(&staging).await.unwrap();
    assert_eq!(
        on_disk, payload,
        "staging не должен содержать хвост оборванного PATCH"
    );

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
    assert_eq!(entry.size_bytes, payload.len() as i64);
}

#[tokio::test]
async fn oversized_chunk_is_rejected() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-limit").await;
    let user = env.create_user("limituser", "SecretPass1!").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "limit.bin".to_owned(),
            parent_id: None,
            size: 4 * 1024 * 1024,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    // Лимит чанка в тестовой конфигурации — 1 МиБ.
    let oversized = vec![0u8; 2 * 1024 * 1024];
    let error = uploads::append_stream(&env.state, session.id, user, 0, body_from(oversized))
        .await
        .expect_err("слишком большой чанк");
    assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error.code, "CHUNK_TOO_LARGE");
}

#[tokio::test]
async fn name_conflict_returns_existing_file() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-conflict").await;
    let user = env.create_user("conflictuser", "SecretPass1!").await;

    let first = upload_file(&env, user, "dup.txt", b"first").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "dup.txt".to_owned(),
            parent_id: None,
            size: 6,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        body_from(b"second".to_vec()),
    )
    .await
    .unwrap();

    let error = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect_err("конфликт имени");
    assert_eq!(error.code, "FILE_NAME_CONFLICT");
    assert_eq!(
        error.details().cloned().unwrap()["existingFileId"],
        first.to_string()
    );
}

#[tokio::test]
async fn replace_keeps_single_entry_and_updates_size() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-replace").await;
    let user = env.create_user("replaceuser", "SecretPass1!").await;

    let entry_id = upload_file(&env, user, "swap.txt", b"old payload").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "swap.txt".to_owned(),
            parent_id: None,
            size: 18,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        body_from(b"new payload longer".to_vec()),
    )
    .await
    .unwrap();
    let replaced = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "replace".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(replaced.id, entry_id, "замена обновляет ту же запись");
    assert_eq!(replaced.size_bytes, 18);

    let count: i64 = sqlx::query_scalar("select count(*) from cloud_entries")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "дубликат записи не создаётся");
}

#[tokio::test]
async fn trash_restore_and_purge() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-trash").await;
    let user = env.create_user("trashuser", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "temp.txt", b"temporary").await;

    service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    let trashed = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert!(trashed.trashed_at.is_some());

    let restored = service::restore_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert!(restored.trashed_at.is_none());

    service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    let removed = service::purge_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(removed, 1);

    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 0, "объект с нулевыми ссылками удалён с диска");

    let usage: (i64, i64) =
        sqlx::query_as("select used_bytes, file_count from storage_usage where user_id = $1")
            .bind(user)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(usage.0, 0);
    assert_eq!(usage.1, 0);
}

#[tokio::test]
async fn restore_returns_file_to_original_folder_after_folder_trash() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-trash-origin").await;
    let user = env.create_user("trashorigin", "SecretPass1!").await;

    let docs = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Docs".to_owned(),
            parent_id: None,
        },
    )
    .await
    .unwrap();
    let nested = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Inner".to_owned(),
            parent_id: Some(docs.id),
        },
    )
    .await
    .unwrap();
    let file_id = upload_file(&env, user, "note.txt", b"payload").await;
    service::update_entry(
        &env.pool,
        user,
        file_id,
        service::UpdateEntryRequest {
            name: None,
            parent_id: Some(Some(nested.id)),
            starred: None,
            strategy: None,
        },
    )
    .await
    .unwrap();

    // Удаление папки уводит в корзину всё поддерево вместе с файлом.
    service::trash_entry(&env.pool, user, docs.id)
        .await
        .unwrap();
    for id in [docs.id, nested.id, file_id] {
        let entry = service::load_entry(&env.pool, user, id).await.unwrap();
        assert!(entry.trashed_at.is_some());
    }

    // Одиночное восстановление файла возвращает его в исходную папку,
    // восстанавливая цепочку предков, а не отправляя в корень.
    let restored = service::restore_entry(&env.pool, user, file_id)
        .await
        .unwrap();
    assert!(restored.trashed_at.is_none());
    assert_eq!(restored.parent_id, Some(nested.id));
    for id in [docs.id, nested.id] {
        let entry = service::load_entry(&env.pool, user, id).await.unwrap();
        assert!(entry.trashed_at.is_none(), "папка-предок восстановлена");
    }
}

#[tokio::test]
async fn folder_tree_and_traversal_protection() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-folders").await;
    let user = env.create_user("folderuser", "SecretPass1!").await;

    let root = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Docs".to_owned(),
            parent_id: None,
        },
    )
    .await
    .unwrap();

    let nested = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Inner".to_owned(),
            parent_id: Some(root.id),
        },
    )
    .await
    .unwrap();

    // Папку нельзя переместить внутрь самой себя.
    let error = service::update_entry(
        &env.pool,
        user,
        root.id,
        service::UpdateEntryRequest {
            name: None,
            parent_id: Some(Some(nested.id)),
            starred: None,
            strategy: None,
        },
    )
    .await
    .expect_err("циклическое перемещение");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);

    // Зарезервированные имена и разделители запрещены.
    for bad in ["..", "a/b", "a\\b", "."] {
        let result = service::create_folder(
            &env.pool,
            user,
            service::CreateFolderRequest {
                name: bad.to_owned(),
                parent_id: None,
            },
        )
        .await;
        assert!(result.is_err(), "имя «{bad}» должно быть отклонено");
    }

    // Разноуровневые имена допустимы: «Docs» в корне уникален только на своём уровне.
    let other = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Docs".to_owned(),
            parent_id: Some(root.id),
        },
    )
    .await;
    assert!(other.is_ok(), "в разных папках имя может совпадать");
}

#[tokio::test]
async fn move_to_root_with_explicit_null() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-move").await;
    let user = env.create_user("moveuser", "SecretPass1!").await;

    let folder = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Target".to_owned(),
            parent_id: None,
        },
    )
    .await
    .unwrap();
    let entry_id = upload_file(&env, user, "movable.txt", b"data").await;

    // Перемещение в папку.
    let moved = service::update_entry(
        &env.pool,
        user,
        entry_id,
        service::UpdateEntryRequest {
            name: None,
            parent_id: Some(Some(folder.id)),
            starred: None,
            strategy: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.parent_id, Some(folder.id));

    // Возврат в корень: явный `null` отличается от отсутствия поля.
    let back = service::update_entry(
        &env.pool,
        user,
        entry_id,
        service::UpdateEntryRequest {
            name: None,
            parent_id: Some(None),
            starred: None,
            strategy: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(back.parent_id, None);
}

#[tokio::test]
async fn share_token_is_hashed_and_limited() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-shares").await;
    let user = env.create_user("shareuser", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "shared.txt", b"shared payload").await;

    let (row, token) = nexus::cloud::shares::create_share(
        &env.state,
        user,
        entry_id,
        nexus::cloud::shares::CreateShareRequest {
            expires_in_hours: Some(1),
            password: Some("pass1234".to_owned()),
            max_downloads: Some(1),
            auth_only: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(token.len(), 48);
    let stored: String = sqlx::query_scalar("select token_hash from cloud_shares where id = $1")
        .bind(row.id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(stored, nexus::cloud::shares::hash_share_token(&token));
    assert_ne!(stored, token, "токен не хранится в открытом виде");
    assert!(row.has_password);

    // Публичное разрешение работает по сырому токену.
    let public = nexus::cloud::shares::resolve_public_share(&env.pool, &token)
        .await
        .unwrap();
    assert_eq!(public.name, "shared.txt");

    // Неверный пароль отклоняется, верный — принимается.
    assert!(
        !nexus::cloud::shares::check_share_password(&env.pool, &token, "wrong")
            .await
            .unwrap()
    );
    assert!(
        nexus::cloud::shares::check_share_password(&env.pool, &token, "pass1234")
            .await
            .unwrap()
    );

    // Лимит скачиваний исчерпывается.
    nexus::cloud::shares::register_download(&env.pool, row.id)
        .await
        .unwrap();
    let after = nexus::cloud::shares::resolve_public_share(&env.pool, &token).await;
    assert!(after.is_err(), "после лимита ссылка недоступна");
}

#[tokio::test]
async fn range_parsing_is_correct() {
    // Проверяем контракт диапазонов через публичный хелпер потока.
    let env = TestEnv::new().await;
    env.add_volume("cloud-range").await;
    let user = env.create_user("rangeuser", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "range.txt", b"0123456789").await;

    let resolved = nexus::cloud::files::resolve_object(&env.pool, user, entry_id)
        .await
        .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=2-5"),
    );
    let response =
        nexus::cloud::files::stream_object(&resolved.location, 10, &headers, "text/plain")
            .await
            .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
    let range = response
        .headers()
        .get(axum::http::header::CONTENT_RANGE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(range, "bytes 2-5/10");

    let bytes = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"2345");
}

#[tokio::test]
async fn invalid_range_is_ignored() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-badrange").await;
    let user = env.create_user("badrange", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "bad.txt", b"tiny").await;

    let resolved = nexus::cloud::files::resolve_object(&env.pool, user, entry_id)
        .await
        .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=100-200"),
    );
    let response =
        nexus::cloud::files::stream_object(&resolved.location, 4, &headers, "text/plain")
            .await
            .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

/* ---------------------------- E3: auth-only ссылки ---------------------------- */

/// Ссылка «только для авторизованных»: гость получает 401, владелец сессии —
/// 200 и Range-фрагмент, после отзыва сессии снова 401.
#[tokio::test]
async fn auth_only_share_requires_session_and_supports_range() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-auth-only").await;
    let user = env.create_user("authonly", "SecretPass1!").await;
    let payload = b"0123456789";
    let entry_id = upload_file(&env, user, "private.txt", payload).await;

    let (_, token) = nexus::cloud::shares::create_share(
        &env.state,
        user,
        entry_id,
        nexus::cloud::shares::CreateShareRequest {
            expires_in_hours: Some(1),
            password: None,
            max_downloads: None,
            auth_only: true,
        },
    )
    .await
    .unwrap();

    let app = nexus::build_router(env.state.clone());
    let info_uri = format!("/api/v1/public/shares/{token}");

    // Гость без cookie-сессии.
    let guest = Request::builder()
        .uri(&info_uri)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(guest).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Авторизованный пользователь получает метаданные.
    let (session_token, _) =
        nexus::auth::session::create_session(&env.pool, user, 24, "127.0.0.1", "test")
            .await
            .unwrap();
    let cookie = format!("nexus_session={session_token}");
    let authorized = Request::builder()
        .uri(&info_uri)
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(authorized).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Range работает на публичном скачивании с действующей сессией.
    let range = Request::builder()
        .uri(format!("{info_uri}/download"))
        .header(header::COOKIE, &cookie)
        .header(header::RANGE, "bytes=2-5")
        .extension(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(range).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let bytes = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"2345");

    // После отзыва сессии ссылка снова недоступна.
    sqlx::query("update sessions set revoked_at = now() where token_hash = $1")
        .bind(nexus::auth::session::hash_token(&session_token))
        .execute(&env.pool)
        .await
        .unwrap();
    // Прямое обновление БД в тесте обходит endpoint logout, который в
    // production инвалидирует кэш проверки сессии.
    env.state.invalidate_all_sessions();
    let revoked = Request::builder()
        .uri(&info_uri)
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(revoked).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/* ------------------------- E4: автоочистка корзины ------------------------- */

async fn run_trash_job(env: &TestEnv) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload)
         values ('system.trash.cleanup', 'pending', '{}') returning id",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    jobs::run_job(env.state.clone(), id).await;
    id
}

#[tokio::test]
async fn trash_cleanup_removes_old_keeps_fresh_and_respects_references() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-trash-cleanup").await;
    let user = env.create_user("trashcleanup", "SecretPass1!").await;

    let old_entry = upload_file(&env, user, "old.txt", b"old payload").await;
    let fresh_entry = upload_file(&env, user, "fresh.txt", b"fresh payload").await;
    service::trash_entry(&env.pool, user, old_entry)
        .await
        .unwrap();
    service::trash_entry(&env.pool, user, fresh_entry)
        .await
        .unwrap();
    sqlx::query("update cloud_entries set trashed_at = now() - interval '40 days' where id = $1")
        .bind(old_entry)
        .execute(&env.pool)
        .await
        .unwrap();

    // Внешняя ссылка (например, файл сборки Minecraft) должна сохранить объект.
    let (object_id, object_key, root_path): (Uuid, String, String) = sqlx::query_as(
        "select o.id, o.object_key, v.root_path
         from cloud_entries e
         join storage_objects o on o.id = e.object_id
         join storage_volumes v on v.id = o.volume_id
         where e.id = $1",
    )
    .bind(old_entry)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into storage_object_refs (object_id, owner_module, owner_id, display_name)
         values ($1, 'minecraft', gen_random_uuid(), 'build file')",
    )
    .bind(object_id)
    .execute(&env.pool)
    .await
    .unwrap();
    sqlx::query("update storage_objects set ref_count = ref_count + 1 where id = $1")
        .bind(object_id)
        .execute(&env.pool)
        .await
        .unwrap();

    let job_id = run_trash_job(&env).await;
    let status: String = sqlx::query_scalar("select status from background_jobs where id = $1")
        .bind(job_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(status, "completed");

    let old_count: i64 = sqlx::query_scalar("select count(*) from cloud_entries where id = $1")
        .bind(old_entry)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(old_count, 0, "старая запись удалена");
    let fresh_count: i64 = sqlx::query_scalar("select count(*) from cloud_entries where id = $1")
        .bind(fresh_entry)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(fresh_count, 1, "свежая запись осталась");

    let object_exists: bool =
        sqlx::query_scalar("select exists(select 1 from storage_objects where id = $1)")
            .bind(object_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(object_exists, "объект с внешней ссылкой не удалён");
    let absolute =
        nexus::storage::engine::ObjectLocation::objects_root(&std::path::PathBuf::from(&root_path))
            .join(&object_key);
    assert!(
        absolute.exists(),
        "файл объекта с внешней ссылкой остался на диске"
    );

    let details: serde_json::Value = sqlx::query_scalar(
        "select details from audit_events
         where event_type = 'cloud.trash.cleaned'
         order by created_at desc, id desc limit 1",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(details["entries"], 1);

    // Идемпотентность: повторная задача ничего не удаляет.
    run_trash_job(&env).await;
    let fresh_after: i64 = sqlx::query_scalar("select count(*) from cloud_entries where id = $1")
        .bind(fresh_entry)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(fresh_after, 1);
    let object_after: bool =
        sqlx::query_scalar("select exists(select 1 from storage_objects where id = $1)")
            .bind(object_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(object_after);
}

/* ------------------- E7: TTL экспорта и удаление архива ------------------- */

async fn insert_completed_export(
    env: &TestEnv,
    user: Uuid,
    file: &std::path::Path,
    finished_sql: &str,
) -> Uuid {
    std::fs::write(file, b"PK\x05\x06").unwrap();
    let result = serde_json::json!({ "file": file.to_string_lossy() });
    sqlx::query_scalar(&format!(
        "insert into background_jobs
           (job_type, status, progress, total_units, payload, result, created_by, finished_at)
         values ('cloud.export', 'completed', 100, 1, '{{}}', $1, $2, {finished_sql})
         returning id"
    ))
    .bind(result)
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn export_cleanup_uses_ttl_and_delete_removes_archive() {
    let env = TestEnv::new().await;
    let user = env.create_user("export-user", "SecretPass1!").await;

    let old_file = env.root.path().join("old-export.zip");
    let fresh_file = env.root.path().join("fresh-export.zip");
    let old_id = insert_completed_export(&env, user, &old_file, "now() - interval '5 hours'").await;
    let fresh_id = insert_completed_export(&env, user, &fresh_file, "now()").await;

    // TTL 1 час: удаляется только старый архив, запись остаётся без result.
    let removed = exports::cleanup_exports(&env.pool, 1).await.unwrap();
    assert_eq!(removed, 1);
    assert!(!old_file.exists(), "просроченный ZIP удалён");
    assert!(fresh_file.exists(), "свежий ZIP сохранён");
    let old_result: Option<serde_json::Value> =
        sqlx::query_scalar("select result from background_jobs where id = $1")
            .bind(old_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(old_result.is_none());

    // Владелец удаляет завершённый архив вместе с записью.
    exports::delete_export(&env.pool, user, fresh_id)
        .await
        .unwrap();
    assert!(!fresh_file.exists(), "ZIP удалён вместе с записью");
    let missing = exports::get_job(&env.pool, user, fresh_id)
        .await
        .unwrap_err();
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let download = exports::export_file(&env.pool, user, fresh_id)
        .await
        .unwrap_err();
    assert_eq!(
        download.status,
        StatusCode::NOT_FOUND,
        "повторное скачивание после удаления — 404"
    );
}

#[tokio::test]
async fn export_delete_is_owner_only_and_rejects_running_job() {
    let env = TestEnv::new().await;
    let owner = env.create_user("export-owner", "SecretPass1!").await;
    let stranger = env.create_user("export-stranger", "SecretPass1!").await;
    let file = env.root.path().join("owner-export.zip");
    let job_id = insert_completed_export(&env, owner, &file, "now()").await;

    let denied = exports::delete_export(&env.pool, stranger, job_id)
        .await
        .unwrap_err();
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
    assert!(file.exists(), "чужой пользователь не удаляет архив");

    let running: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, created_by)
         values ('cloud.export', 'running', '{}', $1) returning id",
    )
    .bind(owner)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    let conflict = exports::delete_export(&env.pool, owner, running)
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "EXPORT_NOT_FINISHED");
}

/* --------------------- E9: «Недавние» по дате изменения --------------------- */

fn list_query(view: &str) -> files::ListQuery {
    files::ListQuery {
        page: None,
        per_page: None,
        parent_id: None,
        q: None,
        sort: None,
        direction: None,
        view: Some(view.to_owned()),
        entry_type: None,
        category: None,
        since_days: None,
    }
}

#[tokio::test]
async fn recent_view_keeps_last_30_days_and_sorts_by_date() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-recent").await;
    let user = env.create_user("recentuser", "SecretPass1!").await;

    let old_entry = upload_file(&env, user, "old.txt", b"old").await;
    let fresh_entry = upload_file(&env, user, "fresh.txt", b"fresh").await;
    sqlx::query("update cloud_entries set updated_at = now() - interval '40 days' where id = $1")
        .bind(old_entry)
        .execute(&env.pool)
        .await
        .unwrap();
    sqlx::query("update cloud_entries set updated_at = now() - interval '2 days' where id = $1")
        .bind(fresh_entry)
        .execute(&env.pool)
        .await
        .unwrap();

    let page = files::list_entries(&env.pool, user, &list_query("recent"))
        .await
        .unwrap();

    assert_eq!(page.total, 1, "старый файл не попадает в «Недавние»");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, fresh_entry);

    // Сортировка по дате изменения: свежий файл выше старого.
    let newer = upload_file(&env, user, "newer.txt", b"newer").await;
    sqlx::query("update cloud_entries set updated_at = now() - interval '1 hour' where id = $1")
        .bind(newer)
        .execute(&env.pool)
        .await
        .unwrap();
    let page = files::list_entries(&env.pool, user, &list_query("recent"))
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].id, newer, "сначала самая свежая запись");
}

/* ---------------------- E10: фильтр по типу файлов ---------------------- */

#[tokio::test]
async fn category_filter_keeps_pagination_and_excludes_folders() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-categories").await;
    let user = env.create_user("catuser", "SecretPass1!").await;

    let png = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R',
    ];
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01,
    ];
    let mp3 = [b'I', b'D', b'3', 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let zip = [b'P', b'K', 0x03, 0x04, 0x14, 0x00, 0x00, 0x00];
    upload_file_with_type(&env, user, "cat.png", &png, "image/png").await;
    // Тип определяется по сигнатуре, а не по заголовку клиента.
    upload_file_with_type(&env, user, "photo.jpg", &jpeg, "application/octet-stream").await;
    upload_file_with_type(&env, user, "track.mp3", &mp3, "audio/mpeg").await;
    upload_file_with_type(&env, user, "report.pdf", b"%PDF-1.4\n", "application/pdf").await;
    upload_file_with_type(&env, user, "bundle.zip", &zip, "application/zip").await;
    upload_file_with_type(
        &env,
        user,
        "blob.bin",
        &[0x00, 0x01, 0x02],
        "application/octet-stream",
    )
    .await;
    service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Photos".to_owned(),
            parent_id: None,
        },
    )
    .await
    .unwrap();

    let mut query = list_query("folder");
    query.category = Some("image".to_owned());
    query.sort = Some("name".to_owned());
    query.page = Some(1);
    query.per_page = Some(1);

    let first = files::list_entries(&env.pool, user, &query).await.unwrap();
    assert_eq!(first.total, 2, "фильтр видит только изображения-файлы");
    assert_eq!(first.page, 1);
    assert_eq!(first.per_page, 1);
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].name, "cat.png");

    query.page = Some(2);
    let second = files::list_entries(&env.pool, user, &query).await.unwrap();
    assert_eq!(second.total, 2, "total не зависит от страницы");
    assert_eq!(second.page, 2);
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].name, "photo.jpg");

    query.category = Some("document".to_owned());
    query.page = Some(1);
    let documents = files::list_entries(&env.pool, user, &query).await.unwrap();
    assert_eq!(documents.total, 1);
    assert_eq!(documents.items[0].name, "report.pdf");

    query.category = Some("archive".to_owned());
    let archives = files::list_entries(&env.pool, user, &query).await.unwrap();
    assert_eq!(archives.total, 1);
    assert_eq!(archives.items[0].name, "bundle.zip");

    query.category = Some("other".to_owned());
    let other = files::list_entries(&env.pool, user, &query).await.unwrap();
    assert_eq!(other.total, 1);
    assert_eq!(other.items[0].name, "blob.bin");
}

/* ------------------------ E5: лимит batch-папок ------------------------ */

#[tokio::test]
async fn folder_batch_is_limited_to_200_names() {
    let env = TestEnv::new().await;
    let user = env.create_user("batchuser", "SecretPass1!").await;

    let too_many: Vec<String> = (0..201).map(|index| format!("folder-{index}")).collect();
    let error = service::create_folders_batch(&env.pool, user, None, too_many)
        .await
        .expect_err("201 имя должно быть отклонено");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);

    let allowed: Vec<String> = (0..200).map(|index| format!("folder-{index}")).collect();
    let created = service::create_folders_batch(&env.pool, user, None, allowed)
        .await
        .unwrap();
    assert_eq!(created.len(), 200);
}

/* ------------------- E11: Windows/NTFS совместимость имён ------------------- */

#[tokio::test]
async fn names_are_case_insensitive_in_one_folder() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-case-insensitive").await;
    let user = env.create_user("caseuser", "SecretPass1!").await;

    // Индекс действительно строится по lower(name).
    let definition: Option<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes
         where schemaname = 'public' and tablename = 'cloud_entries'
           and indexname = 'cloud_entries_name_uq'",
    )
    .fetch_optional(&env.pool)
    .await
    .unwrap();
    let definition = definition.expect("индекс уникальности имён существует");
    assert!(definition.contains("lower(name)"), "{definition}");

    service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "Mixed".to_owned(),
            parent_id: None,
        },
    )
    .await
    .unwrap();

    let folder_error = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "mixed".to_owned(),
            parent_id: None,
        },
    )
    .await
    .expect_err("папка с другим регистром отклоняется");
    assert_eq!(folder_error.code, "FILE_NAME_CONFLICT");

    // Прямая вставка в обход предварительной проверки ловится индексом.
    let db_error = sqlx::query(
        "insert into cloud_entries (owner_id, parent_id, name, entry_type)
         values ($1, null, 'MIXED', 'folder')",
    )
    .bind(user)
    .execute(&env.pool)
    .await
    .expect_err("индекс не пропускает дубль без учёта регистра");
    assert_eq!(
        db_error.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    assert_eq!(
        service::name_conflict_from_sqlx(db_error).code,
        "FILE_NAME_CONFLICT",
        "гонка на индексе возвращает понятную ошибку"
    );

    // Файл и папка с тем же именем в разном регистре тоже конфликтуют.
    let _file_entry = upload_file(&env, user, "File.txt", b"file").await;
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "file.txt".to_owned(),
            parent_id: None,
            size: 4,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(b"next".to_vec()),
    )
    .await
    .unwrap();
    let file_error = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect_err("имя файла занято без учёта регистра");
    assert_eq!(file_error.code, "FILE_NAME_CONFLICT");
}

#[tokio::test]
async fn ntfs_reserved_names_and_characters_are_rejected() {
    let env = TestEnv::new().await;

    let bad_names = [
        "CON",
        "con.txt",
        "PRN.log",
        "AUX",
        "NUL",
        "COM1",
        "com9.log",
        "LPT1",
        "lpt9.txt",
        "a<b",
        "a>b",
        "a:b",
        "a\"b",
        "a/b",
        "a\\b",
        "a|b",
        "a?b",
        "a*b",
        "file.",
        "file ",
        "file.txt:payload",
    ];
    for bad in bad_names {
        let error =
            service::validate_name(bad).expect_err(&format!("имя «{bad}» должно быть отклонено"));
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }

    for good in [
        "CONSOLE.txt",
        "COM10.txt",
        "LPT10",
        "file.txt",
        "Отчёт 2026.txt",
        "a-b_c.tar.gz",
    ] {
        let validated = service::validate_name(good).expect(good);
        assert_eq!(validated, good);
    }

    // Сервис возвращает ту же ошибку, а не создаёт запись с именем NTFS.
    let user = env.create_user("ntfsuser", "SecretPass1!").await;
    let error = service::create_folder(
        &env.pool,
        user,
        service::CreateFolderRequest {
            name: "NUL".to_owned(),
            parent_id: None,
        },
    )
    .await
    .expect_err("NTFS-имя отклоняется сервисом");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn full_path_budget_is_enforced_beyond_100_levels() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-path-budget").await;
    let user = env.create_user("pathuser", "SecretPass1!").await;

    // 100 коротких уровней укладываются в бюджет: 100 имён + 99 слэшей.
    let short_levels: Vec<String> = (0..100).map(|_| "a".to_owned()).collect();
    let leaf = service::ensure_folder_path(&env.pool, user, None, &short_levels)
        .await
        .expect("100 коротких уровней допустимы");
    let leaf_entry = service::load_entry(&env.pool, user, leaf).await.unwrap();
    assert_eq!(leaf_entry.entry_type, "folder");

    let depths: i64 = sqlx::query_scalar(
        "with recursive chain as (
             select id, parent_id from cloud_entries where id = $1 and owner_id = $2
             union all
             select e.id, e.parent_id from cloud_entries e join chain c on e.id = c.parent_id
         )
         select count(*) from chain",
    )
    .bind(leaf)
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(depths, 100, "дерево действительно имеет 100 уровней");

    // Те же 100 уровней с длинными именами превышают MAX_PATH-бюджет.
    let long_levels: Vec<String> = (0..100).map(|index| format!("folder-{index:03}")).collect();
    let estimated: usize =
        long_levels.iter().map(|name| name.len()).sum::<usize>() + long_levels.len() - 1;
    assert!(
        estimated > service::PATH_LENGTH_MAX,
        "тестовый путь должен превышать лимит"
    );
    let error = service::ensure_folder_path(&env.pool, user, None, &long_levels)
        .await
        .expect_err("полная длина пути ограничена");
    assert_eq!(error.code, "PATH_TOO_LONG");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    let details = error.details().cloned().unwrap();
    assert_eq!(details["maxLength"], service::PATH_LENGTH_MAX);
    assert!(details["actualLength"].as_u64().unwrap() > service::PATH_LENGTH_MAX as u64);
}

/* ---------------- E8/E11: идемпотентность опасных операций ---------------- */

#[tokio::test]
async fn replace_patch_replays_result_and_rejects_other_body() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-idem-patch").await;
    let user = env.create_user("idempatch", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "patch.txt", b"payload").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let uri = format!("/api/v1/cloud/entries/{entry_id}");
    let key = "patch-replace-key";
    let body = json!({ "name": "renamed.txt", "strategy": "replace" });

    let (status, first) = send_json(
        &env,
        Method::PATCH,
        &uri,
        &cookie,
        &csrf,
        Some(key),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["data"]["name"], "renamed.txt");

    // Повтор с тем же ключом и телом — сохранённый ответ, без второго аудита.
    let (status, replay) =
        send_json(&env, Method::PATCH, &uri, &cookie, &csrf, Some(key), body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], first["data"]["id"]);
    assert_eq!(replay["data"]["name"], "renamed.txt");
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where event_type = 'cloud.entry.updated' and target_id = $1",
    )
    .bind(entry_id.to_string())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "replay не выполняет операцию повторно");

    // Тот же ключ с другим телом — 422.
    let (status, failure) = send_json(
        &env,
        Method::PATCH,
        &uri,
        &cookie,
        &csrf,
        Some(key),
        json!({ "name": "other.txt", "strategy": "replace" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(failure["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
}

#[tokio::test]
async fn share_creation_is_idempotent_and_scoped_per_user() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-idem-share").await;
    let alice = env.create_user("idemshare-alice", "SecretPass1!").await;
    let bob = env.create_user("idemshare-bob", "SecretPass1!").await;
    let alice_entry = upload_file(&env, alice, "alice.txt", b"alice").await;
    let bob_entry = upload_file(&env, bob, "bob.txt", b"bob").await;
    let (alice_cookie, alice_csrf) = api_credentials(&env, alice).await;
    let (bob_cookie, bob_csrf) = api_credentials(&env, bob).await;
    let key = "share-create-key";
    let body = json!({ "entryId": alice_entry, "expiresInHours": 1 });

    let (status, first) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &alice_cookie,
        &alice_csrf,
        Some(key),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let share_id = first["data"]["share"]["id"].clone();

    let (status, replay) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &alice_cookie,
        &alice_csrf,
        Some(key),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["share"]["id"], share_id);
    let count: i64 = sqlx::query_scalar("select count(*) from cloud_shares where owner_id = $1")
        .bind(alice)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "replay не создаёт вторую ссылку");
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where event_type = 'cloud.share.created' and actor_id = $1",
    )
    .bind(alice)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    // Другой body с тем же ключом — 422.
    let (status, failure) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &alice_cookie,
        &alice_csrf,
        Some(key),
        json!({ "entryId": alice_entry, "expiresInHours": 2 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(failure["error"]["code"], "IDEMPOTENCY_KEY_REUSED");

    // Тот же ключ другого пользователя — независимая операция.
    let (status, bob_share) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &bob_cookie,
        &bob_csrf,
        Some(key),
        json!({ "entryId": bob_entry, "expiresInHours": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bob_share}");
    assert_ne!(bob_share["data"]["share"]["id"], share_id);
    let bob_count: i64 =
        sqlx::query_scalar("select count(*) from cloud_shares where owner_id = $1")
            .bind(bob)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(bob_count, 1);
}

#[tokio::test]
async fn export_creation_replays_job_and_rejects_other_body() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-idem-export").await;
    let user = env.create_user("idemexport", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "export.txt", b"export").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let key = "export-create-key";
    let body = json!({ "entryIds": [entry_id] });

    let (status, first) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/exports",
        &cookie,
        &csrf,
        Some(key),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let job_id = first["data"]["id"].clone();

    let (status, replay) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/exports",
        &cookie,
        &csrf,
        Some(key),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], job_id);
    let jobs_count: i64 = sqlx::query_scalar(
        "select count(*) from background_jobs where job_type = 'cloud.export' and created_by = $1",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(jobs_count, 1, "replay не создаёт вторую задачу экспорта");

    let (status, failure) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/exports",
        &cookie,
        &csrf,
        Some(key),
        json!({ "entryIds": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(failure["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
}

/* ------------- E11: Windows sharing violation и отложенное удаление ------------- */

/// Открывает файл без `FILE_SHARE_DELETE` и снимает блокировку через
/// `hold_ms`. Пока блокировка активна, Windows возвращает
/// `ERROR_SHARING_VIOLATION` при удалении.
#[cfg(windows)]
fn lock_windows_file(path: &std::path::Path, hold_ms: u64) -> tokio::task::JoinHandle<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("файл открывается для блокировки");
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(hold_ms)).await;
        drop(locked);
    })
}

#[cfg(windows)]
#[tokio::test]
async fn purge_retries_when_object_is_open_in_another_process() {
    use std::os::windows::fs::OpenOptionsExt;

    let env = TestEnv::new().await;
    env.add_volume("cloud-purge-locked").await;
    let user = env.create_user("purgelock", "SecretPass1!").await;
    let entry_id = upload_file(&env, user, "locked.bin", b"locked payload").await;
    let resolved = files::resolve_object(&env.pool, user, entry_id)
        .await
        .unwrap();
    let object_path = resolved.location.absolute.clone();

    // Убеждаемся, что без FILE_SHARE_DELETE удаление действительно падает.
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&object_path)
        .unwrap();
    let blocked = std::fs::remove_file(&object_path);
    assert!(
        blocked.is_err(),
        "Windows не удаляет открытый файл: {blocked:?}"
    );
    drop(locked);

    let lock = lock_windows_file(&object_path, 120);
    service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    let removed = service::purge_entry(&env.pool, user, entry_id)
        .await
        .expect("purge ждёт освобождения файла");
    assert_eq!(removed, 1);
    lock.await.unwrap();
    assert!(!object_path.exists(), "объект удалён после retry");
    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 0);
}

#[cfg(windows)]
#[tokio::test]
async fn export_delete_retries_when_zip_is_open() {
    let env = TestEnv::new().await;
    let user = env.create_user("exportlock", "SecretPass1!").await;
    let file = env.root.path().join("locked-export.zip");
    let job_id = insert_completed_export(&env, user, &file, "now()").await;
    let lock = lock_windows_file(&file, 120);

    exports::delete_export(&env.pool, user, job_id)
        .await
        .expect("удаление ZIP ждёт закрытия файла");
    lock.await.unwrap();
    assert!(!file.exists(), "ZIP удалён после retry");
    let missing = exports::get_job(&env.pool, user, job_id).await.unwrap_err();
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

/* ------------------------------------------------------------------------- */
/* I4: детали, обновление, пагинация, ссылки и экспорт через HTTP.           */
/* ------------------------------------------------------------------------- */

async fn get_raw(
    env: &TestEnv,
    uri: &str,
    cookie: Option<&str>,
    extra: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
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
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

async fn create_folder_http(env: &TestEnv, cookie: &str, csrf: &str, name: &str) -> Uuid {
    let (status, body) = send_json(
        env,
        Method::POST,
        "/api/v1/cloud/entries",
        cookie,
        csrf,
        None,
        json!({ "name": name, "parentId": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Uuid::parse_str(body["data"]["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn entry_details_reports_preview_breadcrumb_and_active_shares() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-details").await;
    let user = env.create_user("cloud-details", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let folder = create_folder_http(&env, &cookie, &csrf, "Docs").await;
    let file = upload_file(&env, user, "note.txt", b"hello").await;

    let (status, body) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{file}"),
        &cookie,
        &csrf,
        None,
        json!({ "parentId": folder }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &cookie,
        &csrf,
        None,
        json!({ "entryId": file }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, _, bytes) = get_raw(
        &env,
        &format!("/api/v1/cloud/entries/{file}"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let data = &body["data"];
    assert_eq!(data["entry"]["name"], "note.txt");
    assert_eq!(data["preview"], "text/plain; charset=utf-8");
    assert_eq!(data["activeShares"], 1);
    assert_eq!(data["breadcrumb"][0]["id"], folder.to_string());
    assert_eq!(data["breadcrumb"][0]["name"], "Docs");
}

#[tokio::test]
async fn update_entry_renames_stars_moves_and_rejects_cycles() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-update").await;
    let user = env.create_user("cloud-update", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let parent = create_folder_http(&env, &cookie, &csrf, "Parent").await;
    let child = create_folder_http(&env, &cookie, &csrf, "Child").await;
    let file = upload_file(&env, user, "plain.txt", b"before").await;

    // Сначала делаем Child вложенной в Parent, чтобы проверить запрет кольца.
    let (status, _) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{child}"),
        &cookie,
        &csrf,
        None,
        json!({ "parentId": parent }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{file}"),
        &cookie,
        &csrf,
        None,
        json!({ "name": "renamed.txt", "starred": true, "parentId": parent }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["name"], "renamed.txt");
    assert_eq!(body["data"]["starred"], true);
    assert_eq!(body["data"]["parentId"], parent.to_string());

    // Кольцо папок запрещено: Parent нельзя переместить внутрь Child.
    let (status, body) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{parent}"),
        &cookie,
        &csrf,
        None,
        json!({ "parentId": child }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");

    // Переименование в занятое имя даёт 409 с идентификатором конфликта.
    let other = upload_file(&env, user, "taken.txt", b"other").await;
    let (status, body) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{other}"),
        &cookie,
        &csrf,
        None,
        json!({ "name": "renamed.txt", "parentId": parent }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "FILE_NAME_CONFLICT");
    assert_eq!(body["error"]["details"]["existingFileId"], file.to_string());
}

#[tokio::test]
async fn list_entries_filters_by_query_sort_and_pagination() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-list-filters").await;
    let user = env.create_user("cloud-list-filters", "SecretPass1!").await;
    let (cookie, _) = api_credentials(&env, user).await;
    upload_file(&env, user, "Alpha Report.txt", &[b'a'; 10]).await;
    upload_file(&env, user, "beta.log", &[b'b'; 30]).await;
    upload_file(&env, user, "gamma.bin", &[b'g'; 20]).await;

    let (status, body) = send_json(
        &env,
        Method::GET,
        "/api/v1/cloud/entries?q=alpha&entryType=file",
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["items"][0]["name"], "Alpha Report.txt");

    let (status, body) = send_json(
        &env,
        Method::GET,
        "/api/v1/cloud/entries?entryType=file&sort=size&direction=desc&perPage=2&page=1",
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "beta.log");
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["perPage"], 2);

    let (status, body) = send_json(
        &env,
        Method::GET,
        "/api/v1/cloud/entries?entryType=file&sort=size&direction=desc&perPage=2&page=2",
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "Alpha Report.txt");
}

#[tokio::test]
async fn upload_status_reports_offset_and_abort_releases_session() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-upload-status").await;
    let user = env.create_user("cloud-upload-status", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/uploads",
        &cookie,
        &csrf,
        None,
        json!({
            "name": "stream.bin",
            "parentId": null,
            "size": 8,
            "contentType": "application/octet-stream"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session = Uuid::parse_str(body["data"]["id"].as_str().unwrap()).unwrap();

    let mut request = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/v1/cloud/uploads/{session}"))
        .header(header::COOKIE, &cookie)
        .header("x-csrf-token", &csrf)
        .header("upload-offset", "0")
        .body(Body::from("1234"))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let (status, body) = send_json(
        &env,
        Method::GET,
        &format!("/api/v1/cloud/uploads/{session}"),
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["offset"], 4);
    assert_eq!(body["data"]["size"], 8);
    assert_eq!(body["data"]["status"], "active");

    let (status, body) = send_json(
        &env,
        Method::DELETE,
        &format!("/api/v1/cloud/uploads/{session}"),
        &cookie,
        &csrf,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["aborted"], true);

    let stored: String = sqlx::query_scalar("select status from upload_sessions where id = $1")
        .bind(session)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(stored, "aborted");
}

#[tokio::test]
async fn share_list_and_revoke_invalidates_public_link() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-share-revoke").await;
    let user = env.create_user("cloud-share-revoke", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let file = upload_file(&env, user, "shared.txt", b"share me").await;

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &cookie,
        &csrf,
        None,
        json!({ "entryId": file }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let share = Uuid::parse_str(body["data"]["share"]["id"].as_str().unwrap()).unwrap();
    let token = body["data"]["token"].as_str().unwrap().to_owned();
    assert!(body["data"]["url"].as_str().unwrap().ends_with(&token));

    let (status, body) = send_json(
        &env,
        Method::GET,
        &format!("/api/v1/cloud/shares?entryId={file}"),
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["active"], true);

    let (status, _, _) = get_raw(&env, &format!("/api/v1/public/shares/{token}"), None, &[]).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_json(
        &env,
        Method::DELETE,
        &format!("/api/v1/cloud/shares/{share}"),
        &cookie,
        &csrf,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revoked"], true);

    let (status, _, _) = get_raw(&env, &format!("/api/v1/public/shares/{token}"), None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send_json(
        &env,
        Method::GET,
        &format!("/api/v1/cloud/shares?entryId={file}"),
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["active"], false);
    assert!(body["data"][0]["revokedAt"].is_string());
}

#[tokio::test]
async fn password_share_requires_password_and_range_does_not_count_downloads() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-share-password").await;
    let user = env
        .create_user("cloud-share-password", "SecretPass1!")
        .await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let payload = b"protected payload";
    let file = upload_file(&env, user, "protected.bin", payload).await;

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &cookie,
        &csrf,
        None,
        json!({ "entryId": file, "password": "share-pass" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["data"]["token"].as_str().unwrap().to_owned();
    assert_eq!(body["data"]["share"]["hasPassword"], true);

    let (status, body) = send_json(
        &env,
        Method::GET,
        &format!("/api/v1/public/shares/{token}"),
        "",
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["hasPassword"], true);

    let (status, _, _) = get_raw(
        &env,
        &format!("/api/v1/public/shares/{token}/download"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send_json(
        &env,
        Method::POST,
        &format!("/api/v1/public/shares/{token}/unlock"),
        "",
        "",
        None,
        json!({ "password": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "SHARE_PASSWORD_INVALID");

    let (status, body) = send_json(
        &env,
        Method::POST,
        &format!("/api/v1/public/shares/{token}/unlock"),
        "",
        "",
        None,
        json!({ "password": "share-pass" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["data"]["ticket"].as_str().unwrap().to_owned();

    let download_uri = format!("/api/v1/public/shares/{token}/download?ticket={ticket}");
    let (status, _, body) = get_raw(&env, &download_uri, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, payload.as_slice());

    let (status, headers, body) =
        get_raw(&env, &download_uri, None, &[("range", "bytes=0-3")]).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(body, &payload[..4]);

    let count: i32 = sqlx::query_scalar("select download_count from cloud_shares limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "Range по тикету не увеличивает счётчик скачиваний"
    );
}

#[tokio::test]
async fn max_downloads_share_becomes_unavailable_after_limit() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-share-limit").await;
    let user = env.create_user("cloud-share-limit", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let file = upload_file(&env, user, "once.txt", b"only once").await;

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &cookie,
        &csrf,
        None,
        json!({ "entryId": file, "maxDownloads": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["data"]["token"].as_str().unwrap().to_owned();

    let uri = format!("/api/v1/public/shares/{token}/download");
    let (status, _, _) = get_raw(&env, &uri, None, &[]).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = get_raw(&env, &uri, None, &[]).await;
    assert_eq!(status, StatusCode::GONE, "исчерпанный лимит возвращает 410");
    let count: i32 = sqlx::query_scalar("select download_count from cloud_shares limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn export_status_list_and_file_require_completed_owner_job() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-export-http").await;
    let owner = env.create_user("cloud-export-http", "SecretPass1!").await;
    let other = env.create_user("cloud-export-other", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, owner).await;
    let (other_cookie, _) = api_credentials(&env, other).await;
    let first = upload_file(&env, owner, "one.txt", b"one").await;
    let second = upload_file(&env, owner, "two.txt", b"two").await;

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/exports",
        &cookie,
        &csrf,
        None,
        json!({ "entryIds": [first, second] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let job = Uuid::parse_str(body["data"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(body["data"]["status"], "pending");

    let (status, body) = send_json(
        &env,
        Method::GET,
        "/api/v1/cloud/exports",
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], job.to_string());

    let (status, body) = send_json(
        &env,
        Method::GET,
        &format!("/api/v1/cloud/exports/{job}"),
        &cookie,
        "",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totalUnits"], 2);

    let (status, _, _) = get_raw(
        &env,
        &format!("/api/v1/cloud/exports/{job}/file"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = send_json(
        &env,
        Method::DELETE,
        &format!("/api/v1/cloud/exports/{job}"),
        &cookie,
        &csrf,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "EXPORT_NOT_FINISHED");

    let (status, _, _) = get_raw(
        &env,
        &format!("/api/v1/cloud/exports/{job}"),
        Some(&other_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "чужой экспорт скрыт");
}

#[tokio::test]
async fn purge_requires_trash_and_empty_trash_removes_entries() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-trash-empty").await;
    let user = env.create_user("cloud-trash-empty", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let first = upload_file(&env, user, "a.txt", b"a").await;
    let second = upload_file(&env, user, "b.txt", b"b").await;

    let (status, body) = send_json(
        &env,
        Method::DELETE,
        &format!("/api/v1/cloud/entries/{first}/purge"),
        &cookie,
        &csrf,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "NOT_IN_TRASH");

    for id in [first, second] {
        let (status, _) = send_json(
            &env,
            Method::DELETE,
            &format!("/api/v1/cloud/entries/{id}"),
            &cookie,
            &csrf,
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_json(
        &env,
        Method::DELETE,
        "/api/v1/cloud/trash",
        &cookie,
        &csrf,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deleted"], true);
    assert_eq!(body["data"]["files"], 2);

    let left: i64 = sqlx::query_scalar("select count(*) from cloud_entries")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn foreign_entries_and_parent_ids_leak_nothing() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-foreign").await;
    let alice = env.create_user("cloud-alice", "SecretPass1!").await;
    let bob = env.create_user("cloud-bob", "SecretPass1!").await;
    let (alice_cookie, alice_csrf) = api_credentials(&env, alice).await;
    let (bob_cookie, bob_csrf) = api_credentials(&env, bob).await;
    let folder = create_folder_http(&env, &alice_cookie, &alice_csrf, "Alice").await;
    let file = upload_file(&env, alice, "alice.txt", b"secret").await;

    for uri in [
        format!("/api/v1/cloud/entries/{file}"),
        format!("/api/v1/cloud/entries/{file}/preview"),
        format!("/api/v1/cloud/entries/{file}/download"),
    ] {
        let (status, _, _) = get_raw(&env, &uri, Some(&bob_cookie), &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }

    // Чужой parentId не раскрывает содержимое: пустая страница, не 404 и не чужие имена.
    let (status, _, bytes) = get_raw(
        &env,
        &format!("/api/v1/cloud/entries?parentId={folder}"),
        Some(&bob_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listing: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listing["data"]["items"], json!([]));
    assert_eq!(listing["data"]["total"], 0);

    let (status, body) = send_json(
        &env,
        Method::PATCH,
        &format!("/api/v1/cloud/entries/{file}"),
        &bob_cookie,
        &bob_csrf,
        None,
        json!({ "name": "stolen.txt" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");

    let (status, body) = send_json(
        &env,
        Method::POST,
        "/api/v1/cloud/shares",
        &bob_cookie,
        &bob_csrf,
        None,
        json!({ "entryId": file }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");

    let name: String = sqlx::query_scalar("select name from cloud_entries where id = $1")
        .bind(file)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(name, "alice.txt", "чужое изменение не применилось");
}

#[tokio::test]
async fn parallel_patch_is_serialized_and_keeps_staging_consistent() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-parallel-upload").await;
    let user = env.create_user("cloud-parallel", "SecretPass1!").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "parallel.bin".to_owned(),
            parent_id: None,
            size: 6,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("������ ��������");

    let payload = b"abcdef";
    let first = uploads::append_chunk(&env.state, session.id, user, 0, payload);
    let second = uploads::append_chunk(&env.state, session.id, user, 0, payload);
    let (first, second) = tokio::join!(first, second);

    let successes = usize::from(first.is_ok()) + usize::from(second.is_ok());
    let failures = usize::from(first.is_err()) + usize::from(second.is_err());
    assert_eq!(successes, 1, "����� ���� PATCH ���������� ����");
    assert_eq!(failures, 1, "������ PATCH �����������");
    let error = first.err().or_else(|| second.err()).expect("���� ������");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(
        matches!(
            error.code.as_str(),
            "UPLOAD_BUSY" | "UPLOAD_OFFSET_MISMATCH"
        ),
        "����������� ���: {}",
        error.code
    );

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
    .expect("�������� ����������� ����� ������������");
    assert_eq!(entry.size_bytes, payload.len() as i64);
    assert!(
        env.state.upload_locks.is_empty(),
        "lock ��������� ����� complete"
    );
}

#[tokio::test]
async fn expired_upload_releases_lock_and_marks_session() {
    let env = TestEnv::new().await;
    env.add_volume("cloud-expire-lock").await;
    let user = env.create_user("cloud-expire", "SecretPass1!").await;

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "expire.bin".to_owned(),
            parent_id: None,
            size: 1,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("������ ��������");
    let lock = env.state.upload_lock(session.id);
    drop(lock);

    sqlx::query(
        "update upload_sessions set expires_at = now() - interval '1 minute' where id = $1",
    )
    .bind(session.id)
    .execute(&env.pool)
    .await
    .expect("������ ����������");
    uploads::cleanup_expired(&env.state)
        .await
        .expect("������� �����������");

    let status: String = sqlx::query_scalar("select status from upload_sessions where id = $1")
        .bind(session.id)
        .fetch_one(&env.pool)
        .await
        .expect("������ ����������");
    assert_eq!(status, "expired");
    assert!(
        env.state.upload_locks.is_empty(),
        "lock ��������� ��� ��������� TTL"
    );
}
