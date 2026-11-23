//! Сборки Minecraft: карточки, CRUD, клонирование, архивирование.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::validation::{self, validate_slug};
use crate::core::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BuildRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub category: String,
    pub visibility: String,
    pub status: String,
    pub minecraft_version: String,
    pub loader_type: String,
    pub loader_version: String,
    pub java_major: i32,
    pub java_min: i32,
    pub java_notes: String,
    pub icon_url: Option<String>,
    pub current_release_id: Option<Uuid>,
    pub current_release_version: Option<String>,
    pub file_count: i64,
    pub total_bytes: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

const BUILD_SELECT: &str = "select b.id, b.slug, b.name, b.description, b.version, b.category,
        b.visibility, b.status, b.minecraft_version, b.loader_type, b.loader_version,
        b.java_major, b.java_min, b.java_notes,
        case when b.icon_object_id is not null
             then '/api/v1/minecraft/builds/' || b.id::text || '/icon'
             else null end as icon_url,
        b.current_release_id,
        r.version as current_release_version,
        coalesce(f.file_count, 0)::bigint as file_count,
        coalesce(f.total_bytes, 0)::bigint as total_bytes,
        b.created_at, b.updated_at, b.archived_at
     from minecraft_builds b
     left join minecraft_releases r on r.id = b.current_release_id
     left join (
        select build_id, count(*)::bigint as file_count, coalesce(sum(size_bytes), 0)::bigint as total_bytes
        from minecraft_build_files
        where kind = 'file'
        group by build_id
     ) f on f.build_id = b.id";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub q: Option<String>,
    pub status: Option<String>,
    pub visibility: Option<String>,
    pub loader: Option<String>,
    pub category: Option<String>,
    pub sort: Option<String>,
    pub direction: Option<String>,
}

/// Сводка по сборкам + последние изменения и публикации.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStats {
    pub total: i64,
    pub published: i64,
    pub drafts: i64,
    pub archived: i64,
    pub files: i64,
    pub total_bytes: i64,
    pub recent_updated: Vec<BuildRef>,
    pub recent_published: Vec<BuildRef>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildRef {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub version: Option<String>,
    pub at: DateTime<Utc>,
}

pub async fn stats(db: &PgPool) -> ApiResult<BuildStats> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(
        "select count(*)::bigint,
                count(*) filter (where status = 'published')::bigint,
                count(*) filter (where status = 'draft')::bigint,
                count(*) filter (where status = 'archived')::bigint
         from minecraft_builds",
    )
    .fetch_one(db)
    .await?;
    let files: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(size_bytes), 0)::bigint from minecraft_build_files where kind = 'file'",
    )
    .fetch_one(db)
    .await?;

    let recent_updated =
        sqlx::query_as::<_, (Uuid, String, String, Option<String>, DateTime<Utc>)>(
            "select b.id, b.name, b.slug, r.version as version, b.updated_at
         from minecraft_builds b
         left join minecraft_releases r on r.id = b.current_release_id
         where b.archived_at is null
         order by b.updated_at desc
         limit 5",
        )
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(id, name, slug, version, at)| BuildRef {
            id,
            name,
            slug,
            version,
            at,
        })
        .collect();

    let recent_published =
        sqlx::query_as::<_, (Uuid, String, String, Option<String>, DateTime<Utc>)>(
            "select distinct on (b.id) b.id, b.name, b.slug, r.version, r.published_at
         from minecraft_builds b
         join minecraft_releases r on r.build_id = b.id
         where r.published_at is not null
         order by b.id, r.published_at desc",
        )
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(id, name, slug, version, at)| BuildRef {
            id,
            name,
            slug,
            version,
            at,
        })
        .collect::<Vec<_>>();

    let mut recent_published = recent_published;
    recent_published.sort_by_key(|item| std::cmp::Reverse(item.at));
    recent_published.truncate(5);

    Ok(BuildStats {
        total: row.0,
        published: row.1,
        drafts: row.2,
        archived: row.3,
        files: files.0,
        total_bytes: files.1,
        recent_updated,
        recent_published,
    })
}

pub async fn list(db: &PgPool, query: &ListQuery) -> ApiResult<(Vec<BuildRow>, i64)> {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(24).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(q) = query.q.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        binds.push(format!("%{}%", q.replace('%', "\\%").replace('_', "\\_")));
        conditions.push(format!(
            "(b.name ilike ${} escape '\\' or b.slug ilike ${} escape '\\')",
            binds.len(),
            binds.len()
        ));
    }
    for (value, column) in [
        (query.status.as_deref(), "b.status"),
        (query.visibility.as_deref(), "b.visibility"),
        (query.loader.as_deref(), "b.loader_type"),
        (query.category.as_deref(), "b.category"),
    ] {
        if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
            binds.push(value.to_owned());
            conditions.push(format!("{column} = ${}", binds.len()));
        }
    }

    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!(" where {}", conditions.join(" and "))
    };

    let order = match query.sort.as_deref().unwrap_or("updated") {
        "name" => "b.name",
        "created" => "b.created_at",
        "size" => "coalesce(f.total_bytes, 0)",
        "files" => "coalesce(f.file_count, 0)",
        _ => "b.updated_at",
    };
    let direction = if query.direction.as_deref() == Some("asc") {
        "asc"
    } else {
        "desc"
    };

    let sql = format!(
        "{BUILD_SELECT}{where_sql} order by {order} {direction} limit ${} offset ${}",
        binds.len() + 1,
        binds.len() + 2
    );
    let count_sql = format!(
        "select count(*) from minecraft_builds b
         left join minecraft_releases r on r.id = b.current_release_id
         left join (
            select build_id, count(*)::bigint as file_count, coalesce(sum(size_bytes), 0)::bigint as total_bytes
            from minecraft_build_files where kind = 'file' group by build_id
         ) f on f.build_id = b.id{where_sql}"
    );

    let mut rows = sqlx::query_as::<_, BuildRow>(&sql);
    for bind in &binds {
        rows = rows.bind(bind);
    }
    let items = rows.bind(per_page).bind(offset).fetch_all(db).await?;

    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    for bind in &binds {
        counter = counter.bind(bind);
    }
    let total = counter.fetch_one(db).await?;

    Ok((items, total))
}

pub async fn load(db: &PgPool, id: Uuid) -> ApiResult<BuildRow> {
    sqlx::query_as::<_, BuildRow>(&format!("{BUILD_SELECT} where b.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found("Сборка не найдена."))
}

/// Допустимые типы иконки и предел размера.
pub const ICON_MAX_BYTES: i64 = 2 * 1024 * 1024;

/// Определяет тип иконки по сигнатуре файла, не доверяя расширению.
pub fn sniff_image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some(("image/png", "png"));
    }
    if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        return Some(("image/jpeg", "jpg"));
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(("image/webp", "webp"));
    }
    None
}

/// Сохраняет иконку сборки: проверяет сигнатуру, размер и пишет объект.
pub async fn set_icon(
    state: &crate::state::AppState,
    build_id: Uuid,
    user_id: Uuid,
    bytes: Vec<u8>,
) -> ApiResult<()> {
    load(&state.db, build_id).await?;
    if bytes.is_empty() {
        return Err(ApiError::validation("Файл иконки пуст."));
    }
    if bytes.len() as i64 > ICON_MAX_BYTES {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "FILE_TOO_LARGE",
            "Иконка не должна превышать 2 МБ.",
        )
        .with_details(serde_json::json!({ "maxBytes": ICON_MAX_BYTES })));
    }
    let Some((content_type, _extension)) = sniff_image_type(&bytes) else {
        return Err(ApiError::validation(
            "Поддерживаются только PNG, JPEG и WEBP. Проверьте содержимое файла.",
        ));
    };

    let volume = crate::storage::engine::pick_volume(
        &state.db,
        None,
        bytes.len() as i64,
        crate::storage::engine::VolumeScope::Minecraft,
        Some(user_id),
    )
    .await?;
    let staging = crate::storage::engine::staging_path(
        &std::path::PathBuf::from(&volume.root_path),
        Uuid::new_v4(),
    );
    if let Some(parent) = staging.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| ApiError::internal("не удалось подготовить staging иконки"))?;
    }
    tokio::fs::write(&staging, &bytes)
        .await
        .map_err(|_| ApiError::internal("не удалось записать иконку"))?;

    let location = crate::storage::engine::commit_from_staging(
        &state.db,
        &volume,
        &staging,
        bytes.len() as i64,
        content_type,
    )
    .await?;
    let object_id: Uuid = sqlx::query_scalar(
        "select id from storage_objects where volume_id = $1 and object_key = $2",
    )
    .bind(location.volume_id)
    .bind(&location.key)
    .fetch_one(&state.db)
    .await?;

    let mut tx = state.db.begin().await?;
    let old: Option<Uuid> =
        sqlx::query_scalar("select icon_object_id from minecraft_builds where id = $1")
            .bind(build_id)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "update minecraft_builds set icon_object_id = $2, updated_at = now() where id = $1",
    )
    .bind(build_id)
    .bind(object_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "insert into storage_object_refs (object_id, owner_module, owner_id, display_name)
         values ($1, 'minecraft', $2, 'icon')
         on conflict (owner_module, owner_id) do update set object_id = excluded.object_id",
    )
    .bind(object_id)
    .bind(build_id)
    .execute(&mut *tx)
    .await?;
    if let Some(old_id) = old.filter(|old| *old != object_id) {
        sqlx::query(
            "update storage_objects set ref_count = greatest(ref_count - 1, 0) where id = $1",
        )
        .bind(old_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    crate::storage::objects::delete_zero_ref_objects(&state.db).await?;
    Ok(())
}

/// Удаляет иконку сборки, освобождая объект.
pub async fn remove_icon(db: &PgPool, build_id: Uuid) -> ApiResult<()> {
    load(db, build_id).await?;
    let old: Option<Uuid> =
        sqlx::query_scalar("select icon_object_id from minecraft_builds where id = $1")
            .bind(build_id)
            .fetch_one(db)
            .await?;
    let Some(old) = old else {
        return Ok(());
    };
    let mut tx = db.begin().await?;
    sqlx::query(
        "update minecraft_builds set icon_object_id = null, updated_at = now() where id = $1",
    )
    .bind(build_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "delete from storage_object_refs where owner_module = 'minecraft' and owner_id = $1",
    )
    .bind(build_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("update storage_objects set ref_count = greatest(ref_count - 1, 0) where id = $1")
        .bind(old)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::storage::objects::delete_zero_ref_objects(db).await?;
    Ok(())
}

/// Объект иконки, подготовленный к отдаче клиенту.
#[derive(Debug)]
pub struct IconObject {
    pub location: crate::storage::engine::ObjectLocation,
    pub size: i64,
    pub content_type: &'static str,
    pub sha256: String,
}

/// Загружает иконку для отдачи клиенту: путь объекта, проверенный тип и SHA.
pub async fn icon_object(db: &PgPool, build_id: Uuid) -> ApiResult<Option<IconObject>> {
    #[derive(sqlx::FromRow)]
    struct IconRow {
        object_key: Option<String>,
        volume_id: Option<Uuid>,
        root_path: Option<String>,
        size_bytes: Option<i64>,
        sha256: Option<String>,
    }
    let row: Option<IconRow> = sqlx::query_as(
        "select o.object_key, v.id as volume_id, v.root_path, o.size_bytes, o.sha256
         from minecraft_builds b
         left join storage_objects o
                on o.id = b.icon_object_id and o.state = 'committed'
         left join storage_volumes v on v.id = o.volume_id
         where b.id = $1",
    )
    .bind(build_id)
    .fetch_optional(db)
    .await?;
    let Some(IconRow {
        object_key: Some(key),
        volume_id: Some(volume_id),
        root_path: Some(root_path),
        size_bytes: Some(size),
        sha256: Some(sha256),
    }) = row
    else {
        return Ok(None);
    };

    // Тип определяется по содержимому: читаем первые байты файла.
    let root = std::path::PathBuf::from(&root_path);
    let location = crate::storage::engine::ObjectLocation {
        volume_id,
        absolute: crate::storage::engine::ObjectLocation::objects_root(&root).join(&key),
        key,
        root,
    };
    let mut file = crate::storage::engine::open_object(&location).await?;
    use tokio::io::AsyncReadExt;
    let mut head = [0u8; 12];
    let read = file
        .read(&mut head)
        .await
        .map_err(|_| ApiError::internal("не удалось прочитать иконку"))?;
    let content_type = sniff_image_type(&head[..read])
        .map(|(mime, _)| mime)
        .unwrap_or("application/octet-stream");
    Ok(Some(IconObject {
        location,
        size,
        content_type,
        sha256,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBuildRequest {
    pub name: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub minecraft_version: String,
    #[serde(default = "default_loader")]
    pub loader_type: String,
    #[serde(default)]
    pub loader_version: String,
    #[serde(default)]
    pub java_major: Option<i32>,
    #[serde(default)]
    pub java_min: Option<i32>,
    #[serde(default)]
    pub java_notes: String,
}

fn default_version() -> String {
    "1.0.0".to_owned()
}
fn default_category() -> String {
    "other".to_owned()
}
fn default_visibility() -> String {
    "private".to_owned()
}
fn default_loader() -> String {
    "vanilla".to_owned()
}

pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.to_lowercase().chars() {
        let mapped = match ch {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'е' => "e",
            'ё' => "e",
            'ж' => "zh",
            'з' => "z",
            'и' => "i",
            'й' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' => "h",
            'ц' => "ts",
            'ч' => "ch",
            'ш' => "sh",
            'щ' => "sch",
            'ъ' => "",
            'ы' => "y",
            'ь' => "",
            'э' => "e",
            'ю' => "yu",
            'я' => "ya",
            _ => "",
        };
        if !mapped.is_empty() {
            out.push_str(mapped);
            last_dash = false;
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        format!("build-{}", &Uuid::new_v4().to_string()[..8])
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn validate_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    if name.chars().count() < 2 || name.chars().count() > 96 {
        return Err(ApiError::validation(
            "Название сборки должно содержать от 2 до 96 символов.",
        ));
    }
    Ok(name.to_owned())
}

fn validate_visibility(raw: &str) -> ApiResult<String> {
    match raw.trim() {
        "public" => Ok("public".to_owned()),
        "private" => Ok("private".to_owned()),
        _ => Err(ApiError::validation(
            "Видимость должна быть public или private.",
        )),
    }
}

fn validate_java(value: i32, label: &str) -> ApiResult<i32> {
    if !(8..=30).contains(&value) {
        return Err(ApiError::validation(format!(
            "{label} должна быть в диапазоне 8..30."
        )));
    }
    Ok(value)
}

/// Не даёт сохранить вручную подобранную несовместимую версию загрузчика.
/// Если справочник пуст (источник ещё не синхронизирован), проверка
/// откладывается до запуска проверки сборки.
async fn ensure_loader_compatible(
    db: &PgPool,
    loader_type: &str,
    minecraft_version: &str,
    loader_version: &str,
) -> ApiResult<()> {
    if loader_type == "vanilla" || minecraft_version.is_empty() {
        return Ok(());
    }
    let known: Vec<String> = sqlx::query_scalar(
        "select distinct loader_version from minecraft_loader_versions
         where loader_type = $1 and minecraft_version = $2",
    )
    .bind(loader_type)
    .bind(minecraft_version)
    .fetch_all(db)
    .await?;
    if known.is_empty() || known.iter().any(|version| version == loader_version) {
        return Ok(());
    }
    Err(ApiError::validation(format!(
        "Версия загрузчика {loader_version} не совместима с Minecraft {minecraft_version}."
    )))
}

pub async fn create(
    db: &PgPool,
    actor_id: Uuid,
    request: CreateBuildRequest,
) -> ApiResult<BuildRow> {
    let name = validate_name(&request.name)?;
    let slug = match request
        .slug
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(raw) => validate_slug(raw)?,
        None => validate_slug(&slugify(&name))?,
    };
    let version = validation::validate_version(&request.version)?;
    let category = request.category.trim().to_ascii_lowercase();
    let visibility = validate_visibility(&request.visibility)?;
    let loader_type = validation::validate_loader(&request.loader_type)?;
    let loader_version = request.loader_version.trim().to_owned();
    if loader_type != "vanilla" && loader_version.is_empty() {
        return Err(ApiError::validation(
            "Укажите версию выбранного загрузчика.",
        ));
    }
    let minecraft_version = if request.minecraft_version.trim().is_empty() {
        String::new()
    } else {
        validation::validate_minecraft_version(&request.minecraft_version)?
    };
    ensure_loader_compatible(db, &loader_type, &minecraft_version, &loader_version).await?;
    let java_major = validate_java(request.java_major.unwrap_or(21), "Основная версия Java")?;
    let java_min = validate_java(
        request.java_min.unwrap_or(java_major),
        "Минимальная версия Java",
    )?;
    if java_min > java_major {
        return Err(ApiError::validation(
            "Минимальная версия Java не может быть выше основной.",
        ));
    }

    let exists: Option<Uuid> =
        sqlx::query_scalar("select id from minecraft_builds where slug = $1")
            .bind(&slug)
            .fetch_optional(db)
            .await?;
    if exists.is_some() {
        return Err(ApiError::conflict(
            "BUILD_SLUG_TAKEN",
            "Сборка с таким slug уже существует.",
        ));
    }

    let id: Uuid = sqlx::query_scalar(
        "insert into minecraft_builds
           (slug, name, description, version, category, visibility, minecraft_version,
            loader_type, loader_version, java_major, java_min, java_notes, created_by, owner_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
         returning id",
    )
    .bind(&slug)
    .bind(&name)
    .bind(request.description.trim())
    .bind(&version)
    .bind(&category)
    .bind(&visibility)
    .bind(&minecraft_version)
    .bind(&loader_type)
    .bind(&loader_version)
    .bind(java_major)
    .bind(java_min)
    .bind(request.java_notes.trim())
    .bind(actor_id)
    .fetch_one(db)
    .await?;

    load(db, id).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBuildRequest {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub visibility: Option<String>,
    pub minecraft_version: Option<String>,
    pub loader_type: Option<String>,
    pub loader_version: Option<String>,
    pub java_major: Option<i32>,
    pub java_min: Option<i32>,
    pub java_notes: Option<String>,
}

pub async fn update(db: &PgPool, id: Uuid, request: UpdateBuildRequest) -> ApiResult<BuildRow> {
    let current = load(db, id).await?;
    if current.archived_at.is_some() {
        return Err(ApiError::conflict(
            "BUILD_ARCHIVED",
            "Сборка в архиве. Восстановите её перед изменением.",
        ));
    }

    let name = match request.name.as_deref() {
        Some(value) => validate_name(value)?,
        None => current.name.clone(),
    };
    let slug = match request.slug.as_deref().filter(|v| !v.trim().is_empty()) {
        Some(value) => {
            let slug = validate_slug(value)?;
            if slug != current.slug {
                let taken: Option<Uuid> = sqlx::query_scalar(
                    "select id from minecraft_builds where slug = $1 and id <> $2",
                )
                .bind(&slug)
                .bind(id)
                .fetch_optional(db)
                .await?;
                if taken.is_some() {
                    return Err(ApiError::conflict(
                        "BUILD_SLUG_TAKEN",
                        "Сборка с таким slug уже существует.",
                    ));
                }
            }
            slug
        }
        None => current.slug.clone(),
    };
    let version = match request.version.as_deref() {
        Some(value) => validation::validate_version(value)?,
        None => current.version.clone(),
    };
    let category = match request.category.as_deref() {
        Some(value) => value.trim().to_ascii_lowercase(),
        None => current.category.clone(),
    };
    let visibility = match request.visibility.as_deref() {
        Some(value) => validate_visibility(value)?,
        None => current.visibility.clone(),
    };
    let loader_type = match request.loader_type.as_deref() {
        Some(value) => validation::validate_loader(value)?,
        None => current.loader_type.clone(),
    };
    let loader_version = request
        .loader_version
        .as_deref()
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|| current.loader_version.clone());
    if loader_type != "vanilla" && loader_version.is_empty() {
        return Err(ApiError::validation(
            "Укажите версию выбранного загрузчика.",
        ));
    }
    let minecraft_version = match request.minecraft_version.as_deref() {
        Some(value) if value.trim().is_empty() => String::new(),
        Some(value) => validation::validate_minecraft_version(value)?,
        None => current.minecraft_version.clone(),
    };
    if request.loader_version.is_some()
        || request.loader_type.is_some()
        || request.minecraft_version.is_some()
    {
        ensure_loader_compatible(db, &loader_type, &minecraft_version, &loader_version).await?;
    }
    let java_major = validate_java(
        request.java_major.unwrap_or(current.java_major),
        "Основная версия Java",
    )?;
    let java_min = validate_java(
        request.java_min.unwrap_or(current.java_min),
        "Минимальная версия Java",
    )?;
    if java_min > java_major {
        return Err(ApiError::validation(
            "Минимальная версия Java не может быть выше основной.",
        ));
    }

    sqlx::query(
        "update minecraft_builds
         set name = $2, slug = $3, description = $4, version = $5, category = $6,
             visibility = $7, minecraft_version = $8, loader_type = $9, loader_version = $10,
             java_major = $11, java_min = $12, java_notes = $13, updated_at = now()
         where id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(&slug)
    .bind(
        request
            .description
            .as_deref()
            .unwrap_or(&current.description)
            .trim(),
    )
    .bind(&version)
    .bind(&category)
    .bind(&visibility)
    .bind(&minecraft_version)
    .bind(&loader_type)
    .bind(&loader_version)
    .bind(java_major)
    .bind(java_min)
    .bind(
        request
            .java_notes
            .as_deref()
            .unwrap_or(&current.java_notes)
            .trim(),
    )
    .execute(db)
    .await?;

    load(db, id).await
}

/// Клонирует сборку вместе с файлами (объекты переиспользуются через ссылки).
pub async fn clone_build(db: &PgPool, actor_id: Uuid, id: Uuid) -> ApiResult<BuildRow> {
    let source = load(db, id).await?;
    let base_slug = format!("{}-copy", source.slug);
    let mut slug = base_slug.chars().take(56).collect::<String>();
    for attempt in 2..=50 {
        let taken: Option<Uuid> =
            sqlx::query_scalar("select id from minecraft_builds where slug = $1")
                .bind(&slug)
                .fetch_optional(db)
                .await?;
        if taken.is_none() {
            break;
        }
        slug = format!("{}-{attempt}", base_slug)
            .chars()
            .take(64)
            .collect::<String>();
    }

    let mut tx = db.begin().await?;
    let new_id: Uuid = sqlx::query_scalar(
        "insert into minecraft_builds
           (slug, name, description, version, category, visibility, minecraft_version,
            loader_type, loader_version, java_major, java_min, java_notes, created_by, owner_id)
         values ($1, $2, $3, $4, $5, 'private', $6, $7, $8, $9, $10, $11, $12, $12)
         returning id",
    )
    .bind(&slug)
    .bind(format!("{} (копия)", source.name))
    .bind(&source.description)
    .bind(&source.version)
    .bind(&source.category)
    .bind(&source.minecraft_version)
    .bind(&source.loader_type)
    .bind(&source.loader_version)
    .bind(source.java_major)
    .bind(source.java_min)
    .bind(&source.java_notes)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "insert into minecraft_build_files
           (build_id, kind, path, category, side, required, enabled, description,
            object_id, size_bytes, sha256)
         select $1, kind, path, category, side, required, enabled, description,
                object_id, size_bytes, sha256
         from minecraft_build_files where build_id = $2",
    )
    .bind(new_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    // Новые записи файлов увеличивают счётчик ссылок на объекты.
    sqlx::query(
        "update storage_objects o
         set ref_count = ref_count + sub.cnt
         from (
            select object_id, count(*) as cnt
            from minecraft_build_files
            where build_id = $1 and object_id is not null and kind = 'file'
            group by object_id
         ) sub
         where o.id = sub.object_id",
    )
    .bind(new_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "insert into minecraft_release_history (build_id, release_id, event, actor_id, actor_username, details)
         values ($1, null, 'build.cloned', $2, '', jsonb_build_object('sourceBuildId', $3::text))",
    )
    .bind(new_id)
    .bind(actor_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    load(db, new_id).await
}

pub async fn set_archived(db: &PgPool, id: Uuid, archived: bool) -> ApiResult<BuildRow> {
    let current = load(db, id).await?;
    if archived {
        sqlx::query(
            "update minecraft_builds
             set archived_at = now(), status = 'archived', updated_at = now()
             where id = $1",
        )
        .bind(id)
        .execute(db)
        .await?;
    } else {
        if current.status != "archived" {
            return Ok(current);
        }
        let status = if current.current_release_id.is_some() {
            "published"
        } else {
            "draft"
        };
        sqlx::query(
            "update minecraft_builds
             set archived_at = null, status = $2, updated_at = now()
             where id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(db)
        .await?;
    }
    load(db, id).await
}

/// Удаляет сборку. Запрещено, если на файлы ссылаются опубликованные релизы
/// другой сборки (в текущей модели релизы принадлежат сборке и удаляются вместе с ней).
pub async fn delete(db: &PgPool, id: Uuid) -> ApiResult<()> {
    let build = load(db, id).await?;
    let published: i64 = sqlx::query_scalar(
        "select count(*) from minecraft_releases where build_id = $1 and status in ('published', 'deprecated')",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    if published > 0 && build.status == "published" {
        return Err(ApiError::conflict(
            "BUILD_HAS_PUBLISHED_RELEASES",
            "Сначала снимите публикацию релизов или архивируйте сборку.",
        ));
    }

    let mut tx = db.begin().await?;
    // Освобождаем ссылки на объекты: снимки релизов держат собственные ссылки,
    // поэтому учитываются обе таблицы.
    sqlx::query(
        "update storage_objects o
         set ref_count = greatest(o.ref_count - sub.cnt, 0)
         from (
            select object_id, sum(cnt) as cnt
            from (
                select object_id, count(*) as cnt
                from minecraft_build_files
                where build_id = $1 and object_id is not null and kind = 'file'
                group by object_id
                union all
                select rf.object_id, count(*) as cnt
                from minecraft_release_files rf
                join minecraft_releases r on r.id = rf.release_id
                where r.build_id = $1 and rf.object_id is not null
                group by rf.object_id
            ) merged
            group by object_id
         ) sub
         where o.id = sub.object_id",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "delete from storage_object_refs
         where owner_module = 'minecraft'
           and owner_id in (select id from minecraft_build_files where build_id = $1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("delete from minecraft_builds where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    crate::storage::objects::delete_zero_ref_objects(db).await?;
    Ok(())
}
