//! Контрактные тесты манифеста релиза.
//!
//! Проверяют, что:
//! * манифест, реально собранный `build_manifest` на тестовой БД, соответствует
//!   JSON Schema 2020-12 из `docs/schemas/manifest-1.0.json`;
//! * все полные примеры из `docs/MANIFEST.md` соответствуют той же схеме;
//! * схема действительно отвергает некорректные документы, а не принимает всё.

mod common;

use common::TestEnv;
use nexus::minecraft::{builds, manifests, releases, uploads, validation};
use serde_json::{Value, json};
use uuid::Uuid;

fn schema() -> Value {
    serde_json::from_str(include_str!("../../docs/schemas/manifest-1.0.json"))
        .expect("схема манифеста должна быть корректным JSON")
}

fn validator(schema: &Value) -> jsonschema::Validator {
    jsonschema::validator_for(schema).expect("схема должна собираться валидатором JSON Schema")
}

fn validate(schema: &Value, instance: &Value, label: &str) {
    let validator = validator(schema);
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{label} не соответствует JSON Schema:\n{}",
        errors.join("\n")
    );
}

/// Полные манифесты в документе размечены как ```json; фрагменты и
/// будущий delta-формат — как ```jsonc и здесь намеренно не проверяются.
fn json_manifest_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        if current.is_none() {
            if line.trim() == "```json" {
                current = Some(String::new());
            }
            continue;
        }
        if line.trim() == "```" {
            blocks.push(current.take().expect("блок открыт"));
        } else {
            let buffer = current.as_mut().expect("блок открыт");
            buffer.push_str(line);
            buffer.push('\n');
        }
    }
    assert!(
        current.is_none(),
        "в MANIFEST.md остался незакрытый блок ```json"
    );
    blocks
}

async fn create_build(env: &TestEnv, owner: Uuid) -> Uuid {
    builds::create(
        &env.pool,
        owner,
        builds::CreateBuildRequest {
            name: "Schema Pack".to_owned(),
            slug: Some("schema-pack".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "fabric".to_owned(),
            loader_version: "0.15.0".to_owned(),
            java_major: Some(21),
            java_min: Some(17),
            java_notes: String::new(),
        },
    )
    .await
    .expect("создание сборки")
    .id
}

async fn upload_artifact(
    env: &TestEnv,
    build_id: Uuid,
    owner: Uuid,
    path: &str,
    category: &str,
    payload: &[u8],
) {
    let session = uploads::create_upload(
        &env.state,
        owner,
        build_id,
        uploads::CreateUploadRequest {
            path: path.to_owned(),
            size: payload.len() as i64,
            category: Some(category.to_owned()),
            side: Some("both".to_owned()),
            required: Some(true),
            description: None,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("сессия артефакта");
    uploads::append_stream(
        &env.state,
        session.id,
        owner,
        0,
        axum::body::Body::from(payload.to_vec()),
    )
    .await
    .expect("запись чанка");
    uploads::complete_upload(&env.state, session.id, owner)
        .await
        .expect("фиксация артефакта");
}

#[tokio::test]
async fn generated_manifest_matches_json_schema() {
    let env = TestEnv::new().await;
    env.add_volume("manifest-schema").await;
    let owner = env.create_user("manifest_tester", "SecretPass1!").await;
    let build_id = create_build(&env, owner).await;

    upload_artifact(
        &env,
        build_id,
        owner,
        "config/options.json",
        "config",
        b"{\"gamma\":1}",
    )
    .await;
    upload_artifact(&env, build_id, owner, "mods/core.jar", "mods", b"jar-bytes").await;

    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: "Первый релиз.".to_owned(),
            channel: None,
        },
    )
    .await
    .expect("создание релиза");

    let manifest = manifests::build_manifest(&env.pool, build_id, release.id)
        .await
        .expect("генерация манифеста");

    manifests::validate_manifest(&manifest).expect("runtime-валидация");
    validate(&schema(), &manifest, "сгенерированный манифест");

    assert_eq!(manifest["schema_version"], "1.0");
    let files = manifest["files"].as_array().expect("файлы");
    assert_eq!(files.len(), 2);
    // Детерминированный порядок: lower(path).
    assert_eq!(files[0]["path"], "config/options.json");
    assert_eq!(files[1]["path"], "mods/core.jar");
    for file in files {
        let path = file["path"].as_str().expect("path");
        assert_eq!(
            validation::normalize_path(path).expect("путь нормализуется"),
            path
        );
    }
}

#[test]
fn documented_examples_match_json_schema() {
    let document = include_str!("../../docs/MANIFEST.md");
    let schema = schema();
    let blocks = json_manifest_blocks(document);
    assert!(
        blocks.len() >= 3,
        "в MANIFEST.md должно быть не менее трёх полных примеров, найдено {}",
        blocks.len()
    );

    for (index, block) in blocks.iter().enumerate() {
        let manifest: Value = serde_json::from_str(block)
            .unwrap_or_else(|error| panic!("пример {} — некорректный JSON: {error}", index + 1));
        assert_eq!(
            manifest["schema_version"],
            "1.0",
            "пример {} должен описывать schema 1.0",
            index + 1
        );
        validate(
            &schema,
            &manifest,
            &format!("пример {} из MANIFEST.md", index + 1),
        );
    }
}

#[test]
fn schema_rejects_broken_manifests() {
    let document = include_str!("../../docs/MANIFEST.md");
    let base: Value = serde_json::from_str(
        json_manifest_blocks(document)
            .first()
            .expect("минимальный пример"),
    )
    .expect("пример — JSON");
    let schema = schema();
    let validator = validator(&schema);
    assert!(
        validator.is_valid(&base),
        "исходный пример должен быть валиден"
    );

    let mut without_hash = base.clone();
    without_hash["files"][0]
        .as_object_mut()
        .expect("объект файла")
        .remove("sha256");
    assert!(
        !validator.is_valid(&without_hash),
        "манифест без sha256 должен отвергаться"
    );

    let mut absolute_url = base.clone();
    absolute_url["files"][0]["download_url"] = json!("C:\\storage\\objects\\a.jar");
    assert!(
        !validator.is_valid(&absolute_url),
        "абсолютный серверный путь в download_url должен отвергаться"
    );

    let mut traversal = base.clone();
    traversal["files"][0]["path"] = json!("mods/../secret.jar");
    assert!(
        !validator.is_valid(&traversal),
        "path traversal в path должен отвергаться"
    );

    let mut unknown_root = base.clone();
    unknown_root["generated_at"] = json!("2026-11-19T12:00:00Z");
    assert!(
        !validator.is_valid(&unknown_root),
        "неизвестное поле верхнего уровня должно отвергаться схемой 1.0"
    );
}

/* ------------------------------------------------------------------------- */
/* I4: пустой релиз, отключённые файлы, метаданные и относительные ссылки.    */
/* ------------------------------------------------------------------------- */

async fn snapshot(env: &TestEnv, owner: Uuid, build_id: Uuid) -> (Uuid, Value) {
    let release = releases::create(
        &env.pool,
        owner,
        build_id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: "snapshot".to_owned(),
            channel: None,
        },
    )
    .await
    .expect("релиз");
    let manifest = manifests::build_manifest(&env.pool, build_id, release.id)
        .await
        .expect("манифест");
    (release.id, manifest)
}

#[tokio::test]
async fn empty_release_manifest_is_valid_and_has_no_files() {
    let env = TestEnv::new().await;
    let owner = env.create_user("manifest_empty", "SecretPass1!").await;
    let build_id = create_build(&env, owner).await;
    let (_release, manifest) = snapshot(&env, owner, build_id).await;

    manifests::validate_manifest(&manifest).expect("runtime-валидация пустого релиза");
    validate(&schema(), &manifest, "пустой манифест");
    assert_eq!(manifest["files"], json!([]));
    assert_eq!(manifest["schema_version"], "1.0");
    assert!(manifest.get("icon").is_none());
}

#[tokio::test]
async fn disabled_release_file_is_omitted_from_manifest() {
    let env = TestEnv::new().await;
    env.add_volume("manifest-disabled").await;
    let owner = env.create_user("manifest_disabled", "SecretPass1!").await;
    let build_id = create_build(&env, owner).await;
    upload_artifact(&env, build_id, owner, "mods/keep.jar", "mods", b"keep").await;
    upload_artifact(&env, build_id, owner, "mods/hidden.jar", "mods", b"hide").await;
    let (release_id, _) = snapshot(&env, owner, build_id).await;

    sqlx::query(
        "update minecraft_release_files set enabled = false
         where release_id = $1 and path = 'mods/hidden.jar'",
    )
    .bind(release_id)
    .execute(&env.pool)
    .await
    .unwrap();

    let manifest = manifests::build_manifest(&env.pool, build_id, release_id)
        .await
        .unwrap();
    validate(&schema(), &manifest, "манифест без отключённого файла");
    let paths: Vec<&str> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["mods/keep.jar"]);
}

#[tokio::test]
async fn manifest_preserves_category_side_and_required_flags() {
    let env = TestEnv::new().await;
    env.add_volume("manifest-metadata").await;
    let owner = env.create_user("manifest_metadata", "SecretPass1!").await;
    let build_id = create_build(&env, owner).await;
    upload_artifact(&env, build_id, owner, "mods/core.jar", "mods", b"jar").await;
    upload_artifact(
        &env,
        build_id,
        owner,
        "config/options.json",
        "config",
        b"{\"a\":1}",
    )
    .await;
    let (release_id, _) = snapshot(&env, owner, build_id).await;

    sqlx::query(
        "update minecraft_release_files
         set category = 'config', side = 'server', required = false
         where release_id = $1 and path = 'config/options.json'",
    )
    .bind(release_id)
    .execute(&env.pool)
    .await
    .unwrap();

    let manifest = manifests::build_manifest(&env.pool, build_id, release_id)
        .await
        .unwrap();
    validate(&schema(), &manifest, "манифест с метаданными");
    let config = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "config/options.json")
        .expect("файл в манифесте");
    assert_eq!(config["category"], "config");
    assert_eq!(config["side"], "server");
    assert_eq!(config["required"], false);
    assert_eq!(config["size"], 7);
    assert_eq!(config["sha256"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn manifest_download_urls_are_relative_unique_and_match_ids() {
    let env = TestEnv::new().await;
    env.add_volume("manifest-urls").await;
    let owner = env.create_user("manifest_urls", "SecretPass1!").await;
    let build_id = create_build(&env, owner).await;
    for (path, payload) in [
        ("mods/a.jar", b"aaa".as_slice()),
        ("mods/b.jar", b"bbb".as_slice()),
        ("config/c.json", b"{\"c\":1}".as_slice()),
    ] {
        let category = if path.starts_with("mods/") {
            "mods"
        } else {
            "config"
        };
        upload_artifact(&env, build_id, owner, path, category, payload).await;
    }
    let (_release, manifest) = snapshot(&env, owner, build_id).await;
    validate(&schema(), &manifest, "манифест со ссылками");

    let mut urls = Vec::new();
    for file in manifest["files"].as_array().unwrap() {
        let url = file["download_url"].as_str().unwrap();
        let id = file["id"].as_str().unwrap();
        assert!(!url.contains(":\\"), "нет абсолютного Windows-пути");
        assert!(!url.starts_with("//"), "нет UNC-пути");
        assert_eq!(url, format!("/api/v1/launcher/artifacts/{id}"));
        urls.push(url.to_owned());
    }
    urls.sort();
    urls.dedup();
    assert_eq!(urls.len(), 3, "у каждого файла своя ссылка");
}
