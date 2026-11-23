//! Minecraft: иконка, замена файла, конфликт пути (PLAN §7.2, §7.3).

mod common;

use axum::body::Body;
use common::TestEnv;
use nexus::minecraft::{builds, files, uploads};
use uuid::Uuid;

async fn setup_build(env: &TestEnv, user: Uuid, slug: &str) -> Uuid {
    builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: format!("Build {slug}"),
            slug: Some(slug.to_owned()),
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
    .expect("сборка")
    .id
}

async fn upload_into(
    env: &TestEnv,
    build: Uuid,
    user: Uuid,
    path: &str,
    payload: &[u8],
) -> nexus::minecraft::files::FileRow {
    let session = uploads::create_upload(
        &env.state,
        user,
        build,
        uploads::CreateUploadRequest {
            path: path.to_owned(),
            size: payload.len() as i64,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "application/octet-stream".to_owned(),
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
    uploads::complete_upload(&env.state, session.id, user)
        .await
        .expect("фиксация")
}

#[tokio::test]
async fn invalid_icon_signature_is_rejected() {
    let env = TestEnv::new().await;
    env.add_volume("icon-vol").await;
    let user = env.create_user("iconuser", "SecretPass1!").await;
    let build = setup_build(&env, user, "icon-build").await;

    // Не PNG/JPEG/WEBP: сигнатура не совпадает.
    let error = builds::set_icon(&env.state, build, user, b"GIF89a-not-allowed".to_vec())
        .await
        .expect_err("неверная сигнатура");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn oversized_icon_is_rejected() {
    let env = TestEnv::new().await;
    env.add_volume("icon2-vol").await;
    let user = env.create_user("iconuser2", "SecretPass1!").await;
    let build = setup_build(&env, user, "icon-build-2").await;

    let mut payload = vec![0u8; (builds::ICON_MAX_BYTES + 1) as usize];
    payload[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let error = builds::set_icon(&env.state, build, user, payload)
        .await
        .expect_err("слишком большая иконка");
    assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn valid_icon_is_stored_and_served() {
    let env = TestEnv::new().await;
    env.add_volume("icon3-vol").await;
    let user = env.create_user("iconuser3", "SecretPass1!").await;
    let build = setup_build(&env, user, "icon-build-3").await;

    // Минимальный PNG: сигнатура + немного данных.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0u8; 64]);
    builds::set_icon(&env.state, build, user, png.clone())
        .await
        .unwrap();

    let row = builds::load(&env.pool, build).await.unwrap();
    assert!(row.icon_url.is_some(), "иконка отражена в карточке");

    let icon = builds::icon_object(&env.pool, build)
        .await
        .unwrap()
        .expect("иконка сохранена");
    assert_eq!(icon.content_type, "image/png");
    assert_eq!(icon.size as usize, png.len());
    assert_eq!(icon.sha256.len(), 64);
    assert!(icon.location.absolute.exists());

    // Удаление освобождает объект.
    builds::remove_icon(&env.pool, build).await.unwrap();
    let row = builds::load(&env.pool, build).await.unwrap();
    assert!(row.icon_url.is_none());
}

#[tokio::test]
async fn replacing_file_updates_size_and_sha() {
    let env = TestEnv::new().await;
    env.add_volume("replace-file-vol").await;
    let user = env.create_user("replacer", "SecretPass1!").await;
    let build = setup_build(&env, user, "replace-file").await;

    let original = upload_into(&env, build, user, "mods/a.jar", b"original").await;
    assert_eq!(original.size_bytes, 8);

    // Новая загрузка, фиксируемая поверх существующего пути.
    let session = uploads::create_replace_upload(
        &env.state,
        user,
        build,
        uploads::CreateUploadRequest {
            path: "mods/a.jar".to_owned(),
            size: 11,
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
        Body::from(b"replacement".to_vec()),
    )
    .await
    .unwrap();
    let replaced = uploads::complete_upload_into(&env.state, session.id, user, "mods/a.jar")
        .await
        .unwrap();

    assert_eq!(replaced.id, original.id, "запись та же");
    assert_eq!(replaced.size_bytes, 11);
    assert_ne!(replaced.sha256, original.sha256);

    let file = files::load_file(&env.pool, build, original.id)
        .await
        .unwrap();
    assert_eq!(file.path, "mods/a.jar");
    assert_eq!(file.size_bytes, 11);
}

#[tokio::test]
async fn renaming_to_existing_path_conflicts() {
    let env = TestEnv::new().await;
    env.add_volume("conflict-vol").await;
    let user = env.create_user("renamer", "SecretPass1!").await;
    let build = setup_build(&env, user, "conflict-build").await;

    let _ = upload_into(&env, build, user, "mods/a.jar", b"a").await;
    let second = upload_into(&env, build, user, "mods/b.jar", b"b").await;

    let error = files::update_file(
        &env.pool,
        build,
        second.id,
        files::UpdateFileRequest {
            path: Some("mods/a.jar".to_owned()),
            category: None,
            side: None,
            required: None,
            enabled: None,
            description: None,
        },
    )
    .await
    .expect_err("конфликт пути");
    assert_eq!(error.code, "BUILD_PATH_CONFLICT");
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn rename_preserves_sha_and_moving_folder_updates_children() {
    let env = TestEnv::new().await;
    env.add_volume("rename-vol").await;
    let user = env.create_user("renamer2", "SecretPass1!").await;
    let build = setup_build(&env, user, "rename-build").await;

    let file = upload_into(&env, build, user, "mods/a.jar", b"content").await;
    let renamed = files::update_file(
        &env.pool,
        build,
        file.id,
        files::UpdateFileRequest {
            path: Some("mods/newname.jar".to_owned()),
            category: None,
            side: None,
            required: None,
            enabled: None,
            description: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(renamed.path, "mods/newname.jar");
    assert_eq!(
        renamed.sha256, file.sha256,
        "переименование не меняет содержимое"
    );

    // Перемещение папки config обновляет путь вложенного файла.
    let folder = files::create_folder(&env.pool, build, "config/old")
        .await
        .unwrap();
    let nested = upload_into(&env, build, user, "config/old/settings.toml", b"x = 1").await;
    let moved = files::update_file(
        &env.pool,
        build,
        folder.id,
        files::UpdateFileRequest {
            path: Some("config/new".to_owned()),
            category: None,
            side: None,
            required: None,
            enabled: None,
            description: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.path, "config/new");

    let child = files::load_file(&env.pool, build, nested.id).await.unwrap();
    assert_eq!(
        child.path, "config/new/settings.toml",
        "вложенный файл переехал вместе с папкой"
    );
}

#[test]
fn icon_type_sniffing() {
    assert_eq!(
        builds::sniff_image_type(b"\x89PNG\r\n\x1a\nxxx"),
        Some(("image/png", "png"))
    );
    assert_eq!(
        builds::sniff_image_type(b"\xff\xd8\xffxx"),
        Some(("image/jpeg", "jpg"))
    );
    assert_eq!(
        builds::sniff_image_type(b"RIFF\0\0\0\0WEBPVP8 "),
        Some(("image/webp", "webp"))
    );
    assert_eq!(builds::sniff_image_type(b"<svg></svg>"), None);
}
