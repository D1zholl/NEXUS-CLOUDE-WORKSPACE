//! H4: Windows/NTFS-инварианты файловой безопасности.
//!
//! Все проверки выполняются на реальном HTTP-слое (`build_router`) и на
//! настоящей файловой системе Windows/NTFS. Тесты не подменяют файловые
//! операции моками: junction и symlink создаются штатными средствами ОС.

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::{service, uploads};
#[cfg(windows)]
use nexus::storage::engine::{self, ObjectLocation};
#[cfg(windows)]
use nexus::storage::{paths, service as storage_service};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).expect("ответ — JSON")
        }
    }

    fn error_code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn header(&self, name: header::HeaderName) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

async fn credentials(env: &TestEnv, user: Uuid) -> (String, String) {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "h4-windows",
    )
    .await
    .expect("сессия создаётся");
    let cookie = format!("{SESSION_COOKIE}={token}");
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    (cookie, csrf)
}

async fn send(
    env: &TestEnv,
    method: Method,
    uri: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if !cookie.is_empty() {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("тело ответа читается")
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

async fn get(env: &TestEnv, uri: &str, cookie: &str) -> Reply {
    send(env, Method::GET, uri, cookie, None, None).await
}

async fn post(env: &TestEnv, uri: &str, cookie: &str, csrf: &str, body: Value) -> Reply {
    send(env, Method::POST, uri, cookie, Some(csrf), Some(body)).await
}

/// Готовит завершённый файл в корне Cloud (для проверок скачивания/предпросмотра).
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
    .expect("сессия загрузки");
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк записан");
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
    .expect("загрузка завершена")
    .id
}

async fn begin_upload(env: &TestEnv, user: Uuid, name: &str, payload: &[u8]) -> Uuid {
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
    .expect("сессия загрузки");
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк записан");
    session.id
}

async fn object_path(env: &TestEnv, entry_id: Uuid) -> PathBuf {
    let (key, root): (String, String) = sqlx::query_as(
        "select o.object_key, v.root_path
         from cloud_entries e
         join storage_objects o on o.id = e.object_id
         join storage_volumes v on v.id = o.volume_id
         where e.id = $1",
    )
    .bind(entry_id)
    .fetch_one(&env.pool)
    .await
    .expect("объект записи существует");
    ObjectLocation::objects_root(Path::new(&root)).join(key)
}

fn decode_rfc5987(header_value: &str) -> String {
    let (_, encoded) = header_value
        .split_once("UTF-8''")
        .expect("filename* должен быть в кодировке UTF-8");
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("hex");
            out.push(u8::from_str_radix(hex, 16).expect("корректный percent-encoding"));
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).expect("UTF-8 имя")
}

fn assert_rfc5987_attr_chars(encoded: &str) {
    for ch in encoded.chars() {
        let allowed = ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '!' | '#' | '$' | '&' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~' | '%'
            );
        assert!(allowed, "недопустимый символ RFC 5987: {ch:?} в {encoded}");
    }
}

/* ---------------- Traversal, диск, UNC, ADS ---------------- */

#[tokio::test]
async fn traversal_drive_unc_and_ads_are_rejected_with_422() {
    let env = TestEnv::new().await;
    env.add_volume("win-traversal").await;
    let user = env.create_user("win-traversal", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;

    let bad_names = [
        r"..\evil.txt",
        r"..\..\Windows\System32\evil.dll",
        "..",
        "../escape.txt",
        "sub/../../escape.txt",
        r"\absolute\path.txt",
        "/absolute/path.txt",
        r"C:file.txt",
        r"C:\Windows\evil.dll",
        r"C:/Windows/evil.dll",
        r"\\host\share\evil.txt",
        r"\\?\C:\Windows\evil.dll",
        "file.txt:payload",
        r"file.txt:payload:$DATA",
    ];

    for name in bad_names {
        let reply = post(
            &env,
            "/api/v1/cloud/entries",
            &cookie,
            &csrf,
            json!({ "name": name }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "имя {name:?} должно отклоняться: {}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(reply.error_code(), "VALIDATION_ERROR", "имя {name:?}");

        let upload = post(
            &env,
            "/api/v1/cloud/uploads",
            &cookie,
            &csrf,
            json!({ "name": name, "size": 1, "contentType": "text/plain" }),
        )
        .await;
        assert_eq!(
            upload.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "загрузка с именем {name:?} отклоняется"
        );
    }

    let count: i64 = sqlx::query_scalar("select count(*) from cloud_entries where owner_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "ни одна опасная запись не создана");
}

/* ---------------- SQL-метасимволы остаются данными ---------------- */

#[tokio::test]
async fn sql_metacharacters_in_names_are_stored_as_data() {
    let env = TestEnv::new().await;
    let user = env.create_user("win-sql", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;

    let name = "x'); drop table cloud_entries;--";
    let reply = post(
        &env,
        "/api/v1/cloud/entries",
        &cookie,
        &csrf,
        json!({ "name": name }),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(reply.json()["data"]["name"], name);

    // Таблица цела, имя сохранено дословно: bind-параметр не стал SQL.
    let table: Option<String> =
        sqlx::query_scalar("select to_regclass('public.cloud_entries')::text")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(table.is_some());
    let stored: String = sqlx::query_scalar("select name from cloud_entries where owner_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(stored, name);
}

/* ---------------- Зарезервированные имена, точка/пробел ---------------- */

#[tokio::test]
async fn reserved_device_names_and_trailing_chars_are_rejected_with_422() {
    let env = TestEnv::new().await;
    let user = env.create_user("win-reserved", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;

    // Политика: не «нормализуем» молча, а отклоняем. Иначе пользователь
    // получил бы другое имя, чем ввёл, а Windows создал бы устройство.
    let bad_names = [
        "CON",
        "con.txt",
        "PRN.log",
        "AUX",
        "NUL",
        "NUL.txt",
        "COM1",
        "com9.log",
        "LPT1",
        "lpt9.txt",
        "CONIN$",
        "CONOUT$",
        "CLOCK$",
        "file.",
        "file ",
        "file.txt.",
        "file.txt ",
        "...",
    ];

    for name in bad_names {
        let reply = post(
            &env,
            "/api/v1/cloud/entries",
            &cookie,
            &csrf,
            json!({ "name": name }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "имя {name:?} должно отклоняться: {}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Похожие, но допустимые имена не должны попадать под запрет.
    for name in ["CONSOLE.txt", "COM10.txt", "LPT10", "auxiliary.md"] {
        let reply = post(
            &env,
            "/api/v1/cloud/entries",
            &cookie,
            &csrf,
            json!({ "name": name }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "допустимое имя {name:?}: {}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    let count: i64 = sqlx::query_scalar("select count(*) from cloud_entries where owner_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 4, "созданы только допустимые имена");
}

/* ---------------- Junction и symlink за корень ---------------- */

#[cfg(windows)]
fn make_junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn make_symlink(link: &Path, target: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(windows)]
#[tokio::test]
async fn reparse_points_cannot_escape_volume() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("win-reparse").await;
    let volume_root = env.root.path().join("win-reparse");
    let outside = env.root.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"outside-the-volume").unwrap();

    let objects = ObjectLocation::objects_root(&volume_root);
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::create_dir_all(volume_root.join("normal-folder")).unwrap();

    let junction = volume_root.join("junction-escape");
    assert!(
        make_junction(&junction, &outside),
        "junction создаётся штатным mklink /J"
    );
    let symlink = volume_root.join("symlink-escape");
    let symlink_created = make_symlink(&symlink, &outside);

    // Ссылки внутри объектного хранилища: именно их проверяет production-путь
    // open_object.
    let object_junction = objects.join("junction-escape");
    assert!(make_junction(&object_junction, &outside));
    let object_symlink = objects.join("symlink-escape");
    let object_symlink_created = make_symlink(&object_symlink, &outside);

    let mut links = vec![junction.clone()];
    if symlink_created {
        links.push(symlink.clone());
    } else {
        eprintln!("symlink /D недоступен без привилегии — проверен только junction");
    }

    for link in &links {
        let name = link.file_name().unwrap();
        assert!(
            paths::resolve_within(&volume_root, Path::new(name)).is_err(),
            "resolve_within отвергает {name:?}"
        );
        let error = paths::ensure_no_links(&volume_root, link)
            .expect_err("ensure_no_links отвергает reparse point");
        assert!(error.0.contains("junctions"), "{}", error.0);
    }

    let mut object_links = vec![object_junction];
    if object_symlink_created {
        object_links.push(object_symlink);
    }
    for link in &object_links {
        // Production-путь Storage Engine: объект «внутри» ссылки не читается.
        let location = ObjectLocation {
            volume_id,
            key: format!("{}/secret.txt", link.file_name().unwrap().to_string_lossy()),
            absolute: link.join("secret.txt"),
            root: volume_root.clone(),
        };
        let error = engine::open_object(&location)
            .await
            .expect_err("open_object не идёт по reparse point");
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "VALIDATION_ERROR");
    }

    // Рекурсивный обход (browse_directory) не заходит по ссылкам.
    let listing = storage_service::browse_directory(volume_root.to_str().unwrap())
        .expect("каталог листается");
    let names: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert!(names.contains(&"normal-folder"), "{names:?}");
    assert!(!names.contains(&"junction-escape"), "{names:?}");
    assert!(!names.contains(&"symlink-escape"), "{names:?}");

    // Регистрация тома через ссылку отклоняется на HTTP-слое (422).
    let user = env.create_user("win-reparse", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;
    for link in &links {
        let reply = post(
            &env,
            "/api/v1/storage/volumes",
            &cookie,
            &csrf,
            json!({ "name": "linked", "path": link.to_string_lossy() }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "том через {link:?}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(reply.error_code(), "VALIDATION_ERROR");
    }

    let volumes: i64 =
        sqlx::query_scalar("select count(*) from storage_volumes where name = 'linked'")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(volumes, 0, "том через ссылку не зарегистрирован");
}

/* ---------------- Регистронезависимая коллизия ---------------- */

#[tokio::test]
async fn case_insensitive_conflict_does_not_overwrite_physical_file() {
    let env = TestEnv::new().await;
    env.add_volume("win-conflict").await;
    let user = env.create_user("win-conflict", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;

    let original = b"original-bytes-must-survive";
    let entry_id = upload_file(&env, user, "Report.TXT", original).await;
    let original_object = object_path(&env, entry_id).await;
    assert_eq!(tokio::fs::read(&original_object).await.unwrap(), original);

    // Второй файл отличается только регистром имени.
    let session_id = begin_upload(&env, user, "report.txt", b"attacker-bytes").await;
    let reply = post(
        &env,
        &format!("/api/v1/cloud/uploads/{session_id}/complete"),
        &cookie,
        &csrf,
        json!({ "strategy": "auto" }),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(reply.error_code(), "FILE_NAME_CONFLICT");
    let details = &reply.json()["error"]["details"];
    assert_eq!(details["existingFileId"], json!(entry_id));

    // Запись осталась одна, её размер и объект не изменились.
    let entries: i64 = sqlx::query_scalar("select count(*) from cloud_entries where owner_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(entries, 1, "вторая запись не создана");
    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 1, "новый физический объект не создан");
    assert_eq!(
        tokio::fs::read(&original_object).await.unwrap(),
        original,
        "байты исходного объекта не перезаписаны"
    );
    let entry = service::load_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    assert_eq!(entry.size_bytes, original.len() as i64);
    assert_eq!(entry.name, "Report.TXT");

    let download = get(
        &env,
        &format!("/api/v1/cloud/entries/{entry_id}/download"),
        &cookie,
    )
    .await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.body, original);
}

/* ---------------- Длинный путь (>260) ---------------- */

#[tokio::test]
async fn long_virtual_path_returns_clear_422_without_truncation() {
    let env = TestEnv::new().await;
    let user = env.create_user("win-longpath", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, user).await;

    // 21 уровень по 10 UTF-16 единиц: 21 * 10 + 20 = 230 единиц, в пределах
    // бюджета 240. Это максимум, который сервер разрешает построить.
    let mut parent: Option<Uuid> = None;
    for index in 0..21 {
        let name = format!("level{index:02}abc"); // 10 UTF-16 единиц
        let reply = post(
            &env,
            "/api/v1/cloud/entries",
            &cookie,
            &csrf,
            json!({ "name": name, "parentId": parent }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "уровень {index}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        parent = reply.json()["data"]["id"]
            .as_str()
            .map(|id| id.parse().unwrap());
    }

    // Лист из 128 символов дал бы 230 + 1 + 128 = 359 единиц — заметно
    // больше MAX_PATH (260). Сервер обязан вернуть 422, а не обрезать имя.
    let long_leaf = "я".repeat(128);
    let expected_actual = 230 + 1 + long_leaf.encode_utf16().count();
    let failure = post(
        &env,
        "/api/v1/cloud/entries",
        &cookie,
        &csrf,
        json!({ "name": long_leaf, "parentId": parent }),
    )
    .await;
    assert_eq!(
        failure.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&failure.body)
    );
    assert_eq!(failure.error_code(), "PATH_TOO_LONG");
    let details = &failure.json()["error"]["details"];
    assert_eq!(
        details["maxLength"].as_u64().unwrap(),
        service::PATH_LENGTH_MAX as u64
    );
    assert_eq!(
        details["actualLength"].as_u64().unwrap(),
        expected_actual as u64
    );
    assert!(
        expected_actual > 260,
        "запрошенный путь должен превышать MAX_PATH: {expected_actual}"
    );

    // Одиночное имя длиннее 128 символов отклоняется с понятным кодом.
    let reply = post(
        &env,
        "/api/v1/cloud/entries",
        &cookie,
        &csrf,
        json!({ "name": "я".repeat(129) }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.error_code(), "VALIDATION_ERROR");

    // Сервер не обрезал уже созданные имена: ровно 21 уровень.
    let created: i64 = sqlx::query_scalar(
        "with recursive chain as (
             select id, parent_id, 1::bigint as depth from cloud_entries
             where owner_id = $1 and parent_id is null
             union all
             select e.id, e.parent_id, c.depth + 1
             from cloud_entries e join chain c on e.parent_id = c.id
         )
         select coalesce(max(depth), 0) from chain",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(created, 21);
}

/* ---------------- Content-Disposition: не-ASCII ---------------- */

#[tokio::test]
async fn non_ascii_download_name_uses_rfc5987_filename_star() {
    let env = TestEnv::new().await;
    env.add_volume("win-disposition").await;
    let user = env.create_user("win-disposition", "SecretPass1!").await;
    let (cookie, _csrf) = credentials(&env, user).await;

    let name = "Отчёт 2026 (итог).txt";
    let entry_id = upload_file(&env, user, name, b"report body").await;

    let download = get(
        &env,
        &format!("/api/v1/cloud/entries/{entry_id}/download"),
        &cookie,
    )
    .await;
    assert_eq!(download.status, StatusCode::OK);
    let disposition = download.header(header::CONTENT_DISPOSITION);
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    let fallback = disposition
        .split_once("filename=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value)
        .expect("ASCII-fallback filename");
    assert!(fallback.is_ascii(), "{fallback}");
    assert!(!fallback.contains("Отчёт"));

    let encoded = disposition
        .split_once("filename*=UTF-8''")
        .map(|(_, value)| value)
        .expect("filename* присутствует");
    assert_rfc5987_attr_chars(encoded);
    assert!(!encoded.contains(' '), "пробел должен быть percent-encoded");
    assert!(!encoded.contains('('), "скобка должна быть percent-encoded");
    assert_eq!(decode_rfc5987(&disposition), name);

    let preview = get(
        &env,
        &format!("/api/v1/cloud/entries/{entry_id}/preview"),
        &cookie,
    )
    .await;
    assert_eq!(preview.status, StatusCode::OK);
    let preview_disposition = preview.header(header::CONTENT_DISPOSITION);
    assert!(
        preview_disposition.starts_with("inline;"),
        "{preview_disposition}"
    );
    assert_eq!(decode_rfc5987(&preview_disposition), name);
    assert!(preview_disposition.contains("filename=\""));
}

/* ---------------- Предпросмотр, поддельный тип, чужой файл, CSP ---------------- */

#[tokio::test]
async fn preview_is_inert_foreign_files_are_404_and_csp_is_present() {
    let env = TestEnv::new().await;
    env.add_volume("win-preview").await;
    let alice = env.create_user("win-alice", "SecretPass1!").await;
    let bob = env.create_user("win-bob", "SecretPass1!").await;
    let (alice_cookie, _alice_csrf) = credentials(&env, alice).await;
    let (bob_cookie, _bob_csrf) = credentials(&env, bob).await;

    // 1. HTML с заявленным image/png: тип определяется по содержимому.
    let html = b"<html><script>alert(document.cookie)</script></html>";
    let session = uploads::create_upload(
        &env.state,
        alice,
        uploads::CreateUploadRequest {
            name: "innocent.png".to_owned(),
            parent_id: None,
            size: html.len() as i64,
            content_type: "image/png".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(&env.state, session.id, alice, 0, Body::from(html.to_vec()))
        .await
        .unwrap();
    let html_entry = uploads::complete_upload(
        &env.state,
        session.id,
        alice,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap()
    .id;

    let entry = service::load_entry(&env.pool, alice, html_entry)
        .await
        .unwrap();
    assert_eq!(entry.content_type, "text/html");
    let preview = get(
        &env,
        &format!("/api/v1/cloud/entries/{html_entry}/preview"),
        &alice_cookie,
    )
    .await;
    assert_eq!(preview.status, StatusCode::OK);
    assert_eq!(
        preview.header(header::CONTENT_TYPE),
        "application/octet-stream"
    );
    assert!(
        preview
            .header(header::CONTENT_DISPOSITION)
            .starts_with("attachment")
    );
    assert_eq!(preview.header(header::X_CONTENT_TYPE_OPTIONS), "nosniff");

    // 2. JavaScript никогда не отдаётся активным типом.
    let js_entry = upload_file(&env, alice, "payload.js", b"alert(1)").await;
    let js_preview = get(
        &env,
        &format!("/api/v1/cloud/entries/{js_entry}/preview"),
        &alice_cookie,
    )
    .await;
    assert_eq!(js_preview.status, StatusCode::OK);
    assert_eq!(
        js_preview.header(header::CONTENT_TYPE),
        "text/plain; charset=utf-8"
    );
    assert!(
        js_preview
            .header(header::CONTENT_DISPOSITION)
            .starts_with("inline")
    );

    // 3. CSP и анти-framing на файловых ответах.
    let csp_header = header::HeaderName::from_static("content-security-policy");
    let permissions_header = header::HeaderName::from_static("permissions-policy");
    for reply in [&preview, &js_preview] {
        let csp = reply.header(csp_header.clone());
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(csp.contains("script-src 'self'"), "{csp}");
        assert!(csp.contains("object-src 'none'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert_eq!(reply.header(header::X_FRAME_OPTIONS), "DENY");
        assert_eq!(
            reply.header(header::REFERRER_POLICY),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(
            reply.header(permissions_header.clone()),
            "camera=(), microphone=(), geolocation=()"
        );
    }

    // 4. Чужой файл — 404 на всех маршрутах, без утечки содержимого.
    for suffix in ["", "/download", "/preview"] {
        let reply = get(
            &env,
            &format!("/api/v1/cloud/entries/{html_entry}{suffix}"),
            &bob_cookie,
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "чужой файл {suffix}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            !reply
                .body
                .windows(html.len())
                .any(|window| window == html.as_slice())
        );
    }

    // 5. Подделка Content-Type при загрузке не меняет тип объекта.
    let fake = uploads::create_upload(
        &env.state,
        alice,
        uploads::CreateUploadRequest {
            name: "fake.jpg".to_owned(),
            parent_id: None,
            size: 6,
            content_type: "image/jpeg".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        fake.id,
        alice,
        0,
        Body::from(b"hello!".to_vec()),
    )
    .await
    .unwrap();
    let fake_entry = uploads::complete_upload(
        &env.state,
        fake.id,
        alice,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(fake_entry.content_type, "text/plain");
}
