//! Интеграционные тесты Minecraft Build Manager: пути, релизы, манифест, неизменяемость.

mod common;

use axum::body::Body;
use common::TestEnv;
use nexus::minecraft::{builds, checks, files, manifests, releases, uploads, validation};
use serde_json::{Value, json};
use uuid::Uuid;

async fn test_user(env: &TestEnv) -> Uuid {
    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from users where username = 'mctester'")
            .fetch_optional(&env.pool)
            .await
            .unwrap();
    match existing {
        Some(id) => id,
        None => env.create_user("mctester", "SecretPass1!").await,
    }
}

async fn create_build(env: &TestEnv) -> Uuid {
    create_build_with_slug(env, "test-build").await
}

async fn create_build_with_slug(env: &TestEnv, slug: &str) -> Uuid {
    let owner = test_user(env).await;
    let build = builds::create(
        &env.pool,
        owner,
        builds::CreateBuildRequest {
            name: format!("Test Build {slug}"),
            slug: Some(slug.to_owned()),
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
    .expect("создание сборки");
    build.id
}

async fn upload_artifact(env: &TestEnv, build_id: Uuid, path: &str, payload: &[u8]) -> Uuid {
    let owner = test_user(env).await;
    let session = uploads::create_upload(
        &env.state,
        owner,
        build_id,
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
    .expect("сессия артефакта");
    uploads::append_stream(
        &env.state,
        session.id,
        owner,
        0,
        Body::from(payload.to_vec()),
    )
    .await
    .expect("чанк");
    let file = uploads::complete_upload(&env.state, session.id, owner)
        .await
        .expect("фиксация артефакта");
    file.id
}

#[test]
fn build_paths_are_validated_against_escapes() {
    assert!(validation::normalize_path("mods/example.jar").is_ok());
    assert!(validation::normalize_path("config\\sub\\a.toml").is_ok());
    assert!(validation::normalize_path("../escape").is_err());
    assert!(validation::normalize_path("mods/../../etc").is_err());
    assert!(validation::normalize_path("/abs/path").is_err());
    assert!(validation::normalize_path("C:/Windows/system32").is_err());
    assert!(validation::normalize_path("mods/CON.jar").is_err());
    assert!(validation::normalize_path("mods/a?.jar").is_err());
    assert!(validation::normalize_path("mods/trailing.").is_err());
}

#[test]
fn slugs_and_versions_are_strict() {
    assert!(validation::validate_slug("create-expert").is_ok());
    assert!(validation::validate_slug("Create Expert").is_err());
    assert!(validation::validate_slug("-bad").is_err());
    assert!(validation::validate_version("1.0.0").is_ok());
    assert!(validation::validate_version("1.0 beta").is_err());
}

#[tokio::test]
async fn build_creation_generates_slug_and_rejects_duplicates() {
    let env = TestEnv::new().await;
    let owner = test_user(&env).await;
    let first = builds::create(
        &env.pool,
        owner,
        builds::CreateBuildRequest {
            name: "My Cool Pack".to_owned(),
            slug: None,
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: None,
            java_min: None,
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(first.slug, "my-cool-pack");

    let duplicate = builds::create(
        &env.pool,
        owner,
        builds::CreateBuildRequest {
            name: "My Cool Pack".to_owned(),
            slug: None,
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: None,
            java_min: None,
            java_notes: String::new(),
        },
    )
    .await;
    assert!(duplicate.is_err());
}

#[tokio::test]
async fn artifact_upload_stores_sha_and_size() {
    let env = TestEnv::new().await;
    env.add_volume("mc-vol").await;
    let build_id = create_build(&env).await;
    let payload = b"fake mod jar content";
    let file_id = upload_artifact(&env, build_id, "mods/test.jar", payload).await;

    let row = files::load_file(&env.pool, build_id, file_id)
        .await
        .unwrap();
    assert_eq!(row.size_bytes, payload.len() as i64);
    assert_eq!(row.sha256.len(), 64);
    assert_eq!(row.path, "mods/test.jar");
    assert!(row.object_id.is_some());

    // H1: тип определён по содержимому, клиентский application/java-archive
    // для текстового payload не сохраняется.
    let stored_type: String =
        sqlx::query_scalar("select content_type from storage_objects where id = $1")
            .bind(row.object_id.unwrap())
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(stored_type, "text/plain");

    // Родительская папка создана автоматически.
    let folders: i64 = sqlx::query_scalar(
        "select count(*) from minecraft_build_files where build_id = $1 and kind = 'dir'",
    )
    .bind(build_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(folders >= 1);
}

#[tokio::test]
async fn duplicate_paths_are_rejected_case_insensitively() {
    let env = TestEnv::new().await;
    env.add_volume("mc-dup").await;
    let build_id = create_build(&env).await;
    upload_artifact(&env, build_id, "mods/Test.JAR", b"one").await;

    let owner = test_user(&env).await;
    let error = uploads::create_upload(
        &env.state,
        owner,
        build_id,
        uploads::CreateUploadRequest {
            path: "mods/test.jar".to_owned(),
            size: 3,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "application/java-archive".to_owned(),
        },
    )
    .await
    .expect_err("путь занят с учётом регистра");
    assert_eq!(error.code, "BUILD_PATH_CONFLICT");
}

#[tokio::test]
async fn validation_reports_errors_and_blocks_publication() {
    let env = TestEnv::new().await;
    env.add_volume("mc-validate").await;
    let build_id = create_build(&env).await;

    // Пустая сборка: ошибка валидации.
    let report = checks::validate(&env.pool, build_id).await.unwrap();
    assert!(!report.valid);
    assert!(
        report
            .errors
            .iter()
            .any(|issue| issue.code == "FILES_EMPTY")
    );

    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;
    let report = checks::validate(&env.pool, build_id).await.unwrap();
    assert!(report.valid, "ошибки: {:?}", report.errors);
    assert_eq!(report.file_count, 1);
}

#[tokio::test]
async fn release_snapshot_is_immutable() {
    let env = TestEnv::new().await;
    env.add_volume("mc-release").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"first").await;

    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: "initial".to_owned(),
            channel: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(release.file_count, 1);

    // Добавляем файл уже после создания релиза.
    upload_artifact(&env, build_id, "config/b.toml", b"x = 1").await;

    let stored = releases::load(&env.pool, build_id, release.id)
        .await
        .unwrap();
    assert_eq!(stored.file_count, 1, "снимок релиза не меняется");

    let release_files: i64 =
        sqlx::query_scalar("select count(*) from minecraft_release_files where release_id = $1")
            .bind(release.id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(release_files, 1);
}

#[tokio::test]
async fn manifest_is_stable_and_uses_relative_paths() {
    let env = TestEnv::new().await;
    env.add_volume("mc-manifest").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();

    let first = manifests::build_manifest(&env.pool, build_id, release.id)
        .await
        .unwrap();
    let second = manifests::build_manifest(&env.pool, build_id, release.id)
        .await
        .unwrap();
    assert_eq!(first, second, "повторная генерация идентична");
    manifests::validate_manifest(&first).unwrap();

    assert_eq!(first["schema_version"], "1.0");
    let files = first["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let path = files[0]["path"].as_str().unwrap();
    assert_eq!(path, "mods/a.jar");
    assert!(!path.contains(':'));
    let url = files[0]["download_url"].as_str().unwrap();
    assert!(url.starts_with("/api/v1/launcher/artifacts/"));
    assert_eq!(files[0]["sha256"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn publish_then_activate_previous_release() {
    let env = TestEnv::new().await;
    env.add_volume("mc-publish").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let first = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, first.id, true)
        .await
        .unwrap();

    upload_artifact(&env, build_id, "config/b.toml", b"x = 1").await;
    let second = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.1.0".to_owned()),
            changelog: "add config".to_owned(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, second.id, true)
        .await
        .unwrap();

    let build = builds::load(&env.pool, build_id).await.unwrap();
    assert_eq!(build.current_release_id, Some(second.id));
    assert_eq!(build.current_release_version.as_deref(), Some("1.1.0"));

    // Откат на первый релиз.
    releases::set_current(&env.pool, owner, build_id, first.id)
        .await
        .unwrap();
    let build = builds::load(&env.pool, build_id).await.unwrap();
    assert_eq!(build.current_release_id, Some(first.id));

    // Сравнение показывает добавленный файл.
    let diff = releases::compare(&env.pool, build_id, first.id, second.id)
        .await
        .unwrap();
    assert_eq!(diff["added"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unpublish_build_hides_releases_and_allows_republish() {
    let env = TestEnv::new().await;
    env.add_volume("mc-unpublish").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, release.id, true)
        .await
        .unwrap();

    let build = releases::unpublish_build(&env.pool, owner, "tester", build_id)
        .await
        .unwrap();
    assert_eq!(build.status, "draft");
    assert!(build.current_release_id.is_none());

    let status: String = sqlx::query_scalar("select status from minecraft_releases where id = $1")
        .bind(release.id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(status, "unpublished");

    // Снимок сохраняется: тот же релиз можно опубликовать снова.
    let republished = releases::publish(&env.pool, owner, "tester", build_id, release.id, true)
        .await
        .unwrap();
    assert_eq!(republished.status, "published");
    let build = builds::load(&env.pool, build_id).await.unwrap();
    assert_eq!(build.status, "published");
    assert_eq!(build.current_release_id, Some(release.id));
}

#[tokio::test]
async fn archiving_current_release_keeps_build_published_when_another_release_is_live() {
    let env = TestEnv::new().await;
    env.add_volume("mc-promote").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let first = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, first.id, true)
        .await
        .unwrap();

    let second = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.1.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, second.id, true)
        .await
        .unwrap();

    // Архив текущего релиза не должен оставлять сборку в черновиках, если
    // существует другой опубликованный релиз: он становится текущим.
    releases::set_status(
        &env.pool,
        owner,
        build_id,
        second.id,
        "archived",
        "release.archived",
    )
    .await
    .unwrap();
    let build = builds::load(&env.pool, build_id).await.unwrap();
    assert_eq!(build.status, "published");
    assert_eq!(build.current_release_id, Some(first.id));

    // Идемпотентная публикация уже опубликованного релиза восстанавливает
    // рассинхронизированный статус сборки (draft + published release).
    sqlx::query(
        "update minecraft_builds set status = 'draft', current_release_id = null where id = $1",
    )
    .bind(build_id)
    .execute(&env.pool)
    .await
    .unwrap();
    let repaired = releases::publish(&env.pool, owner, "tester", build_id, first.id, true)
        .await
        .unwrap();
    assert_eq!(repaired.status, "published");
    let build = builds::load(&env.pool, build_id).await.unwrap();
    assert_eq!(build.status, "published");
    assert_eq!(build.current_release_id, Some(first.id));
}

#[tokio::test]
async fn deleting_build_releases_objects() {
    let env = TestEnv::new().await;
    env.add_volume("mc-delete").await;
    let build_id = create_build(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let before: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(before, 1);

    builds::delete(&env.pool, build_id).await.unwrap();

    let after: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "объект удалён вместе со сборкой");

    let refs: i64 = sqlx::query_scalar("select count(*) from storage_object_refs")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(refs, 0);
}

#[tokio::test]
async fn published_release_protects_build_from_deletion() {
    let env = TestEnv::new().await;
    env.add_volume("mc-protect").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;
    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, release.id, true)
        .await
        .unwrap();

    let error = builds::delete(&env.pool, build_id)
        .await
        .expect_err("удаление опубликованной сборки запрещено");
    assert_eq!(error.code, "BUILD_HAS_PUBLISHED_RELEASES");
}

#[tokio::test]
async fn clone_copies_files_and_reuses_objects() {
    let env = TestEnv::new().await;
    env.add_volume("mc-clone").await;
    let build_id = create_build(&env).await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let clone = builds::clone_build(&env.pool, owner, build_id)
        .await
        .unwrap();
    assert_eq!(clone.file_count, 1);

    // Объект один, но ссылок две (оригинал и копия).
    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 1);
    let ref_count: i32 = sqlx::query_scalar("select ref_count from storage_objects limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        ref_count, 2,
        "копия увеличивает счётчик ссылок, а не дублирует файл"
    );
}

/// Семантическая подпись файлов: порядок и содержимое без серверных id.
fn file_signature(manifest: &Value) -> Vec<(String, i64, String)> {
    manifest["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| {
            (
                file["path"].as_str().expect("path").to_owned(),
                file["size"].as_i64().expect("size"),
                file["sha256"].as_str().expect("sha256").to_owned(),
            )
        })
        .collect()
}

async fn snapshot_release(env: &TestEnv, slug: &str, entries: &[(&str, &[u8])]) -> (Uuid, Uuid) {
    let build_id = create_build_with_slug(env, slug).await;
    for (path, payload) in entries {
        upload_artifact(env, build_id, path, payload).await;
    }
    let owner = test_user(env).await;
    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .expect("релиз");
    (build_id, release.id)
}

/// G3: порядок загрузки не влияет на манифест, сортировка байтовая и не
/// зависит от локали/регистра; не-ASCII пути уже в NFC и с прямыми слэшами.
#[tokio::test]
async fn manifest_order_is_byte_stable_for_case() {
    let env = TestEnv::new().await;
    env.add_volume("mc-determinism").await;

    let entries: &[(&str, &[u8])] = &[
        ("mods/Z.jar", b"zed"),
        ("config/q.json", b"config"),
        ("mods/B.jar", b"bee"),
        ("mods/a.jar", b"a"),
    ];
    let (first_build, first_release) = snapshot_release(&env, "order-a", entries).await;

    let mut reversed: Vec<(&str, &[u8])> = entries.to_vec();
    reversed.reverse();
    let (second_build, second_release) = snapshot_release(&env, "order-b", &reversed).await;

    let first = manifests::build_manifest(&env.pool, first_build, first_release)
        .await
        .unwrap();
    let second = manifests::build_manifest(&env.pool, second_build, second_release)
        .await
        .unwrap();

    // Различаются только идентификаторы сборки/релиза; файлы семантически равны.
    assert_eq!(file_signature(&first), file_signature(&second));

    let paths: Vec<String> = first["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect();
    let mut expected = paths.clone();
    expected.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(paths, expected, "порядок должен быть байтовым");

    for path in &paths {
        assert!(!path.contains('\\'), "в манифесте только прямые слэши");
        assert_eq!(
            validation::normalize_path(path).expect("путь нормализуется"),
            *path,
            "путь хранится в NFC"
        );
    }

    // Повторная генерация того же релиза байт-в-байт идентична.
    let again = manifests::build_manifest(&env.pool, first_build, first_release)
        .await
        .unwrap();
    assert_eq!(first, again);
}

async fn insert_job(env: &TestEnv, job_type: &str, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, created_by)
         values ($1, 'pending', '{}'::jsonb, true, $2)
         returning id",
    )
    .bind(job_type)
    .bind(owner)
    .fetch_one(&env.pool)
    .await
    .expect("задача")
}

async fn job_outcome(env: &TestEnv, job_id: Uuid) -> (String, Value) {
    sqlx::query_as(
        "select status, coalesce(result, '{}'::jsonb) from background_jobs where id = $1",
    )
    .bind(job_id)
    .fetch_one(&env.pool)
    .await
    .expect("итог задачи")
}

async fn stored_manifest(env: &TestEnv, release_id: Uuid) -> Value {
    sqlx::query_scalar("select manifest from minecraft_releases where id = $1")
        .bind(release_id)
        .fetch_one(&env.pool)
        .await
        .expect("сохранённый манифест")
}

/// G3/G1: опубликованный релиз неизменяем; единственная разрешённая правка —
/// аддитивное добавление иконки аудируемой задачей `system.manifests.regenerate`.
#[tokio::test]
async fn published_release_changes_only_by_audited_additive_regeneration() {
    let env = TestEnv::new().await;
    env.add_volume("mc-regen").await;
    let build_id = create_build_with_slug(&env, "regen-pack").await;
    let owner = test_user(&env).await;
    upload_artifact(&env, build_id, "mods/a.jar", b"payload").await;

    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: "release".to_owned(),
            channel: None,
        },
    )
    .await
    .unwrap();
    releases::publish(&env.pool, owner, "tester", build_id, release.id, true)
        .await
        .unwrap();

    let before = stored_manifest(&env, release.id).await;
    assert!(before.get("icon").is_none(), "иконки ещё не было");

    // Иконка появляется уже после публикации.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[7u8; 32]);
    builds::set_icon(&env.state, build_id, owner, png)
        .await
        .unwrap();

    // Изменение сборки после публикации не переписывает снимок релиза.
    upload_artifact(&env, build_id, "config/b.toml", b"x = 1").await;
    assert_eq!(
        before,
        stored_manifest(&env, release.id).await,
        "снимок опубликованного релиза неизменен"
    );

    // Аддитивная регенерация добавляет только icon.
    let job = insert_job(&env, "system.manifests.regenerate", owner).await;
    nexus::system::jobs::run_job(env.state.clone(), job).await;
    let (status, result) = job_outcome(&env, job).await;
    assert_eq!(status, "completed");
    assert_eq!(result["updated"].as_i64(), Some(1));
    assert_eq!(result["skipped"].as_i64(), Some(0));

    let regenerated = stored_manifest(&env, release.id).await;
    assert_eq!(regenerated["build"], before["build"]);
    assert_eq!(regenerated["files"], before["files"]);
    assert_eq!(
        regenerated["icon"]["url"],
        json!(manifests::icon_url("regen-pack"))
    );
    let sha: String = sqlx::query_scalar(
        "select o.sha256
         from minecraft_builds b join storage_objects o on o.id = b.icon_object_id
         where b.id = $1",
    )
    .bind(build_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(regenerated["icon"]["sha256"], json!(sha));

    // Повторный запуск идемпотентен и не пишет лишний аудит.
    let second = insert_job(&env, "system.manifests.regenerate", owner).await;
    nexus::system::jobs::run_job(env.state.clone(), second).await;
    let (_, result) = job_outcome(&env, second).await;
    assert_eq!(result["updated"].as_i64(), Some(0));
    assert_eq!(result["unchanged"].as_i64(), Some(1));

    let audit: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where event_type = 'minecraft.manifest.regenerated' and target_id = $1",
    )
    .bind(release.id.to_string())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);

    // Неаддитивное расхождение (изменилось имя сборки) не применяется.
    builds::update(
        &env.pool,
        build_id,
        builds::UpdateBuildRequest {
            name: Some("Renamed Pack".to_owned()),
            slug: None,
            description: None,
            version: None,
            category: None,
            visibility: None,
            minecraft_version: None,
            loader_type: None,
            loader_version: None,
            java_major: None,
            java_min: None,
            java_notes: None,
        },
    )
    .await
    .unwrap();

    let third = insert_job(&env, "system.manifests.regenerate", owner).await;
    nexus::system::jobs::run_job(env.state.clone(), third).await;
    let (status, result) = job_outcome(&env, third).await;
    assert_eq!(status, "completed");
    assert_eq!(result["skipped"].as_i64(), Some(1));
    assert_eq!(result["updated"].as_i64(), Some(0));
    assert_eq!(
        regenerated,
        stored_manifest(&env, release.id).await,
        "неаддитивная правка отклонена, документ не перезаписан"
    );
}

// --- Каноническое дерево и мутации путей (новый файловый интерфейс) ---

#[tokio::test]
async fn nested_config_folders_preserve_structure_and_folder_filter() {
    let env = TestEnv::new().await;
    env.add_volume("mc-nested").await;
    let build_id = create_build(&env).await;

    let folder = files::create_folder(&env.pool, build_id, "config/create/deep")
        .await
        .expect("вложенная папка config");
    assert_eq!(folder.path, "config/create/deep");
    assert_eq!(folder.category, "config");

    upload_artifact(&env, build_id, "config/create/deep/a.toml", b"x = 1").await;
    upload_artifact(&env, build_id, "config/create/b.json", b"{}").await;

    let query = files::ListFilesQuery {
        q: None,
        category: None,
        side: None,
        folder: Some("config/create/deep".to_owned()),
    };
    let items = files::list(&env.pool, build_id, &query).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, "config/create/deep/a.toml");
    assert_eq!(items[0].category, "config");

    let root = files::ListFilesQuery {
        q: None,
        category: None,
        side: None,
        folder: Some(String::new()),
    };
    let root_items = files::list(&env.pool, build_id, &root).await.unwrap();
    assert!(
        root_items.iter().all(|row| row.kind == "dir"),
        "в корне не должно быть файлов, кроме options.txt"
    );
}

#[tokio::test]
async fn config_collisions_are_checked_without_case_sensitivity() {
    let env = TestEnv::new().await;
    env.add_volume("mc-collisions").await;
    let build_id = create_build(&env).await;

    files::create_folder(&env.pool, build_id, "config/Create")
        .await
        .expect("папка");
    let duplicate = files::create_folder(&env.pool, build_id, "config/create").await;
    assert!(duplicate.is_err(), "коллизия регистра должна блокироваться");

    upload_artifact(&env, build_id, "config/a.toml", b"a = 1").await;
    let owner = test_user(&env).await;
    let duplicate_file = uploads::create_upload(
        &env.state,
        owner,
        build_id,
        uploads::CreateUploadRequest {
            path: "config/A.toml".to_owned(),
            size: 5,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "text/plain".to_owned(),
        },
    )
    .await;
    assert!(
        duplicate_file.is_err(),
        "коллизия регистра путей должна блокироваться"
    );

    let second = upload_artifact(&env, build_id, "config/b.toml", b"b = 1").await;
    let row = files::load_file(&env.pool, build_id, second).await.unwrap();
    let renamed = files::update_file(
        &env.pool,
        build_id,
        row.id,
        files::UpdateFileRequest {
            path: Some("config/A.toml".to_owned()),
            category: None,
            side: None,
            required: None,
            enabled: None,
            description: None,
        },
    )
    .await;
    assert!(renamed.is_err(), "переименование в занятый путь запрещено");
}

#[tokio::test]
async fn moving_config_with_incompatible_extensions_is_rejected() {
    let env = TestEnv::new().await;
    env.add_volume("mc-move").await;
    let build_id = create_build(&env).await;

    let folder = files::create_folder(&env.pool, build_id, "config/create")
        .await
        .expect("папка");
    upload_artifact(&env, build_id, "config/create/a.toml", b"x = 1").await;

    let result = files::update_file(
        &env.pool,
        build_id,
        folder.id,
        files::UpdateFileRequest {
            path: Some("mods/create".to_owned()),
            category: None,
            side: None,
            required: None,
            enabled: None,
            description: None,
        },
    )
    .await;
    assert!(result.is_err(), "config с .toml нельзя превратить в mods");
}

#[tokio::test]
async fn options_txt_has_a_single_slot_and_is_replaced_atomically() {
    let env = TestEnv::new().await;
    env.add_volume("mc-options").await;
    let build_id = create_build(&env).await;

    let first = upload_artifact(&env, build_id, "options.txt", b"fov:0.5").await;
    let duplicate = uploads::create_upload(
        &env.state,
        test_user(&env).await,
        build_id,
        uploads::CreateUploadRequest {
            path: "Options.TXT".to_owned(),
            size: 3,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "text/plain".to_owned(),
        },
    )
    .await;
    assert!(
        duplicate.is_err(),
        "options.txt должен быть уникален без учёта регистра"
    );

    let owner = test_user(&env).await;
    let session = uploads::create_replace_upload(
        &env.state,
        owner,
        build_id,
        uploads::CreateUploadRequest {
            path: "options.txt".to_owned(),
            size: 5,
            category: None,
            side: None,
            required: None,
            description: None,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .expect("сессия замены options.txt");
    uploads::append_stream(
        &env.state,
        session.id,
        owner,
        0,
        Body::from(b"fov:1".to_vec()),
    )
    .await
    .expect("чанк replacement");
    let replaced = uploads::complete_upload(&env.state, session.id, owner)
        .await
        .expect("атомарная замена");
    assert_eq!(replaced.id, first, "замена не создаёт дубль");
    assert_eq!(replaced.size_bytes, 5);
    assert_eq!(replaced.category, "config");
}

#[tokio::test]
async fn canonical_nodes_reject_non_ascii_reserved_and_mod_subfolders() {
    let env = TestEnv::new().await;
    env.add_volume("mc-policy").await;
    let build_id = create_build(&env).await;

    for path in [
        "mods/мод.jar",
        "mods/sub/a.jar",
        "mods/a.txt",
        "shaderpacks/deep/a.zip",
        "versions/1.20.1/a.jar",
        "libraries/a.jar",
        "mods/CON.jar",
    ] {
        let error = uploads::create_upload(
            &env.state,
            test_user(&env).await,
            build_id,
            uploads::CreateUploadRequest {
                path: path.to_owned(),
                size: 4,
                category: None,
                side: None,
                required: None,
                description: None,
                content_type: "application/octet-stream".to_owned(),
            },
        )
        .await;
        assert!(error.is_err(), "путь должен быть отклонён: {path}");
    }
}

#[test]
fn canonical_path_rules_are_explicit() {
    assert_eq!(
        validation::check_build_path("CONFIG\\Create\\a.toml", "file").unwrap(),
        "config/Create/a.toml"
    );
    assert!(validation::check_build_path("mods/мод.jar", "file").is_err());
    assert!(validation::check_build_path("mods/sub/a.jar", "file").is_err());
    assert!(validation::check_build_path("shaderpacks/a.jar", "file").is_err());
    assert!(validation::check_build_path("config/a.exe", "file").is_err());
    assert!(validation::check_build_path("options.txt", "dir").is_err());
    assert!(validation::check_build_path("other-root.txt", "file").is_err());
    // Legacy-категории не ломаются: их расширения не входят в allowlist.
    assert!(validation::check_build_path("overrides/big.bin", "file").is_ok());
    assert_eq!(
        validation::resolve_category("config/a.toml", Some("mods")).unwrap(),
        "config"
    );
    assert!(validation::is_fixed_build_folder("MODS"));
    assert!(validation::is_fixed_build_root("options.txt"));
}
