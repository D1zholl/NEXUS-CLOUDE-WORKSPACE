use nexus_core::{Clock, NexusError, SourceId};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{now_ms, validate_name};
use crate::db::DbError;
use crate::security::secrets::validate_secret_ref;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub channel_default: String,
    pub auth_mode: String,
    pub has_secret: bool,
    pub contract_version: String,
    pub last_checked_at: Option<i64>,
    pub last_status: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSource {
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_channel")]
    pub channel_default: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    pub secret_ref: Option<String>,
}

fn default_channel() -> String {
    "stable".to_owned()
}

fn default_auth_mode() -> String {
    "public".to_owned()
}

pub async fn create(
    pool: &SqlitePool,
    clock: &dyn Clock,
    new: NewSource,
) -> Result<SourceRow, NexusError> {
    let name = validate_name(&new.name, "source name", 64)?;
    let base_url = normalize_base_url(&new.base_url)?;

    if !["stable", "beta"].contains(&new.channel_default.as_str()) {
        return Err(NexusError::invalid_argument(
            "channel_default must be stable or beta",
        ));
    }
    if !["public", "bearer"].contains(&new.auth_mode.as_str()) {
        return Err(NexusError::invalid_argument(
            "auth_mode must be public or bearer",
        ));
    }

    let secret_ref = match new.auth_mode.as_str() {
        "bearer" => {
            let reference = new.secret_ref.ok_or_else(|| {
                NexusError::invalid_argument("bearer source requires a keyring secret reference")
            })?;
            validate_secret_ref(&reference)?;
            Some(reference)
        }
        _ => {
            if new.secret_ref.is_some() {
                return Err(NexusError::invalid_argument(
                    "public source must not reference a secret",
                ));
            }
            None
        }
    };

    let id = SourceId::new().to_string();
    let now = now_ms(clock);

    sqlx::query(
        "INSERT INTO nexus_sources
            (id, name, base_url, enabled, channel_default, auth_mode, secret_ref,
             contract_version, created_at, updated_at)
         VALUES (?, ?, ?, 1, ?, ?, ?, '1.0.0', ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(&base_url)
    .bind(&new.channel_default)
    .bind(&new.auth_mode)
    .bind(&secret_ref)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "create nexus source"))?;

    get(pool, &id)
        .await?
        .ok_or_else(|| NexusError::not_found("nexus source was not created"))
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<SourceRow>, NexusError> {
    sqlx::query_as::<_, SourceRow>(
        "SELECT id, name, base_url, enabled, channel_default, auth_mode,
                (secret_ref IS NOT NULL) AS has_secret, contract_version,
                last_checked_at, last_status, created_at, updated_at
         FROM nexus_sources
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<SourceRow>, NexusError> {
    sqlx::query_as::<_, SourceRow>(
        "SELECT id, name, base_url, enabled, channel_default, auth_mode,
                (secret_ref IS NOT NULL) AS has_secret, contract_version,
                last_checked_at, last_status, created_at, updated_at
         FROM nexus_sources
         WHERE deleted_at IS NULL
         ORDER BY name COLLATE NOCASE",
    )
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
        "UPDATE nexus_sources SET enabled = ?, updated_at = ?
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
        "UPDATE nexus_sources SET deleted_at = ?, updated_at = ?
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

fn normalize_base_url(raw: &str) -> Result<String, NexusError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return Err(NexusError::invalid_argument("base_url length is invalid"));
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(NexusError::invalid_argument(
            "base_url must not contain whitespace or control characters",
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(NexusError::invalid_argument(
            "base_url must use http:// or https://",
        ));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_validation() {
        assert_eq!(
            normalize_base_url("https://nexus.example.com/").unwrap(),
            "https://nexus.example.com"
        );
        assert!(normalize_base_url("file:///etc/passwd").is_err());
        assert!(normalize_base_url("https://a b").is_err());
    }
}
