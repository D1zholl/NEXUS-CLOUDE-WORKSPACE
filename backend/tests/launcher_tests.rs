//! Интеграционные тесты Launcher API: токены, scopes, каталог, манифест, артефакты.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session};
use nexus::launcher::{catalog, tokens};
use nexus::minecraft::{builds, releases, uploads};
use tower::ServiceExt;
use uuid::Uuid;

async fn owner(env: &TestEnv) -> Uuid {
    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from users where username = 'launchtester'")
            .fetch_optional(&env.pool)
            .await
            .unwrap();
    match existing {
        Some(id) => id,
        None => env.create_user("launchtester", "SecretPass1!").await,
    }
}

async fn publish_build(
    env: &TestEnv,
    user: Uuid,
    name: &str,
    slug: &str,
    visibility: &str,
) -> (Uuid, Uuid) {
    env.add_volume(&format!("{slug}-vol")).await;
    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: name.to_owned(),
            slug: Some(slug.to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
            visibility: visibility.to_owned(),
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

    let payload = b"launcher artifact";
    let session = uploads::create_upload(
        &env.state,
        user,
        build.id,
        uploads::CreateUploadRequest {
            path: "mods/a.jar".to_owned(),
            size: payload.len() as i64,
            category: Some("mods".to_owned()),
            side: Some("client".to_owned()),
            required: Some(true),
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
        Body::from(payload.to_vec()),
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
            changelog: "release".to_owned(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, user, "tester", build.id, release.id, true)
        .await
        .unwrap();
    (build.id, release.id)
}

async fn publish_public_build(env: &TestEnv) -> (Uuid, Uuid) {
    let user = owner(env).await;
    publish_build(env, user, "Public Pack", "public-pack", "public").await
}

#[tokio::test]
async fn token_is_hashed_and_scoped() {
    let env = TestEnv::new().await;
    owner(&env).await;

    let issued = tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "CLI".to_owned(),
            username: "launchtester".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: Some(vec![tokens::SCOPE_CATALOG.to_owned()]),
            expires_in_hours: Some(24),
        },
    )
    .await
    .expect("выдача токена");

    assert!(issued.token.starts_with("nxl_"));
    assert_eq!(issued.token.len(), 4 + 64);
    assert_eq!(issued.scopes, vec![tokens::SCOPE_CATALOG.to_owned()]);

    let stored: String = sqlx::query_scalar("select token_hash from access_tokens")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(stored, tokens::hash_token(&issued.token));
    assert_ne!(stored, issued.token, "токен не хранится открытым");
}

#[tokio::test]
async fn wrong_password_is_rejected() {
    let env = TestEnv::new().await;
    owner(&env).await;

    let error = tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "CLI".to_owned(),
            username: "launchtester".to_owned(),
            password: "wrong-password".to_owned(),
            scopes: None,
            expires_in_hours: None,
        },
    )
    .await
    .expect_err("неверный пароль");
    assert_eq!(error.code, "INVALID_CREDENTIALS");
}

#[tokio::test]
async fn unknown_scope_is_rejected() {
    let env = TestEnv::new().await;
    owner(&env).await;

    let error = tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "CLI".to_owned(),
            username: "launchtester".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: Some(vec!["admin".to_owned()]),
            expires_in_hours: None,
        },
    )
    .await
    .expect_err("неизвестный scope");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn revoked_token_stops_working() {
    let env = TestEnv::new().await;
    owner(&env).await;
    let issued = tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "CLI".to_owned(),
            username: "launchtester".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: None,
            expires_in_hours: Some(24),
        },
    )
    .await
    .unwrap();

    let token_id: Uuid = sqlx::query_scalar("select id from access_tokens")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    tokens::revoke_self(&env.pool, token_id).await.unwrap();

    let revoked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select revoked_at from access_tokens where id = $1")
            .bind(token_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(revoked.is_some());
    let _ = issued;
}

#[tokio::test]
async fn catalog_contains_public_builds_and_own_private_builds() {
    let env = TestEnv::new().await;
    let (public_id, _) = publish_public_build(&env).await;

    // Приватная сборка того же владельца (опубликованный релиз).
    let user = owner(&env).await;
    let (private_id, _) =
        publish_build(&env, user, "Private Pack", "private-pack", "private").await;

    // Владелец видит обе сборки.
    let own = catalog::catalog(&env.pool, user).await.unwrap();
    let own_slugs: Vec<String> = own.iter().map(|item| item.slug.clone()).collect();
    assert!(own_slugs.contains(&"public-pack".to_owned()));
    assert!(own_slugs.contains(&"private-pack".to_owned()));

    // Другой токен видит только публичную.
    let viewer = env.create_user("viewer", "SecretPass2!").await;
    let items = catalog::catalog(&env.pool, viewer).await.unwrap();
    let slugs: Vec<String> = items.iter().map(|item| item.slug.clone()).collect();
    assert!(slugs.contains(&"public-pack".to_owned()));
    assert!(!slugs.contains(&"private-pack".to_owned()));

    // Приватная сборка недоступна чужому и по slug.
    let error = catalog::build_by_slug(&env.pool, "private-pack", viewer).await;
    assert!(error.is_err());

    let found = catalog::build_by_slug(&env.pool, "public-pack", viewer)
        .await
        .unwrap();
    assert_eq!(found.id, public_id);

    // А владельцу — доступна.
    let own_found = catalog::build_by_slug(&env.pool, "private-pack", user)
        .await
        .unwrap();
    assert_eq!(own_found.id, private_id);
}

#[tokio::test]
async fn latest_manifest_matches_published_release() {
    let env = TestEnv::new().await;
    let (_, release_id) = publish_public_build(&env).await;
    let viewer = owner(&env).await;

    let manifest = catalog::latest_manifest(&env.pool, "public-pack", viewer)
        .await
        .unwrap();
    nexus::minecraft::manifests::validate_manifest(&manifest).unwrap();
    assert_eq!(manifest["schema_version"], "1.0");
    assert_eq!(manifest["build"]["release"], "1.0.0");
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);

    // Ссылка на артефакт соответствует записи снимка релиза.
    let artifact_id = files[0]["id"].as_str().unwrap().to_owned();
    let exists: Option<Uuid> = sqlx::query_scalar(
        "select id from minecraft_release_files where id = $1 and release_id = $2",
    )
    .bind(Uuid::parse_str(&artifact_id).unwrap())
    .bind(release_id)
    .fetch_optional(&env.pool)
    .await
    .unwrap();
    assert!(
        exists.is_some(),
        "манифест ссылается на запись снимка релиза"
    );

    // Тот же манифест доступен по версии.
    let by_version = catalog::release_manifest(&env.pool, "public-pack", "1.0.0", viewer)
        .await
        .unwrap();
    assert_eq!(manifest, by_version);
}

#[tokio::test]
async fn artifact_is_resolvable_and_hashed() {
    let env = TestEnv::new().await;
    publish_public_build(&env).await;
    let viewer = owner(&env).await;

    let manifest = catalog::latest_manifest(&env.pool, "public-pack", viewer)
        .await
        .unwrap();
    let artifact_id = Uuid::parse_str(manifest["files"][0]["id"].as_str().unwrap()).unwrap();
    let artifact = catalog::find_artifact(&env.pool, artifact_id, viewer)
        .await
        .unwrap();
    assert_eq!(artifact.path, "mods/a.jar");
    assert_eq!(artifact.sha256.len(), 64);
    assert_eq!(
        artifact.sha256,
        manifest["files"][0]["sha256"].as_str().unwrap()
    );

    let (location, size) = nexus::launcher::artifacts::resolve(&env.pool, &artifact)
        .await
        .unwrap();
    assert_eq!(size, b"launcher artifact".len() as i64);
    assert!(location.absolute.exists());
}

#[tokio::test]
async fn private_build_artifact_is_available_to_owner_only() {
    let env = TestEnv::new().await;
    let user = owner(&env).await;
    let (_, release_id) = publish_build(&env, user, "Secret", "secret", "private").await;
    let viewer = env.create_user("artifactviewer", "SecretPass2!").await;

    let artifact_id: Uuid =
        sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
            .bind(release_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();

    // Владелец получает собственный артефакт приватной сборки.
    let owned = catalog::find_artifact(&env.pool, artifact_id, user)
        .await
        .expect("владелец видит артефакт");
    assert_eq!(owned.path, "mods/a.jar");

    // Чужой токен получает 404 (не 403): существование не раскрывается.
    let error = catalog::find_artifact(&env.pool, artifact_id, viewer)
        .await
        .expect_err("чужой артефакт недоступен");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.code, "NOT_FOUND");
}

#[tokio::test]
async fn artifact_with_non_portable_path_is_hidden() {
    let env = TestEnv::new().await;
    let user = owner(&env).await;
    let (_, release_id) = publish_build(&env, user, "Portable", "portable", "public").await;

    let artifact_id: Uuid =
        sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
            .bind(release_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    // Путь, который на Windows означал бы другое расположение, не отдаётся.
    sqlx::query("update minecraft_release_files set path = 'mods\\a.jar' where id = $1")
        .bind(artifact_id)
        .execute(&env.pool)
        .await
        .unwrap();

    let error = catalog::find_artifact(&env.pool, artifact_id, user)
        .await
        .expect_err("небезопасный путь скрыт");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
}

/* --------------------- F4: приватные сборки в Launcher API ------------------ */

fn launcher_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("запрос собирается")
}

async fn issue_for(env: &TestEnv, username: &str, password: &str) -> String {
    tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "isolation".to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
            scopes: Some(vec![
                tokens::SCOPE_CATALOG.to_owned(),
                tokens::SCOPE_ARTIFACTS.to_owned(),
            ]),
            expires_in_hours: Some(24),
        },
    )
    .await
    .expect("токен лаунчера")
    .token
}

/// Критерий F4/F5: приватная сборка не утекает ни через один endpoint лаунчера.
/// Для чужого токена каждый метод отвечает 404 без названия и slug сборки,
/// а для владельца — работает как обычно.
#[tokio::test]
async fn foreign_token_cannot_see_private_build_on_any_launcher_endpoint() {
    let env = TestEnv::new().await;
    let owner_id = owner(&env).await;
    let (_, release_id) =
        publish_build(&env, owner_id, "Secret Pack", "secret-pack", "private").await;
    let (_, public_release_id) =
        publish_build(&env, owner_id, "Public Pack", "public-pack", "public").await;

    let private_artifact: Uuid =
        sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
            .bind(release_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    let public_artifact: Uuid =
        sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
            .bind(public_release_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();

    env.create_user("foreign", "SecretPass2!").await;
    let owner_token = issue_for(&env, "launchtester", "SecretPass1!").await;
    let foreign_token = issue_for(&env, "foreign", "SecretPass2!").await;
    let app = nexus::build_router(env.state.clone());

    let private_endpoints = [
        "/api/v1/launcher/builds/secret-pack".to_owned(),
        "/api/v1/launcher/builds/secret-pack/releases".to_owned(),
        "/api/v1/launcher/builds/secret-pack/latest/manifest".to_owned(),
        "/api/v1/launcher/builds/secret-pack/releases/1.0.0/manifest".to_owned(),
        format!("/api/v1/launcher/artifacts/{private_artifact}"),
    ];
    for endpoint in &private_endpoints {
        let foreign = app
            .clone()
            .oneshot(launcher_get(endpoint, &foreign_token))
            .await
            .expect("ответ");
        assert_eq!(
            foreign.status(),
            StatusCode::NOT_FOUND,
            "чужой токен: {endpoint}"
        );
        let body = axum::body::to_bytes(foreign.into_body(), 64 * 1024)
            .await
            .expect("тело");
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("secret-pack") && !text.contains("Secret Pack"),
            "404 не должен раскрывать сборку: {text}"
        );

        let owned = app
            .clone()
            .oneshot(launcher_get(endpoint, &owner_token))
            .await
            .expect("ответ");
        assert_eq!(owned.status(), StatusCode::OK, "владелец: {endpoint}");
    }

    // Списочные методы: чужая приватная сборка отсутствует, своя — есть.
    for endpoint in ["/api/v1/launcher/catalog", "/api/v1/launcher/builds"] {
        let foreign = app
            .clone()
            .oneshot(launcher_get(endpoint, &foreign_token))
            .await
            .expect("ответ");
        assert_eq!(foreign.status(), StatusCode::OK);
        let body = axum::body::to_bytes(foreign.into_body(), 128 * 1024)
            .await
            .expect("тело");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("public-pack"), "публичная видна всем");
        assert!(
            !text.contains("secret-pack"),
            "приватная не утекает: {endpoint}"
        );

        let owned = app
            .clone()
            .oneshot(launcher_get(endpoint, &owner_token))
            .await
            .expect("ответ");
        let body = axum::body::to_bytes(owned.into_body(), 128 * 1024)
            .await
            .expect("тело");
        assert!(
            String::from_utf8_lossy(&body).contains("secret-pack"),
            "владелец видит свою приватную сборку: {endpoint}"
        );
    }

    // Публичная сборка доступна чужому токену, включая артефакт.
    let public_build = app
        .clone()
        .oneshot(launcher_get(
            "/api/v1/launcher/builds/public-pack",
            &foreign_token,
        ))
        .await
        .expect("ответ");
    assert_eq!(public_build.status(), StatusCode::OK);
    let public_download = app
        .clone()
        .oneshot(launcher_get(
            &format!("/api/v1/launcher/artifacts/{public_artifact}"),
            &foreign_token,
        ))
        .await
        .expect("ответ");
    assert_eq!(public_download.status(), StatusCode::OK);
    let body = axum::body::to_bytes(public_download.into_body(), 64 * 1024)
        .await
        .expect("тело");
    assert_eq!(&body[..], b"launcher artifact");
}

#[tokio::test]
async fn manifest_icon_matches_storage_and_is_not_leaked_to_foreign_token() {
    let env = TestEnv::new().await;
    let owner_id = owner(&env).await;
    env.add_volume("icon-launcher-vol").await;

    let build = builds::create(
        &env.pool,
        owner_id,
        builds::CreateBuildRequest {
            name: "Icon Pack".to_owned(),
            slug: Some("icon-pack".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
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

    // PNG определяется по сигнатуре, а не по расширению.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[42u8; 48]);
    builds::set_icon(&env.state, build.id, owner_id, png.clone())
        .await
        .unwrap();

    let payload = b"icon build artifact";
    let session = uploads::create_upload(
        &env.state,
        owner_id,
        build.id,
        uploads::CreateUploadRequest {
            path: "mods/a.jar".to_owned(),
            size: payload.len() as i64,
            category: Some("mods".to_owned()),
            side: Some("client".to_owned()),
            required: Some(true),
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .unwrap();
    uploads::append_stream(
        &env.state,
        session.id,
        owner_id,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .unwrap();
    uploads::complete_upload(&env.state, session.id, owner_id)
        .await
        .unwrap();

    let release = releases::create(
        &env.pool,
        owner_id,
        build.id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner_id, "tester", build.id, release.id, true)
        .await
        .unwrap();

    let stored_sha: String = sqlx::query_scalar(
        "select o.sha256
         from minecraft_builds b join storage_objects o on o.id = b.icon_object_id
         where b.id = $1",
    )
    .bind(build.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();

    let owner_token = issue_for(&env, "launchtester", "SecretPass1!").await;
    let app = nexus::build_router(env.state.clone());

    // Манифест опубликованного релиза содержит иконку с SHA объекта.
    let manifest = app
        .clone()
        .oneshot(launcher_get(
            "/api/v1/launcher/builds/icon-pack/latest/manifest",
            &owner_token,
        ))
        .await
        .expect("ответ");
    assert_eq!(manifest.status(), StatusCode::OK);
    let body = axum::body::to_bytes(manifest.into_body(), 128 * 1024)
        .await
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let icon = &document["data"]["icon"];
    assert_eq!(
        icon["url"],
        serde_json::json!("/api/v1/launcher/builds/icon-pack/icon")
    );
    assert_eq!(icon["sha256"], serde_json::json!(stored_sha));

    // Иконка отдаётся владельцу с типом по сигнатуре и совпадающим телом.
    let icon_response = app
        .clone()
        .oneshot(launcher_get(
            "/api/v1/launcher/builds/icon-pack/icon",
            &owner_token,
        ))
        .await
        .expect("ответ");
    assert_eq!(icon_response.status(), StatusCode::OK);
    assert_eq!(
        icon_response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let body = axum::body::to_bytes(icon_response.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], &png[..]);

    // Чужой токен не получает ни манифест, ни иконку приватной сборки.
    env.create_user("foreign-icon", "SecretPass2!").await;
    let foreign_token = issue_for(&env, "foreign-icon", "SecretPass2!").await;
    for endpoint in [
        "/api/v1/launcher/builds/icon-pack/latest/manifest",
        "/api/v1/launcher/builds/icon-pack/icon",
    ] {
        let response = app
            .clone()
            .oneshot(launcher_get(endpoint, &foreign_token))
            .await
            .expect("ответ");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{endpoint}");
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("icon-pack"),
            "404 не раскрывает сборку: {text}"
        );
    }

    // После публикации сборка становится доступна чужому токену.
    sqlx::query("update minecraft_builds set visibility = 'public' where id = $1")
        .bind(build.id)
        .execute(&env.pool)
        .await
        .unwrap();
    let public_icon = app
        .clone()
        .oneshot(launcher_get(
            "/api/v1/launcher/builds/icon-pack/icon",
            &foreign_token,
        ))
        .await
        .expect("ответ");
    assert_eq!(public_icon.status(), StatusCode::OK);
    let body = axum::body::to_bytes(public_icon.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], &png[..]);
}

#[tokio::test]
async fn deprecated_release_remains_available() {
    let env = TestEnv::new().await;
    let (build_id, release_id) = publish_public_build(&env).await;
    let user = owner(&env).await;

    releases::set_status(
        &env.pool,
        user,
        build_id,
        release_id,
        "deprecated",
        "release.deprecated",
    )
    .await
    .unwrap();

    // Манифест устаревшего релиза по-прежнему отдаётся по явной версии.
    let manifest = catalog::release_manifest(&env.pool, "public-pack", "1.0.0", user).await;
    assert!(manifest.is_ok());
}

#[tokio::test]
async fn download_log_is_recorded() {
    let env = TestEnv::new().await;
    publish_public_build(&env).await;

    let count: i64 = sqlx::query_scalar("select count(*) from launcher_downloads")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "журнал пуст до первого скачивания");

    // Прямая запись через хелпер, как это делает artifacts::download.
    sqlx::query(
        "insert into launcher_downloads (path, bytes_sent, status, range_header)
         values ('mods/a.jar', 17, 200, '')",
    )
    .execute(&env.pool)
    .await
    .unwrap();

    let row: (String, i64, i32) =
        sqlx::query_as("select path, bytes_sent, status from launcher_downloads limit 1")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "mods/a.jar");
    assert_eq!(row.1, 17);
    assert_eq!(row.2, 200);
}

/* ------------------- F3: параллельные скачивания артефактов ----------------- */

async fn artifact_token(env: &TestEnv) -> (String, Uuid) {
    let issued = tokens::issue_token(
        &env.state,
        tokens::CreateTokenRequest {
            name: "downloads".to_owned(),
            username: "launchtester".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: Some(vec![
                tokens::SCOPE_CATALOG.to_owned(),
                tokens::SCOPE_ARTIFACTS.to_owned(),
            ]),
            expires_in_hours: Some(24),
        },
    )
    .await
    .expect("токен лаунчера");
    let token_id: Uuid = sqlx::query_scalar("select id from access_tokens where token_hash = $1")
        .bind(tokens::hash_token(&issued.token))
        .fetch_one(&env.pool)
        .await
        .expect("токен в БД");
    (issued.token, token_id)
}

async fn published_artifact(env: &TestEnv) -> Uuid {
    publish_public_build(env).await;
    let viewer = owner(env).await;
    let manifest = catalog::latest_manifest(&env.pool, "public-pack", viewer)
        .await
        .expect("манифест");
    Uuid::parse_str(
        manifest["files"][0]["id"]
            .as_str()
            .expect("id артефакта в манифесте"),
    )
    .expect("корректный UUID")
}

fn artifact_request(artifact_id: Uuid, token: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/launcher/artifacts/{artifact_id}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("запрос собирается")
}

/// Один объект читается восемью потоками одновременно: `File::open` на Windows
/// открывает файл в режиме совместного чтения, блокировок нет.
#[tokio::test]
async fn same_artifact_is_read_concurrently_without_locking() {
    let env = TestEnv::new().await;
    let artifact_id = published_artifact(&env).await;
    let (token, _) = artifact_token(&env).await;
    let app = nexus::build_router(env.state.clone());

    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    for _ in 0..8 {
        let app = app.clone();
        let token = token.clone();
        let barrier = barrier.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let response = app
                .oneshot(artifact_request(artifact_id, &token))
                .await
                .expect("ответ");
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("тело");
            sender.send((status, body)).await.expect("канал");
        });
    }
    drop(sender);

    let mut results = Vec::new();
    while let Some(item) = receiver.recv().await {
        results.push(item);
    }
    assert_eq!(results.len(), 8, "все восемь чтений завершились");
    for (status, body) in results {
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"launcher artifact");
    }
    // Потоки завершились — слоты освобождены.
    assert_eq!(env.state.downloads.total_active(), 0);
    assert_eq!(env.state.requests.active_downloads(), 0);
}

/// Приёмка F3: 8 одновременных скачиваний удерживают слоты, 9-е ограничено
/// (`503` + `Retry-After`), а после завершения (в том числе обрыва клиента)
/// доступ восстанавливается.
///
/// Синхронизация — барьер и канал, без `sleep`: тест не зависит от скорости
/// NTFS и антивируса.
#[tokio::test]
async fn ninth_parallel_download_is_limited_and_slots_recover() {
    let env = TestEnv::new().await;
    let artifact_id = published_artifact(&env).await;
    let (token, token_id) = artifact_token(&env).await;
    let app = nexus::build_router(env.state.clone());

    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    for _ in 0..8 {
        let app = app.clone();
        let token = token.clone();
        let barrier = barrier.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let response = app
                .oneshot(artifact_request(artifact_id, &token))
                .await
                .expect("ответ");
            sender.send(response).await.expect("канал");
        });
    }
    drop(sender);

    // Тела не читаем: слот удерживается до конца потока, как у реального клиента.
    let mut held = Vec::new();
    while let Some(response) = receiver.recv().await {
        held.push(response);
    }
    assert_eq!(held.len(), 8);
    for response in &held {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-ratelimit-limit")
                .and_then(|value| value.to_str().ok()),
            Some("600")
        );
    }
    assert_eq!(env.state.downloads.active(token_id), 8);

    let limited = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(limited.status(), StatusCode::SERVICE_UNAVAILABLE);
    let retry_after: u64 = limited
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("Retry-After присутствует");
    assert!(retry_after >= 1);
    let body = axum::body::to_bytes(limited.into_body(), 64 * 1024)
        .await
        .expect("тело ошибки");
    assert!(String::from_utf8_lossy(&body).contains("DOWNLOAD_LIMIT_REACHED"));
    assert_eq!(env.state.downloads.active(token_id), 8);

    // Обрыв клиента: ответ отброшен без чтения тела — слот освобождается.
    held.pop();
    let next = app
        .clone()
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(next.status(), StatusCode::OK, "слот освободился сразу");

    // После завершения всех потоков лимит снова полностью доступен.
    drop(held);
    drop(next);
    assert_eq!(env.state.downloads.active(token_id), 0);
    let again = app
        .oneshot(artifact_request(artifact_id, &token))
        .await
        .expect("ответ");
    assert_eq!(again.status(), StatusCode::OK);
    assert!(again.headers().get("x-file-sha256").is_some());
}

/* ------------------------------------------------------------------------- */
/* I4: срок действия, способы передачи токена и права администратора.         */
/* ------------------------------------------------------------------------- */

async fn send_launcher(env: &TestEnv, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("ответ");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

fn launcher_request(uri: &str, token_kind: Option<(&str, &str)>) -> Request<Body> {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some((name, value)) = token_kind {
        builder = builder.header(name, value);
    }
    builder.body(Body::empty()).unwrap()
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

#[tokio::test]
async fn expired_token_is_rejected_on_catalog() {
    let env = TestEnv::new().await;
    owner(&env).await;
    let token = issue_for(&env, "launchtester", "SecretPass1!").await;
    sqlx::query(
        "update access_tokens set expires_at = now() - interval '1 minute' where token_hash = $1",
    )
    .bind(tokens::hash_token(&token))
    .execute(&env.pool)
    .await
    .unwrap();

    let bearer = format!("Bearer {token}");
    let (status, body) = send_launcher(
        &env,
        launcher_request(
            "/api/v1/launcher/catalog",
            Some(("authorization", bearer.as_str())),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "LAUNCHER_TOKEN_EXPIRED");

    let used: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select last_used_at from access_tokens limit 1")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(
        used.is_none(),
        "отклонённый токен не отмечается использованным"
    );
}

#[tokio::test]
async fn missing_and_invalid_tokens_are_rejected_and_header_form_works() {
    let env = TestEnv::new().await;
    owner(&env).await;

    let (status, body) =
        send_launcher(&env, launcher_request("/api/v1/launcher/catalog", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "LAUNCHER_TOKEN_REQUIRED");

    let (status, body) = send_launcher(
        &env,
        launcher_request(
            "/api/v1/launcher/catalog",
            Some(("authorization", "Bearer nxl_not-a-real-token")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "LAUNCHER_TOKEN_INVALID");

    let token = issue_for(&env, "launchtester", "SecretPass1!").await;
    let (status, body) = send_launcher(
        &env,
        launcher_request(
            "/api/v1/launcher/catalog",
            Some(("x-launcher-token", token.as_str())),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["builds"].is_array());
}

#[tokio::test]
async fn admin_token_endpoints_require_manage_permission() {
    let env = TestEnv::new().await;
    let admin = owner(&env).await;
    let plain = create_plain_user(&env, "launcher-plain").await;
    let admin_cookie = session_cookie(&env, admin).await;
    let plain_cookie = session_cookie(&env, plain).await;
    let _ = issue_for(&env, "launchtester", "SecretPass1!").await;

    let (status, _) = send_launcher(
        &env,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/admin/launcher/tokens")
            .header(header::COOKIE, &plain_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send_launcher(
        &env,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/admin/launcher/tokens")
            .header(header::COOKIE, &admin_cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["scopes"].is_array());
    assert!(
        items[0].get("tokenHash").is_none(),
        "хэш токена не отдаётся"
    );
    assert!(items[0].get("token_hash").is_none());
}
