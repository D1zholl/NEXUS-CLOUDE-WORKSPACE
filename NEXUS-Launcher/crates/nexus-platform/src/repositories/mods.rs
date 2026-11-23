use nexus_core::{Clock, ModId, NexusError};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{now_ms, path_key, validate_name, validate_relative_path, validate_sha256};
use crate::db::DbError;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ModRow {
    pub id: String,
    pub instance_id: String,
    pub provider: String,
    pub project_id: String,
    pub version_id: String,
    pub project_name: String,
    pub version_name: String,
    pub file_name: String,
    pub rel_path: String,
    pub path_key: String,
    pub side: String,
    pub enabled: bool,
    pub managed: bool,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub installed_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInstanceMod {
    pub provider: String,
    pub project_id: String,
    pub version_id: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub version_name: String,
    pub file_name: String,
    pub rel_path: String,
    #[serde(default = "default_side")]
    pub side: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_enabled")]
    pub managed: bool,
    pub size_bytes: i64,
    pub sha256: Option<String>,
}

fn default_side() -> String {
    "client".to_owned()
}

const fn default_enabled() -> bool {
    true
}

pub async fn create(
    pool: &SqlitePool,
    clock: &dyn Clock,
    instance_id: &str,
    new: NewInstanceMod,
) -> Result<ModRow, NexusError> {
    if !["modrinth", "curseforge", "nexus", "local"].contains(&new.provider.as_str()) {
        return Err(NexusError::invalid_argument("unsupported mod provider"));
    }
    if !["client", "server", "both"].contains(&new.side.as_str()) {
        return Err(NexusError::invalid_argument("unsupported mod side"));
    }
    let project_id = validate_name(&new.project_id, "project_id", 128)?;
    let version_id = validate_name(&new.version_id, "version_id", 128)?;
    let file_name = validate_name(&new.file_name, "file_name", 255)?;
    let rel_path = validate_relative_path(&new.rel_path)?;
    validate_sha256(new.sha256.as_deref(), "sha256")?;
    if new.size_bytes < 0 {
        return Err(NexusError::invalid_argument("size_bytes must be >= 0"));
    }

    let id = ModId::new().to_string();
    let now = now_ms(clock);

    sqlx::query(
        "INSERT INTO instance_mods
            (id, instance_id, provider, project_id, version_id, project_name,
             version_name, file_name, rel_path, path_key, side, enabled, managed,
             size_bytes, sha256, installed_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(instance_id)
    .bind(&new.provider)
    .bind(&project_id)
    .bind(&version_id)
    .bind(&new.project_name)
    .bind(&new.version_name)
    .bind(&file_name)
    .bind(&rel_path)
    .bind(path_key(&rel_path))
    .bind(&new.side)
    .bind(i64::from(new.enabled))
    .bind(i64::from(new.managed))
    .bind(new.size_bytes)
    .bind(&new.sha256)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "create instance mod"))?;

    get(pool, &id)
        .await?
        .ok_or_else(|| NexusError::not_found("instance mod was not created"))
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<ModRow>, NexusError> {
    sqlx::query_as::<_, ModRow>(
        "SELECT id, instance_id, provider, project_id, version_id, project_name,
                version_name, file_name, rel_path, path_key, side, enabled, managed,
                size_bytes, sha256, installed_at, created_at, updated_at
         FROM instance_mods
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list_for_instance(
    pool: &SqlitePool,
    instance_id: &str,
) -> Result<Vec<ModRow>, NexusError> {
    sqlx::query_as::<_, ModRow>(
        "SELECT id, instance_id, provider, project_id, version_id, project_name,
                version_name, file_name, rel_path, path_key, side, enabled, managed,
                size_bytes, sha256, installed_at, created_at, updated_at
         FROM instance_mods
         WHERE instance_id = ? AND deleted_at IS NULL
         ORDER BY rel_path",
    )
    .bind(instance_id)
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn set_enabled(
    pool: &SqlitePool,
    id: &str,
    enabled: bool,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let result = sqlx::query(
        "UPDATE instance_mods SET enabled = ?, updated_at = ?
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(i64::from(enabled))
    .bind(now_ms(clock))
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

pub async fn soft_delete(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE instance_mods SET deleted_at = ?, updated_at = ?
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
