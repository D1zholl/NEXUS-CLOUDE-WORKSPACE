//! Формат манифеста сборки (`schema_version`).
//!
//! Манифест строится из снимка релиза, а не из текущего состояния сборки:
//! повторная генерация опубликованного релиза даёт то же логическое содержимое.

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};

pub const SCHEMA_VERSION: &str = "1.0";

/// Относительный URL иконки сборки для лаунчера.
pub fn icon_url(slug: &str) -> String {
    format!("/api/v1/launcher/builds/{slug}/icon")
}

/// Собирает манифест релиза.
///
/// Порядок файлов задаётся байтовой сортировкой по `path` в памяти, а не
/// `order by lower(path)` в PostgreSQL: результат не должен зависеть от
/// локали/коллации базы. Пути уже приведены к NFC и прямым слэшам при загрузке.
pub async fn build_manifest(db: &PgPool, build_id: Uuid, release_id: Uuid) -> ApiResult<Value> {
    let build = super::builds::load(db, build_id).await?;
    let release = super::releases::load(db, build_id, release_id).await?;

    let mut files: Vec<(Uuid, String, String, String, bool, i64, String)> = sqlx::query_as(
        "select id as artifact_id, path, category, side,
                required, size_bytes, sha256
         from minecraft_release_files
         where release_id = $1 and enabled = true",
    )
    .bind(release_id)
    .fetch_all(db)
    .await?;

    // Снимок релиза не должен содержать путей, которые на Windows означают
    // другое расположение или устройство, и путей-двойников без учёта регистра.
    let paths: Vec<String> = files.iter().map(|file| file.1.clone()).collect();
    validate_paths(&paths)?;

    // Стабильный байтовый порядок: без учёта локали, регистра и нормализации
    // конкретной ОС. Идентификатор — детерминированный tie-breaker.
    files.sort_by(|left, right| {
        left.1
            .as_bytes()
            .cmp(right.1.as_bytes())
            .then_with(|| left.0.cmp(&right.0))
    });

    let file_entries: Vec<Value> = files
        .into_iter()
        .map(
            |(artifact_id, path, category, side, required, size, sha256)| {
                json!({
                    "id": artifact_id,
                    "path": path,
                    "category": category,
                    "side": side,
                    "required": required,
                    "size": size,
                    "sha256": sha256,
                    "download_url": format!("/api/v1/launcher/artifacts/{artifact_id}"),
                })
            },
        )
        .collect();

    let mut manifest = json!({
        "schema_version": SCHEMA_VERSION,
        "build": {
            "id": build.id,
            "slug": build.slug,
            "name": build.name,
            "release": release.version,
            "changelog": release.changelog,
        },
        "minecraft": {
            "version": build.minecraft_version,
        },
        "loader": {
            "type": build.loader_type,
            "version": build.loader_version,
        },
        "java": {
            "major": build.java_major,
            "min": build.java_min,
            "notes": build.java_notes,
        },
        "files": file_entries,
    });

    // Иконка аддитивна: берём SHA-256 готового объекта из storage_objects.
    // Если объект не committed, поле не добавляется — манифест остаётся валидным.
    let icon_sha: Option<String> = sqlx::query_scalar(
        "select o.sha256
         from minecraft_builds b
         join storage_objects o on o.id = b.icon_object_id
         where b.id = $1 and o.state = 'committed'",
    )
    .bind(build_id)
    .fetch_optional(db)
    .await?;
    if let Some(sha256) = icon_sha {
        manifest["icon"] = json!({
            "url": icon_url(&build.slug),
            "sha256": sha256,
        });
    }

    Ok(manifest)
}

/// Проверяет пути манифеста: переносимость на Windows и отсутствие коллизий.
pub fn validate_manifest_paths(manifest: &Value) -> ApiResult<()> {
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::internal("Манифест без списка файлов."))?;
    let paths: Vec<String> = files
        .iter()
        .map(|file| {
            file.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    validate_paths(&paths)
}

fn validate_paths(paths: &[String]) -> ApiResult<()> {
    for path in paths {
        super::validation::check_manifest_path(path).map_err(|reason| {
            ApiError::internal(format!(
                "Манифест содержит недопустимый путь «{path}»: {reason}."
            ))
        })?;
    }
    super::validation::check_unique_paths(paths).map_err(|reason| {
        ApiError::internal(format!("Манифест содержит конфликт путей: {reason}."))
    })
}

/// Проверяет необязательный блок `icon`.
///
/// Поле аддитивно схеме 1.0, но его форма фиксирована: относительный URL именно
/// этой сборки и SHA-256 в нижнем регистре. Иконка не должна быть серверным
/// путём или ссылкой на чужой ресурс.
fn validate_icon(manifest: &Value, slug: &str) -> ApiResult<()> {
    let Some(icon) = manifest.get("icon") else {
        return Ok(());
    };
    let icon = icon
        .as_object()
        .ok_or_else(|| ApiError::internal("Блок icon манифеста должен быть объектом."))?;
    if icon.len() != 2 || !icon.contains_key("url") || !icon.contains_key("sha256") {
        return Err(ApiError::internal(
            "Блок icon манифеста должен содержать только url и sha256.",
        ));
    }
    let url = icon.get("url").and_then(Value::as_str).unwrap_or_default();
    if url != icon_url(slug) {
        return Err(ApiError::internal(
            "Манифест содержит некорректный URL иконки.",
        ));
    }
    let sha = icon
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !is_sha256(sha) {
        return Err(ApiError::internal(
            "Манифест содержит некорректный SHA-256 иконки.",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, 'a'..='f'))
}

/// Разрешена ли регенерация опубликованного манифеста.
///
/// Менять можно только необязательный `icon`. Порядок `files` может
/// нормализоваться к детерминированному байтовому порядку, но набор файлов и
/// их `path`/`size`/`sha256`/`category`/`side`/`required`/`download_url`
/// обязаны совпадать. Остальные блоки (`build`, `minecraft`, `loader`, `java`)
/// сравниваются целиком. Исключение — пустой/устаревший документ (`{}`),
/// который ещё ни разу не материализовался: его первичная запись считается
/// аддитивной.
pub fn regeneration_is_additive(previous: &Value, next: &Value) -> bool {
    let Some(previous_object) = previous.as_object() else {
        return false;
    };
    if previous_object.is_empty()
        || !previous_object.contains_key("schema_version")
        || !previous_object.contains_key("files")
    {
        return true;
    }
    if !next.is_object() {
        return false;
    }

    // Все поля, кроме icon и порядка files, должны совпадать.
    let without_files = |value: &Value| {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("icon");
            object.remove("files");
        }
        value
    };
    if without_files(previous) != without_files(next) {
        return false;
    }

    let files_by_id = |value: &Value| -> Option<std::collections::BTreeMap<String, Value>> {
        let files = value.get("files")?.as_array()?;
        let mut map = std::collections::BTreeMap::new();
        for file in files {
            let id = file.get("id")?.as_str()?.to_owned();
            if map.insert(id, file.clone()).is_some() {
                return None;
            }
        }
        Some(map)
    };
    match (files_by_id(previous), files_by_id(next)) {
        (Some(previous), Some(next)) => previous == next,
        _ => false,
    }
}

/// Проверяет структуру манифеста перед сохранением и публикацией.
pub fn validate_manifest(manifest: &Value) -> ApiResult<()> {
    let schema = manifest
        .get("schema_version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if schema != SCHEMA_VERSION {
        return Err(ApiError::internal("Некорректная версия схемы манифеста."));
    }
    let build = manifest
        .get("build")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::internal("Манифест без блока build."))?;
    let slug = build
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if slug.is_empty() {
        return Err(ApiError::internal("Манифест без slug сборки."));
    }
    let release = build
        .get("release")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if release.is_empty() {
        return Err(ApiError::internal("Манифест без версии релиза."));
    }
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::internal("Манифест без списка файлов."))?;
    validate_manifest_paths(manifest)?;
    validate_icon(manifest, slug)?;
    for file in files {
        let sha = file
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if sha.len() != 64 || !sha.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(ApiError::internal("Манифест содержит файл без SHA-256."));
        }
        if file.get("size").and_then(Value::as_i64).unwrap_or(-1) < 0 {
            return Err(ApiError::internal("Манифест содержит файл без размера."));
        }
        let url = file
            .get("download_url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if url.contains(":\\") || url.starts_with("//") {
            return Err(ApiError::internal(
                "Манифест не должен содержать серверные пути.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(files: Value) -> Value {
        json!({
            "schema_version": SCHEMA_VERSION,
            "build": { "id": "b", "slug": "demo", "name": "Demo", "release": "1.0.0", "changelog": "" },
            "minecraft": { "version": "1.20.1" },
            "loader": { "type": "fabric", "version": "0.15.0" },
            "java": { "major": 17, "min": 17, "notes": "" },
            "files": files,
        })
    }

    #[test]
    fn accepts_valid_manifest() {
        let value = manifest(json!([{
            "id": "1", "path": "mods/a.jar", "category": "mods", "side": "client",
            "required": true, "size": 10,
            "sha256": "a".repeat(64),
            "download_url": "/api/v1/launcher/artifacts/1"
        }]));
        assert!(validate_manifest(&value).is_ok());
    }

    #[test]
    fn accepts_and_checks_optional_icon() {
        let mut value = manifest(json!([file("mods/a.jar")]));
        value["icon"] = json!({
            "url": icon_url("demo"),
            "sha256": "b".repeat(64),
        });
        assert!(validate_manifest(&value).is_ok());

        // URL должен указывать именно на эту сборку.
        let mut foreign = value.clone();
        foreign["icon"]["url"] = json!("/api/v1/launcher/builds/other/icon");
        assert!(validate_manifest(&foreign).is_err());

        // SHA-256 обязателен и должен быть hex в нижнем регистре.
        let mut broken = value.clone();
        broken["icon"]["sha256"] = json!("not-a-hash");
        assert!(validate_manifest(&broken).is_err());

        let mut upper = value.clone();
        upper["icon"]["sha256"] = json!("B".repeat(64));
        assert!(validate_manifest(&upper).is_err());

        // Посторонние поля в icon запрещены контрактом 1.0.
        let mut extra = value;
        extra["icon"]["contentType"] = json!("image/png");
        assert!(validate_manifest(&extra).is_err());
    }

    #[test]
    fn regeneration_is_additive_only_for_icon() {
        let base = manifest(json!([
            file_id("a", "mods/a.jar"),
            file_id("b", "mods/b.jar")
        ]));
        let mut with_icon = base.clone();
        with_icon["icon"] = json!({
            "url": icon_url("demo"),
            "sha256": "b".repeat(64),
        });
        assert!(regeneration_is_additive(&base, &with_icon));

        // Пустой сохранённый документ — первичная материализация.
        assert!(regeneration_is_additive(&json!({}), &with_icon));

        // Нормализация порядка files допустима: содержимое не меняется.
        let mut reordered = base.clone();
        reordered["files"] = json!([file_id("b", "mods/b.jar"), file_id("a", "mods/a.jar")]);
        assert!(regeneration_is_additive(&base, &reordered));

        // Любое изменение файлов или метаданных — уже не аддитивно.
        let mut changed_path = with_icon.clone();
        changed_path["files"][0]["path"] = json!("mods/renamed.jar");
        assert!(!regeneration_is_additive(&base, &changed_path));

        let mut changed_sha = with_icon.clone();
        changed_sha["files"][0]["sha256"] = json!("c".repeat(64));
        assert!(!regeneration_is_additive(&base, &changed_sha));

        let mut extra_file = base.clone();
        extra_file["files"] = json!([
            file_id("a", "mods/a.jar"),
            file_id("b", "mods/b.jar"),
            file_id("c", "mods/c.jar")
        ]);
        assert!(!regeneration_is_additive(&base, &extra_file));

        let mut changed_name = with_icon;
        changed_name["build"]["name"] = json!("Renamed");
        assert!(!regeneration_is_additive(&base, &changed_name));
    }

    #[test]
    fn rejects_bad_paths_and_hashes() {
        let escape = manifest(json!([{
            "id": "1", "path": "../evil.jar", "category": "mods", "side": "client",
            "required": true, "size": 10, "sha256": "a".repeat(64),
            "download_url": "/api/v1/launcher/artifacts/1"
        }]));
        assert!(validate_manifest(&escape).is_err());

        let no_hash = manifest(json!([{
            "id": "1", "path": "mods/a.jar", "category": "mods", "side": "client",
            "required": true, "size": 10, "sha256": "",
            "download_url": "/api/v1/launcher/artifacts/1"
        }]));
        assert!(validate_manifest(&no_hash).is_err());
    }

    fn file(path: &str) -> Value {
        file_id("1", path)
    }

    fn file_id(id: &str, path: &str) -> Value {
        json!({
            "id": id, "path": path, "category": "mods", "side": "client",
            "required": true, "size": 10, "sha256": "a".repeat(64),
            "download_url": format!("/api/v1/launcher/artifacts/{id}")
        })
    }

    /// Каждый недопустимый вариант пути отвергается манифестом, а не только
    /// внутренним нормализатором загрузки.
    #[test]
    fn rejects_every_non_portable_path() {
        for path in [
            "/etc/passwd",
            "C:/Windows/x.dll",
            "C:evil.jar",
            "mods\\a.jar",
            "mods/a\\b.jar",
            "//server/share/a.jar",
            "\\\\server\\share\\a.jar",
            "../a.jar",
            "mods/../../a.jar",
            "mods/..",
            "mods/.",
            "mods/a.jar:stream",
            "mods/a.jar::$DATA",
            "CON",
            "mods/nul.txt",
            "mods/COM1.jar",
            "mods/LPT9",
            "mods/trailing.",
            "mods/trailing ",
            "mods//a.jar",
            "mods/a\u{1}.jar",
            "mods/a?.jar",
            "",
            "config/\u{0438}\u{0306}.json",
        ] {
            let value = manifest(json!([file(path)]));
            assert!(
                validate_manifest(&value).is_err(),
                "манифест должен отвергнуть путь {path:?}"
            );
        }
    }

    /// Регистронезависимые коллизии внутри одной сборки — ошибка: на Windows
    /// `Mods/A.jar` и `mods/a.jar` перезаписывают друг друга.
    #[test]
    fn rejects_case_insensitive_collisions() {
        let value = manifest(json!([file("Mods/A.jar"), file("mods/a.jar")]));
        let error = validate_manifest(&value).expect_err("коллизия путей");
        assert_eq!(error.code, "INTERNAL_ERROR");

        // Разные файлы в разных папках по-прежнему допустимы.
        let ok = manifest(json!([file("mods/a.jar"), file("config/a.jar")]));
        assert!(validate_manifest(&ok).is_ok());
    }
}
