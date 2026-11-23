//! Каталог публичных сборок и манифесты для лаунчера.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};

/// Сборка в каталоге лаунчера.
///
/// `owner_id` нужен для проверки доступа: приватная сборка видна только
/// владельцу токена. Это поле не сериализуется в ответ лаунчера.
#[derive(Debug, sqlx::FromRow)]
pub struct CatalogBuild {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub minecraft_version: String,
    pub loader_type: String,
    pub loader_version: String,
    pub java_major: i32,
    pub java_min: i32,
    pub java_notes: String,
    pub owner_id: Option<Uuid>,
    pub release_version: Option<String>,
    pub release_id: Option<Uuid>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub file_count: i64,
    pub total_bytes: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const CATALOG_SELECT: &str = "select b.id, b.slug, b.name, b.description, b.category,
        b.minecraft_version, b.loader_type, b.loader_version, b.java_major, b.java_min,
        b.java_notes, b.owner_id, r.version as release_version, r.id as release_id, r.published_at,
        coalesce(r.file_count, 0)::bigint as file_count,
        coalesce(r.total_bytes, 0)::bigint as total_bytes,
        b.updated_at
     from minecraft_builds b
     left join minecraft_releases r on r.id = b.current_release_id
     where (b.visibility = 'public' or b.owner_id = $1)
       and b.archived_at is null
       and b.status = 'published'
       and r.status = 'published'";

/// Каталог: публичные сборки и приватные сборки текущего пользователя.
pub async fn catalog(db: &PgPool, viewer_user_id: Uuid) -> ApiResult<Vec<CatalogBuild>> {
    sqlx::query_as::<_, CatalogBuild>(&format!("{CATALOG_SELECT} order by b.name"))
        .bind(viewer_user_id)
        .fetch_all(db)
        .await
        .map_err(ApiError::from)
}

pub async fn build_by_slug(
    db: &PgPool,
    slug: &str,
    viewer_user_id: Uuid,
) -> ApiResult<CatalogBuild> {
    sqlx::query_as::<_, CatalogBuild>(&format!("{CATALOG_SELECT} and b.slug = $2"))
        .bind(viewer_user_id)
        .bind(slug)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found("Сборка не найдена или не опубликована."))
}

/// Сборка доступна по slug для чтения конкретного релиза, даже если текущий
/// релиз помечен как устаревший: старые релизы должны оставаться доступными.
pub async fn build_by_slug_for_release(
    db: &PgPool,
    slug: &str,
    viewer_user_id: Uuid,
) -> ApiResult<CatalogBuild> {
    sqlx::query_as::<_, CatalogBuild>(
        "select b.id, b.slug, b.name, b.description, b.category,
                b.minecraft_version, b.loader_type, b.loader_version, b.java_major, b.java_min,
                b.java_notes, b.owner_id, r.version as release_version, r.id as release_id,
                r.published_at,
                coalesce(r.file_count, 0)::bigint as file_count,
                coalesce(r.total_bytes, 0)::bigint as total_bytes,
                b.updated_at
         from minecraft_builds b
         left join minecraft_releases r on r.id = b.current_release_id
         where (b.owner_id = $2
                or (b.visibility = 'public' and b.status = 'published'))
           and b.archived_at is null
           and b.slug = $1
           and exists (
                select 1 from minecraft_releases rr
                where rr.build_id = b.id and rr.status in ('published', 'deprecated')
           )",
    )
    .bind(slug)
    .bind(viewer_user_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Сборка не найдена или не опубликована."))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRelease {
    pub id: Uuid,
    pub version: String,
    pub status: String,
    pub channel: String,
    pub changelog: String,
    pub file_count: i32,
    pub total_bytes: i64,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_current: bool,
}

pub async fn releases(db: &PgPool, build_id: Uuid) -> ApiResult<Vec<CatalogRelease>> {
    sqlx::query_as::<_, CatalogRelease>(
        "select r.id, r.version, r.status, r.channel, r.changelog, r.file_count, r.total_bytes,
                r.published_at, coalesce(b.current_release_id = r.id, false) as is_current
         from minecraft_releases r
         join minecraft_builds b on b.id = r.build_id
         where r.build_id = $1
           and r.status in ('published', 'deprecated')
         order by r.published_at desc nulls last",
    )
    .bind(build_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}

/// Манифест текущего опубликованного релиза.
pub async fn latest_manifest(db: &PgPool, slug: &str, viewer_user_id: Uuid) -> ApiResult<Value> {
    let build = build_by_slug(db, slug, viewer_user_id).await?;
    let release_id = build
        .release_id
        .ok_or_else(|| ApiError::not_found("У сборки нет опубликованного релиза."))?;
    manifest_json(db, &build, release_id).await
}

/// Манифест конкретной версии релиза.
pub async fn release_manifest(
    db: &PgPool,
    slug: &str,
    version: &str,
    viewer_user_id: Uuid,
) -> ApiResult<Value> {
    let build = build_by_slug_for_release(db, slug, viewer_user_id).await?;
    let release_id: Option<Uuid> = sqlx::query_scalar(
        "select id from minecraft_releases
         where build_id = $1 and version = $2 and status in ('published', 'deprecated')",
    )
    .bind(build.id)
    .bind(version)
    .fetch_optional(db)
    .await?;
    let release_id =
        release_id.ok_or_else(|| ApiError::not_found("Релиз не найден или не опубликован."))?;
    manifest_json(db, &build, release_id).await
}

/// Строит манифест из БД: сохранённый снимок релиза либо генерация на лету.
///
/// Ссылки на артефакты должны указывать на `minecraft_release_files.id`,
/// поэтому сохранённый документ проверяется на согласованность и при
/// расхождении пересобирается. Полный опубликованный документ при этом не
/// перезаписывается: менять его можно только аудируемой задачей
/// `system.manifests.regenerate`. Кеш заполняется лишь для релизов, у которых
/// манифест ещё не материализован (`{}`).
async fn manifest_json(db: &PgPool, build: &CatalogBuild, release_id: Uuid) -> ApiResult<Value> {
    let stored: Option<Value> = sqlx::query_scalar(
        "select manifest from minecraft_releases where id = $1 and build_id = $2",
    )
    .bind(release_id)
    .bind(build.id)
    .fetch_optional(db)
    .await?;
    let materialized = stored
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|object| object.contains_key("schema_version"));
    if let Some(value) = stored.filter(|value| value.get("schema_version").is_some()) {
        let known: Vec<Uuid> =
            sqlx::query_scalar("select id from minecraft_release_files where release_id = $1")
                .bind(release_id)
                .fetch_all(db)
                .await?;
        // Снимок с небезопасными путями не отдаётся: он пересобирается из БД,
        // где пути уже прошли строгую проверку.
        if manifest_matches(&value, &known)
            && crate::minecraft::manifests::validate_manifest_paths(&value).is_ok()
        {
            return Ok(value);
        }
    }
    let generated = crate::minecraft::manifests::build_manifest(db, build.id, release_id).await?;
    if !materialized {
        // Первичная материализация пустого манифеста, а не изменение релиза.
        sqlx::query("update minecraft_releases set manifest = $2 where id = $1")
            .bind(release_id)
            .bind(&generated)
            .execute(db)
            .await
            .ok();
    }
    Ok(generated)
}

/// Проверяет, что манифест ссылается на существующие записи снимка релиза.
fn manifest_matches(value: &Value, known: &[Uuid]) -> bool {
    let Some(files) = value.get("files").and_then(Value::as_array) else {
        return false;
    };
    files.iter().all(|file| {
        let Some(id) = file.get("id").and_then(Value::as_str) else {
            return false;
        };
        let Ok(id) = Uuid::parse_str(id) else {
            return false;
        };
        known.contains(&id)
    })
}

/// Описание сборки для лаунчера (без файлов).
pub fn build_json(build: &CatalogBuild) -> Value {
    json!({
        "id": build.id,
        "slug": build.slug,
        "name": build.name,
        "description": build.description,
        "category": build.category,
        "minecraft": { "version": build.minecraft_version },
        "loader": { "type": build.loader_type, "version": build.loader_version },
        "java": { "major": build.java_major, "min": build.java_min, "notes": build.java_notes },
        "latestRelease": build.release_version,
        "latestReleaseId": build.release_id,
        "publishedAt": build.published_at,
        "fileCount": build.file_count,
        "totalBytes": build.total_bytes,
        "updatedAt": build.updated_at,
        "manifestUrl": format!("/api/v1/launcher/builds/{}/latest/manifest", build.slug),
    })
}

/// Проверяет, что артефакт принадлежит опубликованному релизу доступной сборки.
#[derive(Debug)]
pub struct ArtifactRef {
    pub build_id: Uuid,
    pub release_id: Uuid,
    pub path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub object_id: Uuid,
}

pub async fn find_artifact(
    db: &PgPool,
    artifact_id: Uuid,
    viewer_user_id: Uuid,
) -> ApiResult<ArtifactRef> {
    let row: Option<(Uuid, Uuid, String, i64, String, Option<Uuid>)> = sqlx::query_as(
        "select r.build_id, r.id, rf.path, rf.size_bytes, rf.sha256, rf.object_id
         from minecraft_release_files rf
         join minecraft_releases r on r.id = rf.release_id
         join minecraft_builds b on b.id = r.build_id
         where rf.id = $1
           and (b.visibility = 'public' or b.owner_id = $2)
           and r.status in ('published', 'deprecated')
           and b.archived_at is null
           and b.status = 'published'",
    )
    .bind(artifact_id)
    .bind(viewer_user_id)
    .fetch_optional(db)
    .await?;
    let Some((build_id, release_id, path, size_bytes, sha256, object_id)) = row else {
        return Err(ApiError::not_found("Артефакт не найден."));
    };
    // Некорректный путь не отдаём даже владельцу: контракт лаунчера —
    // переносимые относительные пути. Нарушение скрывается как 404.
    if crate::minecraft::validation::check_manifest_path(&path).is_err() {
        return Err(ApiError::not_found("Артефакт не найден."));
    }
    let object_id = object_id
        .ok_or_else(|| ApiError::conflict("OBJECT_UNAVAILABLE", "Артефакт недоступен."))?;
    Ok(ArtifactRef {
        build_id,
        release_id,
        path,
        size_bytes,
        sha256,
        object_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_are_relative() {
        // Проверяем контракт: манифест не должен содержать серверных путей.
        let value = json!({
            "schema_version": "1.0",
            "build": { "id": "b", "slug": "s", "name": "n", "release": "1.0.0", "changelog": "" },
            "minecraft": { "version": "1.20.1" },
            "loader": { "type": "vanilla", "version": "" },
            "java": { "major": 21, "min": 21, "notes": "" },
            "files": [{
                "id": "a", "path": "mods/x.jar", "category": "mods", "side": "client",
                "required": true, "size": 1, "sha256": "a".repeat(64),
                "download_url": "/api/v1/launcher/artifacts/a"
            }]
        });
        assert!(crate::minecraft::manifests::validate_manifest(&value).is_ok());
    }
}
