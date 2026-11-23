//! Файлы внутри сборки: дерево путей, создание папок, перемещение, удаление.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::validation::{self, path_key};
use crate::core::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub id: Uuid,
    pub build_id: Uuid,
    pub kind: String,
    pub path: String,
    pub category: String,
    pub side: String,
    pub required: bool,
    pub enabled: bool,
    pub description: String,
    pub object_id: Option<Uuid>,
    pub size_bytes: i64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const FILE_SELECT: &str = "select id, build_id, kind, path, category, side, required, enabled,
        description, object_id, size_bytes, sha256, created_at, updated_at
     from minecraft_build_files";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub side: Option<String>,
    /// Прямые дочерние записи указанной папки (пустая строка — корень).
    pub folder: Option<String>,
}

pub struct FileStats {
    pub files: i64,
    pub folders: i64,
    pub total_bytes: i64,
}

pub async fn stats(db: &PgPool, build_id: Uuid) -> ApiResult<FileStats> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "select count(*) filter (where kind = 'file')::bigint,
                count(*) filter (where kind = 'dir')::bigint,
                coalesce(sum(size_bytes) filter (where kind = 'file'), 0)::bigint
         from minecraft_build_files where build_id = $1",
    )
    .bind(build_id)
    .fetch_one(db)
    .await?;
    Ok(FileStats {
        files: row.0,
        folders: row.1,
        total_bytes: row.2,
    })
}

pub async fn list(db: &PgPool, build_id: Uuid, query: &ListFilesQuery) -> ApiResult<Vec<FileRow>> {
    let mut conditions = vec!["build_id = $1".to_string()];
    let mut binds: Vec<String> = Vec::new();
    if let Some(q) = query.q.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        binds.push(format!("%{}%", q.replace('%', "\\%").replace('_', "\\_")));
        conditions.push(format!("path ilike ${} escape '\\'", binds.len() + 1));
    }
    if let Some(category) = query
        .category
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        binds.push(category.to_ascii_lowercase());
        conditions.push(format!("category = ${}", binds.len() + 1));
    }
    if let Some(side) = query
        .side
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        binds.push(side.to_ascii_lowercase());
        conditions.push(format!("side = ${}", binds.len() + 1));
    }

    let sql = format!(
        "{FILE_SELECT} where {} order by kind desc, lower(path)",
        conditions.join(" and ")
    );
    let mut rows = sqlx::query_as::<_, FileRow>(&sql).bind(build_id);
    for bind in &binds {
        rows = rows.bind(bind);
    }
    let mut items = rows.fetch_all(db).await?;
    if let Some(folder) = query.folder.as_deref().map(str::trim) {
        let folder = if folder.is_empty() {
            String::new()
        } else {
            validation::normalize_path(folder)?
        };
        items.retain(|row| {
            let parent = row
                .path
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("");
            parent.eq_ignore_ascii_case(&folder)
        });
    }
    Ok(items)
}

/// Убеждается, что путь свободен (с учётом регистра и Unicode).
async fn ensure_path_free(
    db: &PgPool,
    build_id: Uuid,
    path: &str,
    exclude: Option<Uuid>,
) -> ApiResult<()> {
    let key = path_key(path);
    let existing: Vec<(Uuid, String, String)> =
        sqlx::query_as("select id, path, kind from minecraft_build_files where build_id = $1")
            .bind(build_id)
            .fetch_all(db)
            .await?;
    for (id, existing_path, kind) in existing {
        if Some(id) == exclude {
            continue;
        }
        let existing_key = path_key(&existing_path);
        if existing_key == key {
            return Err(ApiError::conflict(
                "BUILD_PATH_CONFLICT",
                "Файл с таким путём уже существует в сборке.",
            )
            .with_details(json!({ "existingFileId": id, "path": existing_path })));
        }
        // Родительская папка допустима, а вот файл в роли родителя — нет.
        if kind != "dir" && key.starts_with(&format!("{existing_key}/")) {
            return Err(ApiError::conflict(
                "BUILD_PATH_CONFLICT",
                "Путь конфликтует с существующим файлом.",
            )
            .with_details(json!({ "existingFileId": id, "path": existing_path })));
        }
        // Нельзя превращать папку с содержимым в файл.
        if kind == "dir" && existing_key.starts_with(&format!("{key}/")) {
            return Err(ApiError::conflict(
                "BUILD_PATH_CONFLICT",
                "Путь совпадает с существующей папкой.",
            )
            .with_details(json!({ "existingFileId": id, "path": existing_path })));
        }
    }
    Ok(())
}

fn infer_category(path: &str) -> String {
    validation::canonical_category(path)
        .or_else(|| validation::legacy_category(path))
        .map(str::to_owned)
        .unwrap_or_else(|| validation::infer_extension_category(path))
}

pub async fn create_folder(db: &PgPool, build_id: Uuid, raw_path: &str) -> ApiResult<FileRow> {
    let path = validation::check_build_path(raw_path, "dir")?;
    ensure_path_free(db, build_id, &path, None).await?;
    let category = infer_category(&path);
    let mut tx = db.begin().await?;
    // Вложенная папка config создаёт и все родительские записи, чтобы дерево
    // было одинаково навигируемым до и после загрузки первого файла.
    create_parents_tx(&mut tx, build_id, &path).await?;
    let row = sqlx::query_as::<_, FileRow>(
        "insert into minecraft_build_files (build_id, kind, path, category)
         values ($1, 'dir', $2, $3)
         returning id, build_id, kind, path, category, side, required, enabled,
                   description, object_id, size_bytes, sha256, created_at, updated_at",
    )
    .bind(build_id)
    .bind(&path)
    .bind(&category)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFileRequest {
    pub path: Option<String>,
    pub category: Option<String>,
    pub side: Option<String>,
    pub required: Option<bool>,
    pub enabled: Option<bool>,
    pub description: Option<String>,
}

pub async fn update_file(
    db: &PgPool,
    build_id: Uuid,
    file_id: Uuid,
    request: UpdateFileRequest,
) -> ApiResult<FileRow> {
    let current = load_file(db, build_id, file_id).await?;
    let mut child_updates: Vec<(Uuid, String)> = Vec::new();

    let path = match request.path.as_deref() {
        Some(value) => {
            let next = validation::check_build_path(value, &current.kind)?;
            let changed = path_key(&next) != path_key(&current.path);
            if changed && validation::is_fixed_build_root(&current.path) {
                return Err(ApiError::conflict(
                    "BUILD_ROOT_IMMUTABLE",
                    "Корневой узел канонического дерева нельзя переименовать или переместить.",
                ));
            }
            if changed {
                if current.kind == "dir"
                    && path_key(&next).starts_with(&format!("{}/", path_key(&current.path)))
                {
                    return Err(ApiError::validation(
                        "Нельзя переместить папку внутрь самой себя.",
                    ));
                }
                ensure_path_free(db, build_id, &next, Some(file_id)).await?;
                if current.kind == "dir" {
                    let prefix_old = format!("{}/", current.path);
                    let prefix_new = format!("{}/", next);
                    let children: Vec<(Uuid, String, String)> = sqlx::query_as(
                        "select id, path, kind from minecraft_build_files
                         where build_id = $1 and id <> $2 and lower(path) like lower($3) || '%'",
                    )
                    .bind(build_id)
                    .bind(file_id)
                    .bind(&prefix_old)
                    .fetch_all(db)
                    .await?;
                    // Перемещение папки не должно создавать невалидные пути у детей
                    // (например, config/sub → mods/sub с .toml внутри).
                    for (child_id, child_path, child_kind) in children {
                        let updated = format!("{prefix_new}{}", &child_path[prefix_old.len()..]);
                        validation::check_build_path(&updated, &child_kind)?;
                        child_updates.push((child_id, updated));
                    }
                }
            }
            next
        }
        None => current.path.clone(),
    };
    let category = validation::resolve_category(&path, request.category.as_deref())?;
    let side = match request.side.as_deref() {
        Some(value) => validation::validate_side(value)?,
        None => current.side.clone(),
    };

    // Дети и сама запись меняются одной транзакцией: при сбое путь папки и
    // пути вложенных файлов не расходятся.
    let mut tx = db.begin().await?;
    for (child_id, updated) in child_updates {
        sqlx::query("update minecraft_build_files set path = $2, updated_at = now() where id = $1")
            .bind(child_id)
            .bind(&updated)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "update minecraft_build_files
         set path = $3, category = $4, side = $5, required = $6, enabled = $7,
             description = $8, updated_at = now()
         where id = $1 and build_id = $2",
    )
    .bind(file_id)
    .bind(build_id)
    .bind(&path)
    .bind(&category)
    .bind(&side)
    .bind(request.required.unwrap_or(current.required))
    .bind(request.enabled.unwrap_or(current.enabled))
    .bind(
        request
            .description
            .as_deref()
            .unwrap_or(&current.description)
            .trim(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    load_file(db, build_id, file_id).await
}

pub async fn load_file(db: &PgPool, build_id: Uuid, file_id: Uuid) -> ApiResult<FileRow> {
    sqlx::query_as::<_, FileRow>(&format!("{FILE_SELECT} where id = $1 and build_id = $2"))
        .bind(file_id)
        .bind(build_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found("Файл сборки не найден."))
}

/// Удаляет файл или папку (с вложенным содержимым), освобождая объекты.
pub async fn delete_file(db: &PgPool, build_id: Uuid, file_id: Uuid) -> ApiResult<i64> {
    let target = load_file(db, build_id, file_id).await?;
    if target.kind == "dir" && validation::is_fixed_build_folder(&target.path) {
        return Err(ApiError::conflict(
            "BUILD_ROOT_IMMUTABLE",
            "Корневой узел канонического дерева нельзя удалить.",
        ));
    }
    let (ids, objects): (Vec<Uuid>, Vec<(Uuid, i64)>) = if target.kind == "dir" {
        let prefix = format!("{}/", target.path);
        let rows: Vec<(Uuid, Option<Uuid>, i64)> = sqlx::query_as(
            "select id, object_id, size_bytes from minecraft_build_files
             where build_id = $1 and (id = $2 or lower(path) like lower($3) || '%')",
        )
        .bind(build_id)
        .bind(file_id)
        .bind(&prefix)
        .fetch_all(db)
        .await?;
        let mut ids = Vec::with_capacity(rows.len());
        let mut objects = Vec::new();
        for (id, object_id, size) in rows {
            ids.push(id);
            if let Some(object_id) = object_id {
                objects.push((object_id, size));
            }
        }
        (ids, objects)
    } else {
        (
            vec![file_id],
            target
                .object_id
                .map(|object_id| (object_id, target.size_bytes))
                .into_iter()
                .collect(),
        )
    };

    let mut tx = db.begin().await?;
    // Удаляем ссылки на освобождаемые файлы (владелец ссылки — запись файла).
    if !ids.is_empty() {
        sqlx::query(
            "delete from storage_object_refs
             where owner_module = 'minecraft' and owner_id = any($1)",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("delete from minecraft_build_files where id = any($1)")
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    for (object_id, _) in &objects {
        sqlx::query(
            "update storage_objects set ref_count = greatest(ref_count - 1, 0) where id = $1",
        )
        .bind(object_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    crate::storage::objects::delete_zero_ref_objects(db).await?;
    Ok(ids.len() as i64)
}

/// Регистрирует загруженный объект как файл сборки.
#[allow(clippy::too_many_arguments)]
pub async fn register_file(
    db: &PgPool,
    build_id: Uuid,
    object_id: Uuid,
    path: &str,
    size_bytes: i64,
    sha256: &str,
    category: &str,
    side: &str,
    required: bool,
    description: &str,
) -> ApiResult<FileRow> {
    let normalized = validation::check_build_path(path, "file")?;
    let category = validation::resolve_category(&normalized, Some(category))?;
    let side = validation::validate_side(side)?;

    let previous: Option<Uuid> = sqlx::query_scalar(
        "select id from minecraft_build_files where build_id = $1 and lower(path) = lower($2)",
    )
    .bind(build_id)
    .bind(&normalized)
    .fetch_optional(db)
    .await?;

    let mut tx = db.begin().await?;
    // Все родительские каталоги существуют как записи дерева.
    create_parents_tx(&mut tx, build_id, &normalized).await?;

    let file_id: Uuid = if let Some(existing_id) = previous {
        let old_object: Option<Uuid> =
            sqlx::query_scalar("select object_id from minecraft_build_files where id = $1")
                .bind(existing_id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query(
            "update minecraft_build_files
             set kind = 'file', object_id = $2, size_bytes = $3, sha256 = $4,
                 category = $5, side = $6, required = $7, description = $8,
                 enabled = true, path = $9, updated_at = now()
             where id = $1",
        )
        .bind(existing_id)
        .bind(object_id)
        .bind(size_bytes)
        .bind(sha256)
        .bind(&category)
        .bind(&side)
        .bind(required)
        .bind(description)
        .bind(&normalized)
        .execute(&mut *tx)
        .await?;
        if let Some(old) = old_object {
            if old != object_id {
                sqlx::query(
                    "update storage_objects set ref_count = greatest(ref_count - 1, 0) where id = $1",
                )
                .bind(old)
                .execute(&mut *tx)
                .await?;
            }
        }
        existing_id
    } else {
        sqlx::query_scalar(
            "insert into minecraft_build_files
               (build_id, kind, path, category, side, required, description,
                object_id, size_bytes, sha256)
             values ($1, 'file', $2, $3, $4, $5, $6, $7, $8, $9)
             returning id",
        )
        .bind(build_id)
        .bind(&normalized)
        .bind(&category)
        .bind(&side)
        .bind(required)
        .bind(description)
        .bind(object_id)
        .bind(size_bytes)
        .bind(sha256)
        .fetch_one(&mut *tx)
        .await?
    };

    // Ссылка принадлежит конкретному файлу сборки, а не всей сборке:
    // у одной сборки может быть много объектов.
    sqlx::query(
        "insert into storage_object_refs (object_id, owner_module, owner_id, display_name)
         values ($1, 'minecraft', $2, $3)
         on conflict (owner_module, owner_id) do update
         set object_id = excluded.object_id, display_name = excluded.display_name",
    )
    .bind(object_id)
    .bind(file_id)
    .bind(&normalized)
    .execute(&mut *tx)
    .await?;

    sqlx::query("update minecraft_builds set updated_at = now() where id = $1")
        .bind(build_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    crate::storage::objects::delete_zero_ref_objects(db).await?;

    load_file(db, build_id, file_id).await
}

async fn create_parents_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    build_id: Uuid,
    path: &str,
) -> ApiResult<()> {
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 {
        return Ok(());
    }
    let existing: Vec<String> =
        sqlx::query_scalar("select lower(path) from minecraft_build_files where build_id = $1")
            .bind(build_id)
            .fetch_all(&mut **tx)
            .await?;
    let mut current = String::new();
    for segment in &segments[..segments.len() - 1] {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        if existing.contains(&current.to_lowercase()) {
            continue;
        }
        let category = infer_category(&current);
        sqlx::query(
            "insert into minecraft_build_files (build_id, kind, path, category)
             values ($1, 'dir', $2, $3)
             on conflict do nothing",
        )
        .bind(build_id)
        .bind(&current)
        .bind(&category)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Проверяет путь без записи в БД (используется перед загрузкой).
/// Проверяет, что путь принадлежит файлу, и возвращает его размер.
pub async fn existing_file_size(db: &PgPool, build_id: Uuid, path: &str) -> ApiResult<Option<i64>> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "select kind, size_bytes from minecraft_build_files
         where build_id = $1 and lower(path) = lower($2)",
    )
    .bind(build_id)
    .bind(path)
    .fetch_optional(db)
    .await?;
    match row {
        Some((kind, size)) if kind == "file" => Ok(Some(size)),
        Some(_) => Err(ApiError::conflict(
            "BUILD_PATH_IS_FOLDER",
            "По этому пути находится папка, а не файл.",
        )),
        None => Err(ApiError::not_found("Файл для замены не найден.")),
    }
}

pub async fn path_available(db: &PgPool, build_id: Uuid, path: &str) -> ApiResult<String> {
    let normalized = validation::check_build_path(path, "file")?;
    ensure_path_free(db, build_id, &normalized, None).await?;
    Ok(normalized)
}
