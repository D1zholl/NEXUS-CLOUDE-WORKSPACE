use nexus_core::{Clock, ErrorCode, InstanceId, NexusError};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{now_ms, validate_loader, validate_name};
use crate::db::DbError;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub dir_name: String,
    pub game_version: String,
    pub loader_type: String,
    pub loader_version: Option<String>,
    pub java_runtime_id: Option<String>,
    pub memory_min_mb: i64,
    pub memory_max_mb: i64,
    pub source_kind: String,
    pub install_state: String,
    pub archived_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub last_played_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInstance {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub game_version: String,
    pub loader_type: String,
    pub loader_version: Option<String>,
    pub java_runtime_id: Option<String>,
    pub memory_min_mb: i64,
    pub memory_max_mb: i64,
    #[serde(default = "default_source_kind")]
    pub source_kind: String,
}

fn default_source_kind() -> String {
    "local".to_owned()
}

pub async fn create(
    pool: &SqlitePool,
    clock: &dyn Clock,
    new: NewInstance,
) -> Result<InstanceRow, NexusError> {
    let name = validate_name(&new.name, "name", 96)?;
    let game_version = validate_name(&new.game_version, "game_version", 64)?;
    validate_loader(&new.loader_type, new.loader_version.as_deref())?;

    if !(512..=32_768).contains(&new.memory_min_mb) {
        return Err(NexusError::invalid_argument(
            "memory_min_mb must be between 512 and 32768",
        ));
    }
    if !(512..=32_768).contains(&new.memory_max_mb) || new.memory_max_mb < new.memory_min_mb {
        return Err(NexusError::invalid_argument(
            "memory_max_mb must be between memory_min_mb and 32768",
        ));
    }
    if !["local", "nexus", "modpack"].contains(&new.source_kind.as_str()) {
        return Err(NexusError::invalid_argument("unsupported source_kind"));
    }

    let id = InstanceId::new();
    let id_text = id.to_string();
    let now = now_ms(clock);
    let loader_version = new
        .loader_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    sqlx::query(
        "INSERT INTO instances
            (id, name, description, dir_name, game_version, loader_type, loader_version,
             java_runtime_id, memory_min_mb, memory_max_mb, source_kind,
             install_state, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
    )
    .bind(&id_text)
    .bind(&name)
    .bind(&new.description)
    .bind(&id_text)
    .bind(&game_version)
    .bind(&new.loader_type)
    .bind(loader_version)
    .bind(&new.java_runtime_id)
    .bind(new.memory_min_mb)
    .bind(new.memory_max_mb)
    .bind(&new.source_kind)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "create instance"))?;

    get(pool, &id_text)
        .await?
        .ok_or_else(|| NexusError::not_found("instance was not created"))
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<InstanceRow>, NexusError> {
    get_with_deleted(pool, id, false).await
}

pub async fn get_with_deleted(
    pool: &SqlitePool,
    id: &str,
    include_deleted: bool,
) -> Result<Option<InstanceRow>, NexusError> {
    sqlx::query_as::<_, InstanceRow>(
        "SELECT id, name, description, dir_name, game_version, loader_type,
                loader_version, java_runtime_id, memory_min_mb, memory_max_mb, source_kind,
                install_state, archived_at, deleted_at, last_played_at, created_at, updated_at
         FROM instances
         WHERE id = ? AND (? = 1 OR deleted_at IS NULL)",
    )
    .bind(id)
    .bind(i64::from(include_deleted))
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list(
    pool: &SqlitePool,
    include_archived: bool,
) -> Result<Vec<InstanceRow>, NexusError> {
    sqlx::query_as::<_, InstanceRow>(
        "SELECT id, name, description, dir_name, game_version, loader_type,
                loader_version, java_runtime_id, memory_min_mb, memory_max_mb, source_kind,
                install_state, archived_at, deleted_at, last_played_at, created_at, updated_at
         FROM instances
         WHERE deleted_at IS NULL AND (? = 1 OR archived_at IS NULL)
         ORDER BY archived_at IS NOT NULL, COALESCE(last_played_at, 0) DESC, name COLLATE NOCASE",
    )
    .bind(i64::from(include_archived))
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

/// Soft delete keeps the row (and its relations) for diagnostics. The instance
/// directory must be removed by the instance service after confirmation.
pub async fn soft_delete(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE instances SET deleted_at = ?, updated_at = ?
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

/// Hard delete is only used after the instance directory has been moved to
/// trash. Child rows (`instance_mods`, `installed_builds`) cascade.
pub async fn hard_delete(pool: &SqlitePool, id: &str) -> Result<bool, NexusError> {
    let result = sqlx::query("DELETE FROM instances WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_archived(
    pool: &SqlitePool,
    id: &str,
    archived: bool,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE instances SET archived_at = ?, updated_at = ?
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(if archived { Some(now) } else { None })
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

pub async fn touch_played(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE instances SET last_played_at = ?, updated_at = ?
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_install_state(
    pool: &SqlitePool,
    id: &str,
    state: &str,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    if !["pending", "installed", "failed"].contains(&state) {
        return Err(NexusError::new(
            ErrorCode::InvalidArgument,
            "Некорректное состояние установки.",
        ));
    }
    sqlx::query("UPDATE instances SET install_state = ?, updated_at = ? WHERE id = ?")
        .bind(state)
        .bind(now_ms(clock))
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(())
}

pub async fn require(pool: &SqlitePool, id: &str) -> Result<InstanceRow, NexusError> {
    get(pool, id)
        .await?
        .ok_or_else(|| NexusError::not_found(format!("instance {id}")))
}
