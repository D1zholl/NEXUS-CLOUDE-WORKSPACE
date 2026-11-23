//! Манифест для MyLauncher (формат `manifestVersion: 1`).
//!
//! Контракт зафиксирован по коду клиента:
//! `api/packs/{pack_id}/versions/{version}/manifest?channel=...`.
//! Внутренний манифест NEXUS (`schema_version: "1.0"`) не меняется и
//! продолжает использоваться браузерным API; этот модуль формирует отдельный
//! документ для лаунчера и неизменяемый снимок при публикации.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};

/// Версия формата манифеста MyLauncher.
pub const MANIFEST_VERSION: u32 = 1;
/// Версия строки схемы, которая хранится рядом со снимком.
pub const SCHEMA_VERSION: i32 = 1;
/// Ограничение клиента на число файлов.
pub const MAX_FILES: usize = 100_000;
/// Ограничение клиента на один файл (8 ГиБ).
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Ограничение клиента на суммарный размер (20 ГБ).
pub const MAX_TOTAL_BYTES: u64 = 20_000_000_000;

/// Расширения, которые клиент разрешает в путях манифеста.
const ALLOWED_EXTENSIONS: &[&str] = &[
    "jar",
    "zip",
    "json",
    "json5",
    "toml",
    "bak",
    "backup",
    "cfg",
    "conf",
    "properties",
    "txt",
    "md",
    "png",
    "jpg",
    "jpeg",
    "webp",
    "mcmeta",
    "dat",
    "nbt",
    "snbt",
    "mcfunction",
    "vsh",
    "fsh",
    "glsl",
    "lang",
    "yml",
    "yaml",
    "xml",
    "ini",
];

/// Загрузчики, которые понимает `validate_loader_selection` клиента.
pub const SUPPORTED_LOADERS: &[&str] = &["vanilla", "fabric", "forge", "neoforge"];

/// Обязательная Java по версии Minecraft.
///
/// Повторяет `infer_java_version` клиента. Метаданные Mojang имеют приоритет,
/// но это правило используется, когда метаданные недоступны, и для проверки
/// перед публикацией.
pub fn required_java_major(version: &str) -> u8 {
    let mut components = version.split('.');
    let Ok(major) = components
        .next()
        .unwrap_or_default()
        .parse::<u32>()
        .map_err(|_| ())
    else {
        return 21;
    };
    let minor = components
        .next()
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let patch = components
        .next()
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    if major == 1 {
        match minor {
            0..=16 => 8,
            17 => 16,
            18..=19 => 17,
            20 if patch < 5 => 17,
            _ => 21,
        }
    } else if major >= 25 {
        25
    } else {
        21
    }
}

/// `valid_component` клиента: версия сборки/загрузчика/Minecraft.
pub fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
}

/// Идентификатор сборки в URL клиента.
pub fn valid_pack_id(value: &str) -> bool {
    if value.len() < 3
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return false;
    }
    value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

/// Проверяет путь ровно по политике `safe_repository_path` клиента.
pub fn check_launcher_path(value: &str) -> Result<(), &'static str> {
    crate::minecraft::validation::check_manifest_path(value)?;
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || " _./+@()-§".contains(character))
    {
        return Err("символ вне политики лаунчера (разрешён только ASCII)");
    }
    if value.len() > 512 {
        return Err("путь длиннее 512 символов");
    }
    let first = value.split('/').next().unwrap_or_default();
    if first.eq_ignore_ascii_case("versions")
        || first.eq_ignore_ascii_case("assets")
        || first.eq_ignore_ascii_case("libraries")
        || [
            ".launcher_meta.json",
            ".launcher_meta.json.tmp",
            ".launcher_meta.json.backup",
        ]
        .iter()
        .any(|reserved| first.eq_ignore_ascii_case(reserved))
    {
        return Err("зарезервированный служебный путь лаунчера");
    }

    let filename = value.rsplit('/').next().unwrap_or(value);
    if filename == ".defaults" {
        return Ok(());
    }
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err("расширение файла не разрешено политикой лаунчера");
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn file_url(hash: &str) -> String {
    format!("/api/files/sha256:{hash}")
}

struct ManifestFile {
    path: String,
    sha256: String,
    size: u64,
    required: bool,
}

impl ManifestFile {
    fn json(&self) -> Value {
        let hash = format!("sha256:{}", self.sha256);
        json!({
            "path": self.path,
            "hash": hash,
            "size": self.size,
            "url": file_url(&self.sha256),
            "required": self.required,
        })
    }
}

/// Собирает манифест из неизменяемого снимка релиза.
///
/// Порядок файлов — байтовая сортировка по пути: результат не зависит от
/// локали PostgreSQL и повторная генерация даёт тот же документ.
pub async fn build(db: &PgPool, build_id: Uuid, release_id: Uuid) -> ApiResult<Value> {
    let build = crate::minecraft::builds::load(db, build_id).await?;
    let release = crate::minecraft::releases::load(db, build_id, release_id).await?;

    if !SUPPORTED_LOADERS.contains(&build.loader_type.as_str()) {
        return Err(ApiError::validation(format!(
            "Загрузчик {} не поддерживается MyLauncher.",
            build.loader_type
        )));
    }
    if !valid_pack_id(&build.slug) {
        return Err(ApiError::validation(
            "Slug сборки не соответствует формату идентификатора MyLauncher (3–64, a-z0-9-).",
        ));
    }
    if !valid_component(&release.version) {
        return Err(ApiError::validation(
            "Версия релиза не соответствует формату MyLauncher.",
        ));
    }
    if !valid_component(&build.minecraft_version) {
        return Err(ApiError::validation(
            "Версия Minecraft не соответствует формату MyLauncher.",
        ));
    }
    if build.loader_type != "vanilla"
        && (build.loader_version.is_empty()
            || build.loader_version.len() > 128
            || build.loader_version.contains(['/', '\\']))
    {
        return Err(ApiError::validation(
            "Некорректная версия загрузчика для MyLauncher.",
        ));
    }
    let required_java = required_java_major(&build.minecraft_version);
    if build.java_major as u8 != required_java {
        return Err(ApiError::validation(format!(
            "Для Minecraft {} требуется Java {}, а в сборке указана Java {}.",
            build.minecraft_version, required_java, build.java_major
        )));
    }

    let author: Option<String> = sqlx::query_scalar(
        "select coalesce(nullif(u.display_name, ''), u.username)
         from minecraft_builds b
         left join users u on u.id = coalesce(b.owner_id, b.created_by)
         where b.id = $1",
    )
    .bind(build_id)
    .fetch_optional(db)
    .await?
    .flatten();
    let authors = author
        .filter(|value| !value.trim().is_empty())
        .map(|value| vec![value])
        .unwrap_or_default();

    let rows: Vec<(String, String, i64, bool, String)> = sqlx::query_as(
        "select path, sha256, size_bytes, required, side
         from minecraft_release_files
         where release_id = $1 and enabled = true
         order by path",
    )
    .bind(release_id)
    .fetch_all(db)
    .await?;
    if rows.len() > MAX_FILES {
        return Err(ApiError::validation(format!(
            "Сборка содержит больше {MAX_FILES} файлов."
        )));
    }

    let mut files: Vec<ManifestFile> = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut total: u64 = 0;
    for (path, sha256, size, required, side) in rows {
        // Клиент не читает side и установил бы server-файлы в клиентский
        // инстанс. В репозиторий попадают только client/both.
        if side != "client" && side != "both" {
            continue;
        }
        check_launcher_path(&path)
            .map_err(|reason| ApiError::validation(format!("Файл «{path}»: {reason}.")))?;
        let key = path.nfc().collect::<String>().to_lowercase();
        if let Some(previous) = seen.insert(key, path.clone()) {
            return Err(ApiError::validation(format!(
                "Пути «{previous}» и «{path}» совпадают без учёта регистра."
            )));
        }
        if size < 0 {
            return Err(ApiError::validation(format!(
                "Файл «{path}» имеет отрицательный размер."
            )));
        }
        let size = size as u64;
        if size > MAX_FILE_BYTES {
            return Err(ApiError::validation(format!("Файл «{path}» больше 8 ГиБ.")));
        }
        total = total
            .checked_add(size)
            .filter(|value| *value <= MAX_TOTAL_BYTES)
            .ok_or_else(|| ApiError::validation("Сборка превышает 20 ГБ."))?;
        let sha256 = sha256.to_ascii_lowercase();
        if !is_sha256(&sha256) {
            return Err(ApiError::validation(format!(
                "Файл «{path}» не имеет корректного SHA-256."
            )));
        }
        files.push(ManifestFile {
            path,
            sha256,
            size,
            required,
        });
    }
    files.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    let manifest = json!({
        "manifestVersion": MANIFEST_VERSION,
        "packId": build.slug,
        "version": release.version,
        "channel": release.channel,
        "name": build.name,
        "description": build.description,
        "authors": authors,
        "categories": [build.category],
        "minecraft": {
            "version": build.minecraft_version,
            "loader": build.loader_type,
            "loaderVersion": if build.loader_type == "vanilla" {
                Value::Null
            } else {
                Value::String(build.loader_version.clone())
            },
        },
        "java": { "major": build.java_major },
        "files": files.iter().map(ManifestFile::json).collect::<Vec<_>>(),
        "settings": {},
    });
    validate(&manifest)?;
    Ok(manifest)
}

/// Проверяет документ ровно по полям, которые читает клиент, и по лимитам.
pub fn validate(manifest: &Value) -> ApiResult<()> {
    let object = manifest
        .as_object()
        .ok_or_else(|| ApiError::internal("Манифест лаунчера должен быть объектом."))?;
    if object.get("manifestVersion").and_then(Value::as_u64) != Some(MANIFEST_VERSION as u64) {
        return Err(ApiError::internal(
            "Некорректная версия манифеста лаунчера.",
        ));
    }
    let pack_id = object
        .get("packId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_pack_id(pack_id) {
        return Err(ApiError::internal(
            "Некорректный packId манифеста лаунчера.",
        ));
    }
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_component(version) {
        return Err(ApiError::internal(
            "Некорректная версия манифеста лаунчера.",
        ));
    }
    let minecraft = object
        .get("minecraft")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::internal("Манифест без блока minecraft."))?;
    let mc_version = minecraft
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_component(mc_version) {
        return Err(ApiError::internal(
            "Некорректная версия Minecraft в манифесте.",
        ));
    }
    let loader = minecraft
        .get("loader")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !SUPPORTED_LOADERS.contains(&loader) {
        return Err(ApiError::internal(
            "Загрузчик манифеста не поддерживается MyLauncher.",
        ));
    }
    if loader != "vanilla" {
        let loader_version = minecraft
            .get("loaderVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if loader_version.is_empty()
            || loader_version.len() > 128
            || loader_version.contains(['/', '\\'])
        {
            return Err(ApiError::internal(
                "Некорректная версия загрузчика в манифесте.",
            ));
        }
    }
    let java = object
        .get("java")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::internal("Манифест без блока java."))?;
    let java_major = java
        .get("major")
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::internal("Манифест без версии Java."))?;
    if !(8..=30).contains(&java_major) {
        return Err(ApiError::internal("Некорректная версия Java в манифесте."));
    }
    if java_major as u8 != required_java_major(mc_version) {
        return Err(ApiError::internal(
            "Версия Java в манифесте не совпадает с требованием Minecraft.",
        ));
    }

    let files = object
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::internal("Манифест без списка файлов."))?;
    if files.len() > MAX_FILES {
        return Err(ApiError::internal(
            "Манифест содержит слишком много файлов.",
        ));
    }
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut total: u64 = 0;
    for file in files {
        let path = file
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("Файл без пути."))?;
        check_launcher_path(path).map_err(|reason| {
            ApiError::internal(format!("Путь «{path}» несовместим с MyLauncher: {reason}."))
        })?;
        let key = path.nfc().collect::<String>().to_lowercase();
        if seen.insert(key, ()).is_some() {
            return Err(ApiError::internal("Повторяющийся путь в манифесте."));
        }
        let hash = file
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("Файл без SHA-256."))?;
        let Some(digest) = hash.strip_prefix("sha256:") else {
            return Err(ApiError::internal(
                "Хэш файла должен иметь префикс sha256:.",
            ));
        };
        if !is_sha256(digest) {
            return Err(ApiError::internal(
                "SHA-256 файла должен быть в нижнем регистре hex.",
            ));
        }
        let size = file
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| ApiError::internal("Файл без размера."))?;
        if size > MAX_FILE_BYTES {
            return Err(ApiError::internal("Файл больше 8 ГиБ."));
        }
        total = total
            .checked_add(size)
            .filter(|value| *value <= MAX_TOTAL_BYTES)
            .ok_or_else(|| ApiError::internal("Манифест превышает 20 ГБ."))?;
        let url = file.get("url").and_then(Value::as_str).unwrap_or_default();
        if url != file_url(digest) {
            return Err(ApiError::internal(
                "URL файла должен быть /api/files/sha256:{hex}.",
            ));
        }
        if file.get("required").and_then(Value::as_bool).is_none() {
            return Err(ApiError::internal("Файл без признака required."));
        }
    }
    Ok(())
}

/// SHA-256 канонического JSON снимка (для аудита и проверки неизменяемости).
pub fn snapshot_hash(manifest: &Value) -> String {
    let bytes = serde_json::to_vec(manifest).unwrap_or_default();
    hex::encode(Sha256::digest(&bytes))
}

/// Снимок манифеста из БД либо детерминированная генерация для релизов,
/// опубликованных до появления колонки. Готовый снимок не перезаписывается.
pub async fn snapshot(db: &PgPool, build_id: Uuid, release_id: Uuid) -> ApiResult<Value> {
    let stored: Option<Value> = sqlx::query_scalar(
        "select launcher_manifest from minecraft_releases where id = $1 and build_id = $2",
    )
    .bind(release_id)
    .bind(build_id)
    .fetch_optional(db)
    .await?
    .flatten();
    if let Some(value) = stored.filter(|value| {
        value.get("manifestVersion").and_then(Value::as_u64) == Some(MANIFEST_VERSION as u64)
    }) {
        validate(&value)?;
        return Ok(value);
    }
    let generated = build(db, build_id, release_id).await?;
    // Первичная материализация допустима; существующий снимок не трогаем.
    sqlx::query(
        "update minecraft_releases
         set launcher_manifest = $2, launcher_manifest_hash = $3,
             launcher_manifest_schema = $4, launcher_manifest_at = now()
         where id = $1 and launcher_manifest is null",
    )
    .bind(release_id)
    .bind(&generated)
    .bind(snapshot_hash(&generated))
    .bind(SCHEMA_VERSION)
    .execute(db)
    .await
    .ok();
    Ok(generated)
}

/// Фиксирует неизменяемый снимок при публикации.
pub async fn freeze(db: &PgPool, build_id: Uuid, release_id: Uuid) -> ApiResult<(Value, String)> {
    let manifest = build(db, build_id, release_id).await?;
    validate(&manifest)?;
    let hash = snapshot_hash(&manifest);
    sqlx::query(
        "update minecraft_releases
         set launcher_manifest = $2, launcher_manifest_hash = $3,
             launcher_manifest_schema = $4, launcher_manifest_at = now()
         where id = $1",
    )
    .bind(release_id)
    .bind(&manifest)
    .bind(&hash)
    .bind(SCHEMA_VERSION)
    .execute(db)
    .await?;
    Ok((manifest, hash))
}

/// Публичный URL иконки сборки для каталога.
pub fn icon_url(public_url: &str, pack_id: &str) -> String {
    format!(
        "{}/api/packs/{pack_id}/icon",
        public_url.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest() -> Value {
        json!({
            "manifestVersion": 1,
            "packId": "demo-pack",
            "version": "1.0.0",
            "channel": "stable",
            "name": "Demo",
            "description": "",
            "authors": ["tester"],
            "categories": ["other"],
            "minecraft": { "version": "1.20.1", "loader": "forge", "loaderVersion": "47.4.10" },
            "java": { "major": 17 },
            "files": [{
                "path": "mods/a.jar",
                "hash": format!("sha256:{}", "a".repeat(64)),
                "size": 10,
                "url": format!("/api/files/sha256:{}", "a".repeat(64)),
                "required": true
            }],
            "settings": {}
        })
    }

    #[test]
    fn manifest_matches_client_parser() {
        validate(&valid_manifest()).expect("валидный манифест");
    }

    #[test]
    fn manifest_matches_client_json_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../docs/schemas/launcher-manifest-1.json"
        ))
        .expect("JSON Schema лаунчера");
        let validator = jsonschema::validator_for(&schema).expect("валидатор схемы");
        let errors: Vec<String> = validator
            .iter_errors(&valid_manifest())
            .map(|error| format!("{error} at {}", error.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn rejects_incompatible_paths_and_extensions() {
        for path in [
            "mods/файл.jar",
            "mods/loader.exe",
            "mods/a.jar:stream",
            "versions/1.20.1/1.20.1.json",
            "libraries/a.jar",
            "../a.jar",
            "mods/trailing.",
        ] {
            let mut manifest = valid_manifest();
            manifest["files"][0]["path"] = json!(path);
            assert!(validate(&manifest).is_err(), "принят путь {path}");
        }
        for path in ["mods/a.jar", "config/.defaults", "resourcepacks/pack 1.zip"] {
            let mut manifest = valid_manifest();
            manifest["files"][0]["path"] = json!(path);
            validate(&manifest).expect("совместимый путь");
        }
    }

    #[test]
    fn rejects_wrong_hash_url_java_and_loader() {
        let mut upper = valid_manifest();
        upper["files"][0]["hash"] = json!(format!("sha256:{}", "A".repeat(64)));
        assert!(validate(&upper).is_err());

        let mut url = valid_manifest();
        url["files"][0]["url"] = json!("/api/v1/launcher/artifacts/1");
        assert!(validate(&url).is_err());

        let mut absolute = valid_manifest();
        absolute["files"][0]["url"] = json!("https://evil.example/a.jar");
        assert!(validate(&absolute).is_err());

        let mut java = valid_manifest();
        java["java"]["major"] = json!(21);
        assert!(validate(&java).is_err());

        let mut quilt = valid_manifest();
        quilt["minecraft"]["loader"] = json!("quilt");
        assert!(validate(&quilt).is_err());
    }

    #[test]
    fn java_mapping_matches_launcher() {
        assert_eq!(required_java_major("1.16.5"), 8);
        assert_eq!(required_java_major("1.17.1"), 16);
        assert_eq!(required_java_major("1.20.4"), 17);
        assert_eq!(required_java_major("1.20.1"), 17);
        assert_eq!(required_java_major("1.20.5"), 21);
        assert_eq!(required_java_major("1.21.1"), 21);
        assert_eq!(required_java_major("26.3"), 25);
    }

    #[test]
    fn snapshot_hash_is_stable() {
        let manifest = valid_manifest();
        assert_eq!(snapshot_hash(&manifest), snapshot_hash(&manifest));
        let mut changed = manifest.clone();
        changed["version"] = json!("1.0.1");
        assert_ne!(snapshot_hash(&manifest), snapshot_hash(&changed));
    }
}
