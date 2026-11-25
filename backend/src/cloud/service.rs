//! Логика записей Cloud Storage: папки, файлы, корзина, избранное.

use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::models::{CloudEntry, descendant_ids};
use crate::core::error::{ApiError, ApiResult};

pub const NAME_MAX: usize = 128;

/// Бюджет полного виртуального пути в UTF-16 code units.
///
/// Windows без `LongPathsEnabled` ограничивает путь 260 символами, включая
/// корень тома и служебные каталоги. Оставляем запас для `.nexus/exports`,
/// временных файлов и распаковки: 240 единиц на путь внутри Cloud.
pub const PATH_LENGTH_MAX: usize = 240;

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    // COM1–COM9 и LPT1–LPT9 (не COM0/COM10).
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && (b'1'..=b'9').contains(&bytes[3])
}

pub fn validate_name(raw: &str) -> ApiResult<String> {
    // Завершающие точка/пробел запрещены Windows; не «исправляем» их молча.
    if raw.ends_with(' ') || raw.ends_with('.') {
        return Err(ApiError::validation(
            "Имя не может заканчиваться точкой или пробелом (ограничение Windows).",
        ));
    }
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ApiError::validation(
            "Имя должно содержать от 1 до 128 символов.",
        ));
    }
    if name == "." || name == ".." {
        return Err(ApiError::validation("Имя не может быть служебным."));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::validation(
            "Имя не может содержать управляющие символы.",
        ));
    }
    // `:` также запрещает alternate data streams вида `file.txt:payload`.
    if name
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err(ApiError::validation(
            "Имя содержит символ, недопустимый в Windows: < > : \" / \\ | ? *.",
        ));
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err(ApiError::validation(
            "Имя не может заканчиваться точкой или пробелом (ограничение Windows).",
        ));
    }
    if is_reserved_windows_name(name) {
        return Err(ApiError::validation(
            "Имя совпадает с зарезервированным именем Windows (CON, PRN, AUX, NUL, COM1–9, LPT1–9).",
        ));
    }
    Ok(name.to_owned())
}

fn path_too_long(actual: usize) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "PATH_TOO_LONG",
        "Полная длина пути превышает безопасный лимит Windows.",
    )
    .with_details(serde_json::json!({
        "maxLength": PATH_LENGTH_MAX,
        "actualLength": actual,
    }))
}

fn ensure_path_units(parent_units: usize, name: &str) -> ApiResult<usize> {
    let actual = if parent_units == 0 {
        utf16_len(name)
    } else {
        parent_units + 1 + utf16_len(name)
    };
    if actual > PATH_LENGTH_MAX {
        return Err(path_too_long(actual));
    }
    Ok(actual)
}

/// Длина пути родительской папки в UTF-16 code units (0 для корня).
async fn parent_path_units(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
) -> ApiResult<usize> {
    let Some(parent_id) = parent_id else {
        return Ok(0);
    };
    let names: Vec<String> = sqlx::query_scalar(
        "with recursive chain as (
             select id, parent_id, name, 0 as depth
             from cloud_entries where id = $1 and owner_id = $2
             union all
             select e.id, e.parent_id, e.name, c.depth + 1
             from cloud_entries e join chain c on e.id = c.parent_id
         )
         select name from chain order by depth desc",
    )
    .bind(parent_id)
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    let units: usize = names.iter().map(|name| utf16_len(name)).sum();
    Ok(units + names.len().saturating_sub(1))
}

/// Проверяет полный путь `parent/name` до создания или переименования.
pub async fn ensure_path_budget(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    name: &str,
) -> ApiResult<()> {
    let parent_units = parent_path_units(db, owner_id, parent_id).await?;
    ensure_path_units(parent_units, name).map(|_| ())
}

/// Понятная ошибка при нарушении индекса `lower(name)` в гонке запросов.
pub fn name_conflict_from_sqlx(error: sqlx::Error) -> ApiError {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.code().as_deref() == Some("23505")
            && database_error.constraint() == Some("cloud_entries_name_uq")
        {
            return ApiError::conflict(
                "FILE_NAME_CONFLICT",
                "Запись с таким именем уже существует в этой папке (имена сравниваются без учёта регистра).",
            );
        }
    }
    ApiError::from(error)
}

pub async fn load_entry(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<CloudEntry> {
    sqlx::query_as::<_, CloudEntry>(
        "select e.id, e.owner_id, e.parent_id, e.name, e.entry_type, e.size_bytes,
                e.content_type,
                coalesce(f.entry_id is not null, false) as starred,
                e.created_at, e.updated_at, e.trashed_at, null::text as owner_name
         from cloud_entries e
         left join cloud_favorites f on f.entry_id = e.id and f.user_id = e.owner_id
         where e.id = $1 and e.owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Файл или папка не найдены."))
}

pub async fn entry_name_taken(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    name: &str,
) -> ApiResult<Option<CloudEntry>> {
    let existing = sqlx::query_as::<_, CloudEntry>(
        "select id, owner_id, parent_id, name, entry_type, size_bytes, content_type,
                false as starred, created_at, updated_at, trashed_at, null::text as owner_name
         from cloud_entries
         where owner_id = $1
           and parent_id is not distinct from $2
           and lower(name) = lower($3)
           and trashed_at is null",
    )
    .bind(owner_id)
    .bind(parent_id)
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(existing)
}

/// Создаёт цепочку папок за один запрос (для загрузки целых папок).
///
/// Все папки создаются или переиспользуются атомарно: либо вся цепочка
/// существует, либо не создаётся ничего. Возвращает id последней папки.
pub async fn ensure_folder_path(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    segments: &[String],
) -> ApiResult<Uuid> {
    if segments.len() > 200 {
        return Err(ApiError::validation(
            "Слишком глубокая вложенность папок (максимум 200 уровней).",
        ));
    }
    let mut current = parent_id;
    if let Some(parent) = current {
        let entry = load_entry(db, owner_id, parent).await?;
        if entry.entry_type != "folder" || entry.trashed_at.is_some() {
            return Err(ApiError::validation("Целевая папка недоступна."));
        }
    }
    let mut current_units = parent_path_units(db, owner_id, current).await?;

    for segment in segments {
        let name = validate_name(segment)?;
        current_units = ensure_path_units(current_units, &name)?;
        if let Some(existing) = entry_name_taken(db, owner_id, current, &name).await? {
            if existing.entry_type != "folder" {
                return Err(
                    ApiError::conflict("FILE_NAME_CONFLICT", "Имя папки занято файлом.")
                        .with_details(serde_json::json!({ "existingFileId": existing.id })),
                );
            }
            current = Some(existing.id);
            continue;
        }
        let created = sqlx::query_scalar::<_, Uuid>(
            "insert into cloud_entries (owner_id, parent_id, name, entry_type)
             values ($1, $2, $3, 'folder')
             on conflict do nothing
             returning id",
        )
        .bind(owner_id)
        .bind(current)
        .bind(&name)
        .fetch_optional(db)
        .await?;
        current = match created {
            Some(id) => Some(id),
            None => {
                // Параллельный запрос создал папку — используем её.
                entry_name_taken(db, owner_id, current, &name)
                    .await?
                    .filter(|entry| entry.entry_type == "folder")
                    .map(|entry| entry.id)
            }
        };
    }

    current.ok_or_else(|| ApiError::validation("Путь папки не задан."))
}

/// Пакетное создание папок: до 200 штук за один запрос, атомарно.
pub async fn create_folders_batch(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    names: Vec<String>,
) -> ApiResult<Vec<CloudEntry>> {
    if names.is_empty() || names.len() > 200 {
        return Err(ApiError::validation(
            "За один запрос можно создать от 1 до 200 папок.",
        ));
    }
    if let Some(parent) = parent_id {
        let entry = load_entry(db, owner_id, parent).await?;
        if entry.entry_type != "folder" || entry.trashed_at.is_some() {
            return Err(ApiError::validation("Родительская папка недоступна."));
        }
    }

    let parent_units = parent_path_units(db, owner_id, parent_id).await?;
    let mut validated = Vec::with_capacity(names.len());
    for raw in &names {
        let name = validate_name(raw)?;
        ensure_path_units(parent_units, &name)?;
        validated.push(name);
    }

    let mut tx = db.begin().await?;
    let mut out = Vec::with_capacity(validated.len());
    for name in validated {
        let entry = sqlx::query_as::<_, CloudEntry>(
            "insert into cloud_entries (owner_id, parent_id, name, entry_type)
             values ($1, $2, $3, 'folder')
             on conflict do nothing
             returning id, owner_id, parent_id, name, entry_type, size_bytes, content_type,
                       false as starred, created_at, updated_at, trashed_at, null::text as owner_name",
        )
        .bind(owner_id)
        .bind(parent_id)
        .bind(&name)
        .fetch_optional(&mut *tx)
        .await?;
        match entry {
            Some(entry) => out.push(entry),
            // Уже существует — переиспользуем.
            None => {
                let existing = sqlx::query_as::<_, CloudEntry>(
                    "select id, owner_id, parent_id, name, entry_type, size_bytes, content_type,
                            false as starred, created_at, updated_at, trashed_at, null::text as owner_name
                     from cloud_entries
                     where owner_id = $1 and parent_id is not distinct from $2 and lower(name) = lower($3)
                       and trashed_at is null",
                )
                .bind(owner_id)
                .bind(parent_id)
                .bind(&name)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some(existing) = existing {
                    out.push(existing);
                } else {
                    tx.rollback().await.ok();
                    return Err(ApiError::conflict(
                        "FILE_NAME_CONFLICT",
                        "Не удалось создать папку: имя занято.",
                    ));
                }
            }
        }
    }
    tx.commit().await?;
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
}

pub async fn create_folder(
    db: &PgPool,
    owner_id: Uuid,
    request: CreateFolderRequest,
) -> ApiResult<CloudEntry> {
    let name = validate_name(&request.name)?;
    let parent_id = request.parent_id;
    if let Some(parent) = parent_id {
        let parent_entry = load_entry(db, owner_id, parent).await?;
        if parent_entry.entry_type != "folder" {
            return Err(ApiError::validation(
                "Родительская запись не является папкой.",
            ));
        }
        if parent_entry.trashed_at.is_some() {
            return Err(ApiError::validation(
                "Родительская папка находится в корзине.",
            ));
        }
    }
    ensure_path_budget(db, owner_id, parent_id, &name).await?;
    if entry_name_taken(db, owner_id, parent_id, &name)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "FILE_NAME_CONFLICT",
            "Запись с таким именем уже существует в этой папке.",
        ));
    }
    let entry = sqlx::query_as::<_, CloudEntry>(
        "insert into cloud_entries (owner_id, parent_id, name, entry_type)
         values ($1, $2, $3, 'folder')
         returning id, owner_id, parent_id, name, entry_type, size_bytes, content_type,
                   false as starred, created_at, updated_at, trashed_at, null::text as owner_name",
    )
    .bind(owner_id)
    .bind(parent_id)
    .bind(&name)
    .fetch_one(db)
    .await
    .map_err(name_conflict_from_sqlx)?;
    Ok(entry)
}

/// Внутренняя функция: создаёт запись файла после фиксации объекта.
pub async fn create_file_entry(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    name: &str,
    size_bytes: i64,
    content_type: &str,
) -> ApiResult<CloudEntry> {
    ensure_path_budget(db, owner_id, parent_id, name).await?;
    let entry = sqlx::query_as::<_, CloudEntry>(
        "insert into cloud_entries (owner_id, parent_id, name, entry_type, size_bytes, content_type)
         values ($1, $2, $3, 'file', $4, $5)
         returning id, owner_id, parent_id, name, entry_type, size_bytes, content_type,
                   false as starred, created_at, updated_at, trashed_at, null::text as owner_name",
    )
    .bind(owner_id)
    .bind(parent_id)
    .bind(name)
    .bind(size_bytes)
    .bind(content_type)
    .fetch_one(db)
    .await
    .map_err(name_conflict_from_sqlx)?;
    Ok(entry)
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEntryRequest {
    pub name: Option<String>,
    /// `None` — не менять родителя; `Some(None)` — переместить в корень;
    /// `Some(Some(id))` — переместить в указанную папку.
    #[serde(default, deserialize_with = "deserialize_optional_parent")]
    pub parent_id: Option<Option<Uuid>>,
    pub starred: Option<bool>,
    /// Необязательная стратегия для клиентов, разрешающих конфликт имени.
    /// `replace` включает проверку `Idempotency-Key` в маршруте.
    #[serde(default)]
    pub strategy: Option<String>,
}

/// Отличает отсутствие поля (`None`) от явного `null` (`Some(None)`).
fn deserialize_optional_parent<'de, D>(deserializer: D) -> Result<Option<Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Uuid>::deserialize(deserializer).map(Some)
}

async fn is_descendant(db: &PgPool, ancestor_id: Uuid, candidate_id: Uuid) -> ApiResult<bool> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "with recursive tree as (
             select id from cloud_entries where id = $1
             union all
             select c.id from cloud_entries c join tree t on c.parent_id = t.id
         )
         select id from tree where id = $2 limit 1",
    )
    .bind(ancestor_id)
    .bind(candidate_id)
    .fetch_optional(db)
    .await?;
    Ok(found.is_some())
}

pub async fn update_entry(
    db: &PgPool,
    owner_id: Uuid,
    id: Uuid,
    request: UpdateEntryRequest,
) -> ApiResult<CloudEntry> {
    let entry = load_entry(db, owner_id, id).await?;
    if entry.trashed_at.is_some() {
        return Err(ApiError::validation(
            "Запись в корзине. Сначала восстановите её.",
        ));
    }

    if let Some(starred) = request.starred {
        if starred {
            sqlx::query(
                "insert into cloud_favorites (entry_id, user_id) values ($1, $2)
                 on conflict (entry_id) do nothing",
            )
            .bind(id)
            .bind(owner_id)
            .execute(db)
            .await?;
        } else {
            sqlx::query("delete from cloud_favorites where entry_id = $1 and user_id = $2")
                .bind(id)
                .bind(owner_id)
                .execute(db)
                .await?;
        }
        if request.name.is_none() && request.parent_id.is_none() {
            return load_entry(db, owner_id, id).await;
        }
    }

    let mut new_name = entry.name.clone();
    if let Some(raw_name) = request.name.as_deref() {
        new_name = validate_name(raw_name)?;
    }

    let mut new_parent = entry.parent_id;
    if let Some(target_parent) = request.parent_id {
        if target_parent != entry.parent_id {
            if let Some(target) = target_parent {
                let parent_entry = load_entry(db, owner_id, target).await?;
                if parent_entry.entry_type != "folder" {
                    return Err(ApiError::validation("Целевая запись не является папкой."));
                }
                if parent_entry.trashed_at.is_some() {
                    return Err(ApiError::validation("Целевая папка находится в корзине."));
                }
                if entry.entry_type == "folder"
                    && (target == id || is_descendant(db, id, target).await?)
                {
                    return Err(ApiError::validation(
                        "Нельзя переместить папку внутрь самой себя.",
                    ));
                }
            }
            new_parent = target_parent;
        }
    }

    if new_name != entry.name || new_parent != entry.parent_id {
        ensure_path_budget(db, owner_id, new_parent, &new_name).await?;
        if let Some(conflict) = entry_name_taken(db, owner_id, new_parent, &new_name).await? {
            if conflict.id != id {
                return Err(ApiError::conflict(
                    "FILE_NAME_CONFLICT",
                    "Запись с таким именем уже существует в целевой папке.",
                )
                .with_details(serde_json::json!({ "existingFileId": conflict.id })));
            }
        }
    }

    sqlx::query(
        "update cloud_entries
         set name = $2, parent_id = $3, updated_at = now()
         where id = $1 and owner_id = $4",
    )
    .bind(id)
    .bind(&new_name)
    .bind(new_parent)
    .bind(owner_id)
    .execute(db)
    .await
    .map_err(name_conflict_from_sqlx)?;

    load_entry(db, owner_id, id).await
}

/// Перемещает запись в корзину вместе с содержимым.
pub async fn trash_entry(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<()> {
    let entry = load_entry(db, owner_id, id).await?;
    if entry.trashed_at.is_some() {
        return Ok(());
    }
    sqlx::query(
        "with recursive tree as (
             select id from cloud_entries where id = $1
             union all
             select c.id from cloud_entries c join tree t on c.parent_id = t.id
         )
         update cloud_entries e set trashed_at = now()
         from tree where e.id = tree.id and e.owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Восстанавливает цепочку родительских папок записи из корзины.
///
/// Если запись удалили вместе с папкой, все предки тоже помечены `trashed_at`,
/// поэтому одиночное восстановление файла возвращало его в корень. Здесь
/// цепочка восстанавливается от корня к непосредственному родителю, чтобы
/// запись вернулась ровно на прежнее место. Посторонние элементы папки при
/// этом остаются в корзине — восстанавливается только путь к записи.
///
/// Возвращает актуальный `parent_id` записи (`None`, если родитель был удалён
/// безвозвратно и восстановить его невозможно).
async fn restore_parent_chain(
    db: &PgPool,
    owner_id: Uuid,
    parent_id: Option<Uuid>,
) -> ApiResult<Option<Uuid>> {
    let Some(parent_id) = parent_id else {
        return Ok(None);
    };

    // `depth` растёт вверх, поэтому сортировка по убыванию даёт путь от корня
    // к непосредственному родителю.
    let chain: Vec<(Uuid, Option<Uuid>, String, bool)> = sqlx::query_as(
        "with recursive chain as (
             select id, parent_id, name, 0 as depth, trashed_at
             from cloud_entries where id = $1 and owner_id = $2
             union all
             select e.id, e.parent_id, e.name, c.depth + 1, e.trashed_at
             from cloud_entries e join chain c on e.id = c.parent_id
             where e.owner_id = $2
         )
         select id, parent_id, name, (trashed_at is not null) as trashed
         from chain order by depth desc",
    )
    .bind(parent_id)
    .bind(owner_id)
    .fetch_all(db)
    .await?;

    if chain.is_empty() {
        // Родитель уже удалён безвозвратно — запись вернётся в корень.
        return Ok(None);
    }
    let mut current_parent: Option<Uuid> = None;

    for (entry_id, row_parent, name, trashed) in &chain {
        // Для верхней папки цепочки родитель — корень; каждая следующая
        // папка вкладывается в предыдущую.
        let new_parent = current_parent;
        if *trashed {
            let restore_name = match entry_name_taken(db, owner_id, new_parent, name).await? {
                Some(_) => {
                    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
                    format!("{name} (восстановлен {stamp})")
                }
                None => name.clone(),
            };
            sqlx::query(
                "update cloud_entries
                 set trashed_at = null, parent_id = $2, name = $3, updated_at = now()
                 where id = $1 and owner_id = $4",
            )
            .bind(entry_id)
            .bind(new_parent)
            .bind(&restore_name)
            .bind(owner_id)
            .execute(db)
            .await?;
        } else if *row_parent != new_parent {
            // Активная папка не может висеть под удалённым родителем; такой
            // случай возможен только после повреждения цепочки.
            sqlx::query(
                "update cloud_entries set parent_id = $2, updated_at = now()
                 where id = $1 and owner_id = $3",
            )
            .bind(entry_id)
            .bind(new_parent)
            .bind(owner_id)
            .execute(db)
            .await?;
        }
        current_parent = Some(*entry_id);
    }

    Ok(Some(parent_id))
}

pub async fn restore_entry(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<CloudEntry> {
    let entry = load_entry(db, owner_id, id).await?;
    if entry.trashed_at.is_none() {
        return Ok(entry);
    }

    // Возвращаем запись туда, откуда её удалили: если вместе с ней в корзину
    // попала исходная папка (или цепочка папок), восстанавливаем и её.
    let parent = restore_parent_chain(db, owner_id, entry.parent_id).await?;

    let restore_name = match entry_name_taken(db, owner_id, parent, &entry.name).await? {
        Some(_) => {
            let stamp = Utc::now().format("%Y%m%d-%H%M%S");
            format!("{} (восстановлен {})", entry.name, stamp)
        }
        None => entry.name.clone(),
    };

    sqlx::query(
        "with recursive tree as (
             select id from cloud_entries where id = $1
             union all
             select c.id from cloud_entries c join tree t on c.parent_id = t.id
         )
         update cloud_entries e set trashed_at = null, updated_at = now()
         from tree where e.id = tree.id and e.owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .execute(db)
    .await?;

    sqlx::query("update cloud_entries set parent_id = $2, name = $3 where id = $1")
        .bind(id)
        .bind(parent)
        .bind(&restore_name)
        .execute(db)
        .await?;

    load_entry(db, owner_id, id).await
}

/// Полное удаление записи и её поддерева. Освобождает ссылки на объекты.
/// Возвращает число удалённых записей-файлов.
pub async fn purge_entry(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<usize> {
    let ids = descendant_ids(db, id).await?;

    let files: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
        "select e.id, e.object_id, e.size_bytes
         from cloud_entries e
         where e.id = any($1) and e.entry_type = 'file' and e.object_id is not null",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;

    let mut tx = db.begin().await?;
    sqlx::query("delete from cloud_entries where id = any($1) and owner_id = $2")
        .bind(&ids)
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // После удаления записей освобождаем ссылки: физический файл исчезает
    // только когда на него больше не ссылается ни один владелец.
    let file_count = files.len();
    for (entry_id, object_id, size) in files {
        release_object(db, owner_id, entry_id, object_id, size).await?;
    }
    delete_zero_ref_objects(db).await?;
    Ok(file_count)
}

/// Освобождает ссылку владельца на объект. Файл удаляется только при ref_count = 0.
pub async fn release_object(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
    object_id: Uuid,
    size_bytes: i64,
) -> ApiResult<()> {
    let mut tx = db.begin().await?;
    sqlx::query("delete from storage_object_refs where owner_module = 'cloud' and owner_id = $1")
        .bind(entry_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "update storage_objects set ref_count = ref_count - 1 where id = $1 and ref_count > 0",
    )
    .bind(object_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "update storage_usage
         set used_bytes = greatest(used_bytes - $2, 0),
             file_count = greatest(file_count - 1, 0),
             updated_at = now()
         where user_id = $1",
    )
    .bind(owner_id)
    .bind(size_bytes)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Физически удаляет объекты с нулевым счётчиком ссылок.
pub async fn delete_zero_ref_objects(db: &PgPool) -> ApiResult<usize> {
    crate::storage::objects::delete_zero_ref_objects(db).await
}

pub async fn empty_trash(db: &PgPool, owner_id: Uuid) -> ApiResult<usize> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "select id from cloud_entries where owner_id = $1 and trashed_at is not null",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    let mut removed = 0;
    for id in ids {
        removed += purge_entry(db, owner_id, id).await?;
    }
    Ok(removed)
}

/// E4: автоочистка корзины по сроку хранения.
///
/// Удаляет записи, у которых `trashed_at` старше `retention_days`, вместе с
/// поддеревом. Для каждого файла освобождается ссылка владельца, после чего
/// `delete_zero_ref_objects` удаляет только объекты без оставшихся ссылок
/// (например, объект, используемый сборкой Minecraft, сохраняется).
///
/// Возвращает `(удалено записей, удалено файлов)`.
pub async fn cleanup_trash(db: &PgPool, retention_days: i64) -> ApiResult<(usize, usize)> {
    let days = retention_days.clamp(1, 3650) as i32;
    let expired: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "select id, owner_id
         from cloud_entries
         where trashed_at is not null
           and trashed_at < now() - make_interval(days => $1)
         order by trashed_at, id",
    )
    .bind(days)
    .fetch_all(db)
    .await?;

    let entries = expired.len();
    let mut files = 0;
    for (id, owner_id) in expired {
        // Запись могла быть удалена вместе с родителем на предыдущей итерации.
        let exists: bool =
            sqlx::query_scalar("select exists(select 1 from cloud_entries where id = $1)")
                .bind(id)
                .fetch_one(db)
                .await?;
        if !exists {
            continue;
        }
        files += purge_entry(db, owner_id, id).await?;
    }
    Ok((entries, files))
}

pub async fn breadcrumb_path(db: &PgPool, entry: &CloudEntry) -> ApiResult<Vec<(Uuid, String)>> {
    match entry.parent_id {
        Some(parent) => super::models::breadcrumb(db, parent)
            .await
            .map_err(ApiError::from),
        None => Ok(Vec::new()),
    }
}

/// Прибавляет использование квоты пользователя.
pub async fn add_usage(
    db: &PgPool,
    owner_id: Uuid,
    delta_bytes: i64,
    delta_count: i64,
) -> ApiResult<()> {
    sqlx::query(
        "insert into storage_usage (user_id, used_bytes, file_count, updated_at)
         values ($1, greatest($2, 0), greatest($3, 0), now())
         on conflict (user_id) do update
         set used_bytes = greatest(storage_usage.used_bytes + $2, 0),
             file_count = greatest(storage_usage.file_count + $3, 0),
             updated_at = now()",
    )
    .bind(owner_id)
    .bind(delta_bytes)
    .bind(delta_count)
    .execute(db)
    .await?;
    Ok(())
}
