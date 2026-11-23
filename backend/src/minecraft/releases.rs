//! Релизы сборок: снапшот файлов, публикация, откат, сравнение.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{checks, launcher_manifest, manifests, validation};
use crate::core::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRow {
    pub id: Uuid,
    pub build_id: Uuid,
    pub version: String,
    pub status: String,
    pub channel: String,
    pub changelog: String,
    pub file_count: i32,
    pub total_bytes: i64,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deprecated_at: Option<DateTime<Utc>>,
    pub is_current: bool,
}

const RELEASE_SELECT: &str = "select r.id, r.build_id, r.version, r.status, r.channel, r.changelog,
        r.file_count, r.total_bytes, r.created_at, r.published_at, r.archived_at,
        r.deprecated_at, coalesce(b.current_release_id = r.id, false) as is_current
     from minecraft_releases r
     join minecraft_builds b on b.id = r.build_id";

pub async fn list(db: &PgPool, build_id: Uuid) -> ApiResult<Vec<ReleaseRow>> {
    sqlx::query_as::<_, ReleaseRow>(&format!(
        "{RELEASE_SELECT} where r.build_id = $1 order by r.created_at desc"
    ))
    .bind(build_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}

pub async fn load(db: &PgPool, build_id: Uuid, release_id: Uuid) -> ApiResult<ReleaseRow> {
    sqlx::query_as::<_, ReleaseRow>(&format!(
        "{RELEASE_SELECT} where r.id = $1 and r.build_id = $2"
    ))
    .bind(release_id)
    .bind(build_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Релиз не найден."))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReleaseRequest {
    pub version: Option<String>,
    #[serde(default)]
    pub changelog: String,
    /// Канал лаунчера: `stable` (по умолчанию) или `beta`.
    #[serde(default)]
    pub channel: Option<String>,
}

fn validate_channel(raw: Option<&str>) -> ApiResult<String> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("stable") => Ok("stable".to_owned()),
        Some("beta") => Ok("beta".to_owned()),
        Some(_) => Err(ApiError::validation(
            "Канал релиза должен быть stable или beta.",
        )),
    }
}

fn next_version(current: &str) -> String {
    let parts: Vec<&str> = current.split('.').collect();
    if parts.len() == 3 {
        if let (Ok(major), Ok(minor), Ok(patch)) = (
            parts[0].parse::<u64>(),
            parts[1].parse::<u64>(),
            parts[2].parse::<u64>(),
        ) {
            return format!("{major}.{minor}.{}", patch + 1);
        }
    }
    let now = Utc::now();
    format!("{}.{}.0", now.format("%Y"), now.format("%m%d"))
}

/// Создаёт черновик релиза со снимком текущих файлов сборки.
pub async fn create(
    db: &PgPool,
    actor_id: Uuid,
    build_id: Uuid,
    request: CreateReleaseRequest,
) -> ApiResult<ReleaseRow> {
    let build = super::builds::load(db, build_id).await?;
    if build.archived_at.is_some() {
        return Err(ApiError::conflict(
            "BUILD_ARCHIVED",
            "Сборка в архиве: публикация недоступна.",
        ));
    }

    let last: Option<String> = sqlx::query_scalar(
        "select version from minecraft_releases where build_id = $1 order by created_at desc limit 1",
    )
    .bind(build_id)
    .fetch_optional(db)
    .await?;
    let version = match request.version.as_deref().filter(|v| !v.trim().is_empty()) {
        Some(raw) => validation::validate_version(raw)?,
        None => next_version(last.as_deref().unwrap_or(&build.version)),
    };

    let taken: Option<Uuid> = sqlx::query_scalar(
        "select id from minecraft_releases where build_id = $1 and version = $2",
    )
    .bind(build_id)
    .bind(&version)
    .fetch_optional(db)
    .await?;
    if taken.is_some() {
        return Err(ApiError::conflict(
            "RELEASE_VERSION_TAKEN",
            "Релиз с такой версией уже существует.",
        ));
    }

    let stats: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(size_bytes), 0)::bigint
         from minecraft_build_files where build_id = $1 and kind = 'file' and enabled = true",
    )
    .bind(build_id)
    .fetch_one(db)
    .await?;

    let channel = validate_channel(request.channel.as_deref())?;

    let mut tx = db.begin().await?;
    let release_id: Uuid = sqlx::query_scalar(
        "insert into minecraft_releases
           (build_id, version, status, channel, changelog, file_count, total_bytes, created_by)
         values ($1, $2, 'draft', $3, $4, $5, $6, $7)
         returning id",
    )
    .bind(build_id)
    .bind(&version)
    .bind(&channel)
    .bind(request.changelog.trim())
    .bind(stats.0 as i32)
    .bind(stats.1)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    // Снимок файлов: дальнейшие изменения сборки не затрагивают релиз.
    sqlx::query(
        "insert into minecraft_release_files
           (release_id, path, category, side, required, enabled, size_bytes, sha256,
            object_id, source_file_id)
         select $1, path, category, side, required, enabled, size_bytes, sha256,
                object_id, id
         from minecraft_build_files
         where build_id = $2 and kind = 'file' and enabled = true",
    )
    .bind(release_id)
    .bind(build_id)
    .execute(&mut *tx)
    .await?;

    // Снимок удерживает собственные ссылки на объекты.
    sqlx::query(
        "update storage_objects o
         set ref_count = ref_count + sub.cnt
         from (
            select object_id, count(*) as cnt
            from minecraft_release_files
            where release_id = $1 and object_id is not null
            group by object_id
         ) sub
         where o.id = sub.object_id",
    )
    .bind(release_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "insert into minecraft_release_history
           (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, $2, 'release.created', $3, '', $4)",
    )
    .bind(build_id)
    .bind(release_id)
    .bind(actor_id)
    .bind(json!({ "version": version }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    load(db, build_id, release_id).await
}

/// Переводит сборку в статус `published` и делает указанный релиз текущим.
///
/// Вызывается только внутри транзакции публикации/активации релиза.
async fn mark_build_published(
    conn: &mut sqlx::PgConnection,
    build_id: Uuid,
    release_id: Uuid,
    version: &str,
) -> ApiResult<()> {
    sqlx::query(
        "update minecraft_builds
         set status = 'published', current_release_id = $2, version = $3, updated_at = now()
         where id = $1 and archived_at is null",
    )
    .bind(build_id)
    .bind(release_id)
    .bind(version)
    .execute(conn)
    .await?;
    Ok(())
}

/// Приводит статус сборки в соответствие с её опубликованными релизами.
///
/// Если после смены статуса текущего релиза (`deprecated`/`archived`) остаётся
/// другой опубликованный релиз, он становится текущим, а сборка остаётся
/// опубликованной. Иначе сборка возвращается в черновики. Без этого сборка
/// могла остаться в статусе `draft` с уже опубликованным текущим релизом:
/// интерфейс показывал черновик, а каталог лаунчера — ничего.
async fn sync_build_publication(conn: &mut sqlx::PgConnection, build_id: Uuid) -> ApiResult<()> {
    let next: Option<(Uuid, String)> = sqlx::query_as(
        "select id, version
         from minecraft_releases
         where build_id = $1 and status = 'published'
         order by coalesce(
                      id = (select current_release_id from minecraft_builds where id = $1),
                      false
                  ) desc,
                  published_at desc nulls last,
                  created_at desc
         limit 1",
    )
    .bind(build_id)
    .fetch_optional(&mut *conn)
    .await?;

    match next {
        Some((release_id, version)) => {
            mark_build_published(conn, build_id, release_id, &version).await?;
        }
        None => {
            sqlx::query(
                "update minecraft_builds
                 set status = 'draft', current_release_id = null, updated_at = now()
                 where id = $1 and archived_at is null",
            )
            .bind(build_id)
            .execute(conn)
            .await?;
        }
    }
    Ok(())
}

/// Публикует релиз: обязательна успешная проверка сборки.
pub async fn publish(
    db: &PgPool,
    actor_id: Uuid,
    actor_username: &str,
    build_id: Uuid,
    release_id: Uuid,
    accept_warnings: bool,
) -> ApiResult<ReleaseRow> {
    let build = super::builds::load(db, build_id).await?;
    if build.archived_at.is_some() {
        return Err(ApiError::conflict(
            "BUILD_ARCHIVED",
            "Сборка в архиве: сначала восстановите её.",
        ));
    }
    let release = load(db, build_id, release_id).await?;
    if release.status == "published" {
        // Идемпотентный повтор: если сборку ранее вернули в черновики, но её
        // текущий релиз остался опубликованным, восстанавливаем публикацию.
        if build.status != "published" || build.current_release_id != Some(release_id) {
            let mut tx = db.begin().await?;
            mark_build_published(&mut *tx, build_id, release_id, &release.version).await?;
            tx.commit().await?;
        }
        return load(db, build_id, release_id).await;
    }
    if release.status == "archived" {
        return Err(ApiError::conflict(
            "RELEASE_ARCHIVED",
            "Релиз в архиве: публикация недоступна.",
        ));
    }

    let report = checks::validate(db, build_id).await?;
    if !report.valid {
        return Err(ApiError::validation("Проверка сборки нашла ошибки.")
            .with_details(checks::report_json(&report)));
    }
    if !report.warnings.is_empty() && !accept_warnings {
        return Err(ApiError::conflict(
            "VALIDATION_WARNINGS",
            "Проверка сборки вернула предупреждения: подтвердите публикацию.",
        )
        .with_details(checks::report_json(&report)));
    }

    // Манифест для браузерного API строится из снимка релиза.
    let manifest = manifests::build_manifest(db, build_id, release_id).await?;
    manifests::validate_manifest(&manifest)?;

    // Манифест MyLauncher фиксируется тем же снимком релиза и проверяется по
    // точному контракту клиента. Ошибка блокирует публикацию.
    let launcher_manifest = launcher_manifest::build(db, build_id, release_id)
        .await
        .map_err(|error| {
            ApiError::validation("Сборка не готова для MyLauncher.")
                .with_details(json!({ "launcher": error.message }))
        })?;
    launcher_manifest::validate(&launcher_manifest)?;
    let launcher_hash = launcher_manifest::snapshot_hash(&launcher_manifest);

    let mut tx = db.begin().await?;
    sqlx::query(
        "update minecraft_releases
         set status = 'published', published_at = now(), manifest = $2,
             launcher_manifest = $3, launcher_manifest_hash = $4,
             launcher_manifest_schema = $5, launcher_manifest_at = now(),
             deprecated_at = null
         where id = $1",
    )
    .bind(release_id)
    .bind(&manifest)
    .bind(&launcher_manifest)
    .bind(&launcher_hash)
    .bind(launcher_manifest::SCHEMA_VERSION)
    .execute(&mut *tx)
    .await?;
    mark_build_published(&mut *tx, build_id, release_id, &release.version).await?;
    sqlx::query(
        "insert into minecraft_release_history
           (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, $2, 'release.published', $3, $4, $5)",
    )
    .bind(build_id)
    .bind(release_id)
    .bind(actor_id)
    .bind(actor_username)
    .bind(json!({
        "version": release.version,
        "files": report.file_count,
        "bytes": report.total_bytes,
        "warnings": report.warnings.len(),
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    load(db, build_id, release_id).await
}

/// Переключает текущий релиз (откат на предыдущую версию).
pub async fn set_current(
    db: &PgPool,
    actor_id: Uuid,
    build_id: Uuid,
    release_id: Uuid,
) -> ApiResult<ReleaseRow> {
    let build = super::builds::load(db, build_id).await?;
    if build.archived_at.is_some() {
        return Err(ApiError::conflict(
            "BUILD_ARCHIVED",
            "Сборка в архиве: сначала восстановите её.",
        ));
    }
    let release = load(db, build_id, release_id).await?;
    if release.status != "published" {
        return Err(ApiError::conflict(
            "RELEASE_NOT_PUBLISHED",
            "Текущим можно сделать только опубликованный релиз.",
        ));
    }
    let mut tx = db.begin().await?;
    // Активация опубликованного релиза всегда возвращает сборку в каталог:
    // до этого сборка могла остаться в черновиках после архивирования релиза.
    mark_build_published(&mut *tx, build_id, release_id, &release.version).await?;
    sqlx::query(
        "insert into minecraft_release_history
           (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, $2, 'release.activated', $3, '', $4)",
    )
    .bind(build_id)
    .bind(release_id)
    .bind(actor_id)
    .bind(json!({ "version": release.version }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    load(db, build_id, release_id).await
}

/// Меняет статус релиза (`deprecated`, `archived`).
pub async fn set_status(
    db: &PgPool,
    actor_id: Uuid,
    build_id: Uuid,
    release_id: Uuid,
    status: &str,
    event: &str,
) -> ApiResult<ReleaseRow> {
    if !matches!(status, "deprecated" | "archived") {
        return Err(ApiError::validation("Недопустимый статус релиза."));
    }
    let release = load(db, build_id, release_id).await?;
    if release.status == status {
        return Ok(release);
    }
    if release.status == "archived" {
        return Err(ApiError::conflict(
            "RELEASE_ARCHIVED",
            "Релиз уже в архиве.",
        ));
    }

    let mut tx = db.begin().await?;
    if status == "deprecated" {
        sqlx::query(
            "update minecraft_releases set status = 'deprecated', deprecated_at = now() where id = $1",
        )
        .bind(release_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "update minecraft_releases set status = 'archived', archived_at = now() where id = $1",
        )
        .bind(release_id)
        .execute(&mut *tx)
        .await?;
    }
    // Если текущий релиз снят с публикации или архивирован, сборка не должна
    // оставаться «опубликованной» со ссылкой на скрытый релиз. Выбираем
    // следующий опубликованный релиз либо возвращаем сборку в черновики.
    if release.is_current {
        sync_build_publication(&mut *tx, build_id).await?;
    }
    sqlx::query(
        "insert into minecraft_release_history
           (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, $2, $3, $4, '', $5)",
    )
    .bind(build_id)
    .bind(release_id)
    .bind(event)
    .bind(actor_id)
    .bind(json!({ "version": release.version, "status": status }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    load(db, build_id, release_id).await
}

/// Снимает сборку с публикации: опубликованные и устаревшие релизы
/// переводятся в `unpublished`, сборка возвращается в черновики.
///
/// Снимки релизов и их манифесты сохраняются: публикацию можно вернуть,
/// опубликовав нужный релиз снова. В отличие от архивирования сборки,
/// карточка остаётся в рабочем пространстве.
pub async fn unpublish_build(
    db: &PgPool,
    actor_id: Uuid,
    actor_username: &str,
    build_id: Uuid,
) -> ApiResult<super::builds::BuildRow> {
    let build = super::builds::load(db, build_id).await?;
    if build.archived_at.is_some() {
        return Err(ApiError::conflict(
            "BUILD_ARCHIVED",
            "Сборка в архиве: сначала восстановите её.",
        ));
    }

    // Снимаем с публикации оба канала: иначе сборка останется в каталоге.
    let releases: Vec<(Uuid, String)> = sqlx::query_as(
        "select id, version from minecraft_releases
         where build_id = $1 and status in ('published', 'deprecated')
         order by created_at",
    )
    .bind(build_id)
    .fetch_all(db)
    .await?;

    if releases.is_empty() && build.status != "published" && build.current_release_id.is_none() {
        return Ok(build);
    }

    let mut tx = db.begin().await?;
    sqlx::query(
        "update minecraft_releases
         set status = 'unpublished'
         where build_id = $1 and status in ('published', 'deprecated')",
    )
    .bind(build_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "update minecraft_builds
         set current_release_id = null, status = 'draft', updated_at = now()
         where id = $1",
    )
    .bind(build_id)
    .execute(&mut *tx)
    .await?;

    let versions: Vec<&str> = releases
        .iter()
        .map(|(_, version)| version.as_str())
        .collect();
    sqlx::query(
        "insert into minecraft_release_history
           (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, null, 'build.unpublished', $2, $3, $4)",
    )
    .bind(build_id)
    .bind(actor_id)
    .bind(actor_username)
    .bind(json!({ "releases": releases.len(), "versions": versions }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    super::builds::load(db, build_id).await
}

/// Сравнивает два релиза: добавленные, удалённые и изменённые файлы.
pub async fn compare(
    db: &PgPool,
    build_id: Uuid,
    left_id: Uuid,
    right_id: Uuid,
) -> ApiResult<serde_json::Value> {
    let left = load(db, build_id, left_id).await?;
    let right = load(db, build_id, right_id).await?;
    let left_files: Vec<(String, String, i64)> = sqlx::query_as(
        "select path, sha256, size_bytes from minecraft_release_files where release_id = $1",
    )
    .bind(left_id)
    .fetch_all(db)
    .await?;
    let right_files: Vec<(String, String, i64)> = sqlx::query_as(
        "select path, sha256, size_bytes from minecraft_release_files where release_id = $1",
    )
    .bind(right_id)
    .fetch_all(db)
    .await?;

    let map_left: std::collections::HashMap<String, (String, i64)> = left_files
        .into_iter()
        .map(|(path, sha, size)| (super::validation::path_key(&path), (sha, size)))
        .collect();
    let map_right: std::collections::HashMap<String, (String, i64)> = right_files
        .into_iter()
        .map(|(path, sha, size)| (super::validation::path_key(&path), (sha, size)))
        .collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (key, (sha, size)) in &map_right {
        match map_left.get(key) {
            None => added.push(json!({ "path": key, "size": size, "sha256": sha })),
            Some((old_sha, old_size)) if old_sha != sha || old_size != size => changed.push(
                json!({ "path": key, "size": size, "sha256": sha, "previousSha256": old_sha }),
            ),
            _ => {}
        }
    }
    for (key, (sha, size)) in &map_left {
        if !map_right.contains_key(key) {
            removed.push(json!({ "path": key, "size": size, "sha256": sha }));
        }
    }

    Ok(json!({
        "left": { "id": left.id, "version": left.version },
        "right": { "id": right.id, "version": right.version },
        "added": added,
        "removed": removed,
        "changed": changed,
    }))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    pub id: Uuid,
    pub build_id: Uuid,
    pub release_id: Option<Uuid>,
    pub event: String,
    pub actor_username: String,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn history(db: &PgPool, build_id: Uuid, limit: i64) -> ApiResult<Vec<HistoryRow>> {
    sqlx::query_as::<_, HistoryRow>(
        "select id, build_id, release_id, event, actor_username, details, created_at
         from minecraft_release_history
         where build_id = $1
         order by created_at desc
         limit $2",
    )
    .bind(build_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}
