//! Contract-тесты публичного репозитория MyLauncher.
//!
//! Запросы и ожидаемые формы повторяют `src-tauri/src/modpack.rs` клиента:
//! `/api/packs`, `/api/packs/{id}/versions`, манифест `manifestVersion: 1`
//! и `/api/files/sha256:{hex}` с Range.

mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::minecraft::{builds, releases, uploads};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

async fn user(env: &TestEnv) -> Uuid {
    env.create_user("repo_tester", "SecretPass1!").await
}

async fn make_build(
    env: &TestEnv,
    user: Uuid,
    slug: &str,
    visibility: &str,
    loader: &str,
    loader_version: &str,
) -> Uuid {
    env.add_volume(&format!("{slug}-vol")).await;
    builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: format!("Pack {slug}"),
            slug: Some(slug.to_owned()),
            description: "Contract pack".to_owned(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
            visibility: visibility.to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: loader.to_owned(),
            loader_version: loader_version.to_owned(),
            java_major: Some(17),
            java_min: Some(17),
            java_notes: String::new(),
        },
    )
    .await
    .unwrap()
    .id
}

async fn upload(env: &TestEnv, user: Uuid, build: Uuid, path: &str, payload: &[u8]) {
    let session = uploads::create_upload(
        &env.state,
        user,
        build,
        uploads::CreateUploadRequest {
            path: path.to_owned(),
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
}

async fn publish(env: &TestEnv, user: Uuid, build: Uuid, version: &str, channel: &str) -> Uuid {
    let release = releases::create(
        &env.pool,
        user,
        build,
        releases::CreateReleaseRequest {
            version: Some(version.to_owned()),
            changelog: String::new(),
            channel: Some(channel.to_owned()),
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, user, "tester", build, release.id, true)
        .await
        .expect("публикация лаунчер-совместимой сборки");
    release.id
}

async fn get(
    env: &TestEnv,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = nexus::build_router(env.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

async fn get_json(env: &TestEnv, uri: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = get(env, uri, &[]).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn catalog_versions_manifest_and_file_match_client_contract() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "contract-pack", "public", "forge", "47.4.10").await;
    upload(&env, user, build, "mods/a.jar", b"contract-payload").await;
    publish(&env, user, build, "1.0.0", "stable").await;

    let (status, catalog) = get_json(&env, "/api/packs?channel=stable").await;
    assert_eq!(status, StatusCode::OK);
    let packs = catalog["packs"].as_array().expect("packs[]");
    assert_eq!(packs.len(), 1);
    let pack = &packs[0];
    assert_eq!(pack["id"], "contract-pack");
    assert_eq!(pack["latestVersion"], "1.0.0");
    assert_eq!(pack["channel"], "stable");
    assert!(pack["authors"].is_array());
    assert!(pack["categories"].is_array());
    assert!(pack["iconUrl"].is_null());

    let (status, versions) =
        get_json(&env, "/api/packs/contract-pack/versions?channel=stable").await;
    assert_eq!(status, StatusCode::OK);
    let versions = versions["versions"].as_array().expect("versions[]");
    assert_eq!(versions[0]["version"], "1.0.0");
    assert_eq!(versions[0]["channel"], "stable");
    assert!(versions[0]["updatedAt"].is_i64(), "updatedAt — секунды i64");

    let (status, manifest) = get_json(
        &env,
        "/api/packs/contract-pack/versions/1.0.0/manifest?channel=stable",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["manifestVersion"], 1);
    assert_eq!(manifest["packId"], "contract-pack");
    assert_eq!(manifest["version"], "1.0.0");
    assert_eq!(manifest["minecraft"]["version"], "1.20.1");
    assert_eq!(manifest["minecraft"]["loader"], "forge");
    assert_eq!(manifest["minecraft"]["loaderVersion"], "47.4.10");
    assert_eq!(manifest["java"]["major"], 17);
    let file = &manifest["files"][0];
    assert_eq!(file["path"], "mods/a.jar");
    assert_eq!(file["size"], 16);
    let hash = file["hash"].as_str().expect("hash");
    assert!(hash.starts_with("sha256:"), "префикс sha256:");
    assert_eq!(hash.len(), 7 + 64);
    assert_eq!(file["url"], format!("/api/files/{hash}"));
    assert_eq!(file["required"], true);

    let (status, headers, bytes) = get(
        &env,
        &format!("/api/files/{hash}?packId=contract-pack&version=1.0.0&channel=stable"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"contract-payload");
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(
        headers
            .get("x-file-sha256")
            .and_then(|value| value.to_str().ok()),
        Some(&hash[7..])
    );

    let (status, headers, bytes) = get(
        &env,
        &format!("/api/files/{hash}?packId=contract-pack&version=1.0.0&channel=stable"),
        &[("range", "bytes=9-")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes, b"payload");
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 9-15/16");

    let (status, headers, _) = get(
        &env,
        &format!("/api/files/{hash}?packId=contract-pack&version=1.0.0&channel=stable"),
        &[("range", "bytes=999-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(headers[header::CONTENT_RANGE], "bytes */16");
}

#[tokio::test]
async fn channels_select_independent_latest_versions() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "channel-pack", "public", "fabric", "0.15.11").await;
    upload(&env, user, build, "mods/a.jar", b"a").await;
    publish(&env, user, build, "1.0.0", "stable").await;
    publish(&env, user, build, "1.1.0-beta", "beta").await;

    let (_, stable) = get_json(&env, "/api/packs?channel=stable").await;
    assert_eq!(stable["packs"][0]["latestVersion"], "1.0.0");
    let (_, beta) = get_json(&env, "/api/packs?channel=beta").await;
    assert_eq!(beta["packs"][0]["latestVersion"], "1.1.0-beta");

    let (_, versions) = get_json(&env, "/api/packs/channel-pack/versions?channel=beta").await;
    assert_eq!(versions["versions"].as_array().unwrap().len(), 1);
    assert_eq!(versions["versions"][0]["version"], "1.1.0-beta");

    let (status, manifest) = get_json(
        &env,
        "/api/packs/channel-pack/versions/1.1.0-beta/manifest?channel=beta",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["channel"], "beta");

    let (status, _) = get_json(
        &env,
        "/api/packs/channel-pack/versions/1.1.0-beta/manifest?channel=stable",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "версия другого канала скрыта"
    );
}

#[tokio::test]
async fn private_pack_requires_token_and_revocation_is_immediate() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "private-pack", "private", "vanilla", "").await;
    upload(&env, user, build, "config/x.json", b"{}").await;
    publish(&env, user, build, "1.0.0", "stable").await;

    let (_, anonymous) = get_json(&env, "/api/packs?channel=stable").await;
    assert_eq!(anonymous["packs"].as_array().unwrap().len(), 0);

    let issued = nexus::launcher::tokens::issue_token(
        &env.state,
        nexus::launcher::tokens::CreateTokenRequest {
            name: "contract".to_owned(),
            username: "repo_tester".to_owned(),
            password: "SecretPass1!".to_owned(),
            scopes: Some(vec![
                nexus::launcher::tokens::SCOPE_CATALOG.to_owned(),
                nexus::launcher::tokens::SCOPE_ARTIFACTS.to_owned(),
            ]),
            expires_in_hours: Some(1),
        },
    )
    .await
    .unwrap();
    let auth = format!("Bearer {}", issued.token);
    let (status, catalog) = get_json_auth(&env, "/api/packs?channel=stable", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["packs"][0]["id"], "private-pack");

    // Отзыв: следующее обращение получает 401.
    sqlx::query("update access_tokens set revoked_at = now() where token_hash = $1")
        .bind(nexus::launcher::tokens::hash_token(&issued.token))
        .execute(&env.pool)
        .await
        .unwrap();
    let (status, _) = get_json_auth(&env, "/api/packs?channel=stable", &auth).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

async fn get_json_auth(env: &TestEnv, uri: &str, token: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = get(env, uri, &[("authorization", token)]).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn archiving_current_release_keeps_other_channel_visible() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "archive-pack", "public", "vanilla", "").await;
    upload(&env, user, build, "mods/a.jar", b"a").await;
    let beta = publish(&env, user, build, "1.1.0-beta", "beta").await;
    let stable = publish(&env, user, build, "1.0.0", "stable").await;
    releases::set_status(
        &env.pool,
        user,
        build,
        stable,
        "archived",
        "release.archived",
    )
    .await
    .unwrap();

    let (_, stable_catalog) = get_json(&env, "/api/packs?channel=stable").await;
    assert_eq!(
        stable_catalog["packs"].as_array().unwrap().len(),
        0,
        "архивный канал пуст"
    );
    let (_, beta_catalog) = get_json(&env, "/api/packs?channel=beta").await;
    assert_eq!(beta_catalog["packs"][0]["latestVersion"], "1.1.0-beta");
    assert_eq!(beta_catalog["packs"][0]["id"], "archive-pack");

    let (status, manifest) = get_json(
        &env,
        "/api/packs/archive-pack/versions/1.1.0-beta/manifest?channel=beta",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["version"], "1.1.0-beta");
    let _ = beta;
}

#[tokio::test]
async fn rollback_to_previous_release_is_reflected_in_catalog_and_versions() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "rollback-pack", "public", "vanilla", "").await;
    upload(&env, user, build, "mods/a.jar", b"a").await;
    let first = publish(&env, user, build, "1.0.0", "stable").await;
    publish(&env, user, build, "1.1.0", "stable").await;

    // Откат на предыдущий релиз: лаунчер должен увидеть именно текущий релиз,
    // а не последний по времени публикации.
    releases::set_current(&env.pool, user, build, first)
        .await
        .unwrap();

    let (status, catalog) = get_json(&env, "/api/packs?channel=stable").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["packs"][0]["latestVersion"], "1.0.0");

    let (status, versions) =
        get_json(&env, "/api/packs/rollback-pack/versions?channel=stable").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        versions["versions"][0]["version"], "1.0.0",
        "текущий релиз идёт первым"
    );
}

#[tokio::test]
async fn published_manifest_is_immutable_after_build_edits() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "immutable-pack", "public", "vanilla", "").await;
    upload(&env, user, build, "mods/a.jar", b"first").await;
    publish(&env, user, build, "1.0.0", "stable").await;

    let uri = "/api/packs/immutable-pack/versions/1.0.0/manifest?channel=stable";
    let (_, before) = get_json(&env, uri).await;
    let hash_before = before["files"][0]["hash"].clone();

    // Правки сборки после публикации не меняют опубликованный снимок.
    upload(&env, user, build, "mods/b.jar", b"second").await;
    sqlx::query(
        "update minecraft_builds set description = 'changed', name = 'Changed' where id = $1",
    )
    .bind(build)
    .execute(&env.pool)
    .await
    .unwrap();

    let (_, after) = get_json(&env, uri).await;
    assert_eq!(after, before, "опубликованный манифест неизменяем");
    assert_eq!(after["files"].as_array().unwrap().len(), 1);
    assert_eq!(after["files"][0]["hash"], hash_before);
    assert_eq!(after["name"], "Pack immutable-pack");
}

#[tokio::test]
async fn removed_files_are_absent_from_next_manifest() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "update-pack", "public", "vanilla", "").await;
    upload(&env, user, build, "mods/a.jar", b"a").await;
    upload(&env, user, build, "mods/b.jar", b"b").await;
    publish(&env, user, build, "1.0.0", "stable").await;

    sqlx::query("delete from minecraft_build_files where build_id = $1 and path = 'mods/b.jar'")
        .bind(build)
        .execute(&env.pool)
        .await
        .unwrap();
    publish(&env, user, build, "1.1.0", "stable").await;

    let (status, manifest) = get_json(
        &env,
        "/api/packs/update-pack/versions/1.1.0/manifest?channel=stable",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let paths: Vec<&str> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(paths, vec!["mods/a.jar"], "исчезнувший файл не отдаётся");
}

#[tokio::test]
async fn launcher_incompatible_path_blocks_publication() {
    let env = TestEnv::new().await;
    let user = user(&env).await;
    let build = make_build(&env, user, "unicode-pack", "public", "vanilla", "").await;
    // Новый контракт блокирует несовместимый путь уже при загрузке, а не
    // только на публикации.
    let error = uploads::create_upload(
        &env.state,
        user,
        build,
        uploads::CreateUploadRequest {
            path: "mods/файл.jar".to_owned(),
            size: 1,
            category: Some("mods".to_owned()),
            side: Some("client".to_owned()),
            required: Some(true),
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .expect_err("несовместимый путь должен отклоняться при загрузке");
    assert_eq!(error.code, "VALIDATION_ERROR");
}

#[tokio::test]
async fn public_catalog_is_rate_limited() {
    let env = TestEnv::new().await;
    let mut limited = false;
    for _ in 0..140 {
        let (status, _) = get_json(&env, "/api/packs?channel=stable").await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "после лимита каталог должен вернуть 429");
}
