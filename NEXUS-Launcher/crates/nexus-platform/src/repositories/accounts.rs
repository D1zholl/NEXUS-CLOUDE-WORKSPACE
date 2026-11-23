use nexus_core::{AccountId, Clock, NexusError};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{now_ms, validate_name};
use crate::db::DbError;
use crate::security::secrets::validate_secret_ref;

/// Account row without any secret material. `has_secret` only tells the UI that
/// a keyring entry exists; the reference itself is not needed by the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AccountRow {
    pub id: String,
    pub provider: String,
    pub provider_account_id: String,
    pub username: String,
    pub uuid: Option<String>,
    pub display_name: Option<String>,
    pub status: String,
    pub has_secret: bool,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub provider: String,
    pub provider_account_id: String,
    pub username: String,
    pub uuid: Option<String>,
    pub display_name: Option<String>,
    /// Keyring entry name. For non-offline providers it is mandatory.
    pub secret_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedAccount {
    pub secret_ref: Option<String>,
}

pub async fn create(
    pool: &SqlitePool,
    clock: &dyn Clock,
    new: NewAccount,
) -> Result<AccountRow, NexusError> {
    let provider = new.provider.trim().to_ascii_lowercase();
    if !["offline", "microsoft", "elyby", "azuriom"].contains(&provider.as_str()) {
        return Err(NexusError::invalid_argument(format!(
            "unsupported account provider: {provider}"
        )));
    }

    let provider_account_id = validate_name(&new.provider_account_id, "provider_account_id", 128)?;
    let username = validate_name(&new.username, "username", 64)?;

    let uuid = match new.uuid.as_deref() {
        Some(value) if !value.trim().is_empty() => {
            Uuid::parse_str(value.trim())
                .map_err(|_| NexusError::invalid_argument("uuid must be a valid UUID string"))?;
            Some(value.trim().to_ascii_lowercase())
        }
        _ => None,
    };

    let secret_ref = match provider.as_str() {
        "offline" => {
            if new.secret_ref.is_some() {
                return Err(NexusError::invalid_argument(
                    "offline accounts must not reference a secret",
                ));
            }
            None
        }
        _ => {
            let reference = new.secret_ref.ok_or_else(|| {
                NexusError::invalid_argument("online accounts require a keyring secret reference")
            })?;
            validate_secret_ref(&reference)?;
            Some(reference)
        }
    };

    let id = AccountId::new().to_string();
    let now = now_ms(clock);

    sqlx::query(
        "INSERT INTO accounts
            (id, provider, provider_account_id, username, uuid, display_name,
             secret_ref, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
    )
    .bind(&id)
    .bind(&provider)
    .bind(&provider_account_id)
    .bind(&username)
    .bind(&uuid)
    .bind(&new.display_name)
    .bind(&secret_ref)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| DbError::conflict_or_query(error, "create account"))?;

    get(pool, &id)
        .await?
        .ok_or_else(|| NexusError::not_found("account was not created"))
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<AccountRow>, NexusError> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT id, provider, provider_account_id, username, uuid, display_name,
                status, (secret_ref IS NOT NULL) AS has_secret, last_used_at,
                created_at, updated_at
         FROM accounts
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<AccountRow>, NexusError> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT id, provider, provider_account_id, username, uuid, display_name,
                status, (secret_ref IS NOT NULL) AS has_secret, last_used_at,
                created_at, updated_at
         FROM accounts
         WHERE deleted_at IS NULL
         ORDER BY last_used_at DESC, created_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

pub async fn touch_used(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    let now = now_ms(clock);
    let result = sqlx::query(
        "UPDATE accounts SET last_used_at = ?, updated_at = ?
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

pub async fn set_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    clock: &dyn Clock,
) -> Result<bool, NexusError> {
    if !["active", "expired", "error"].contains(&status) {
        return Err(NexusError::invalid_argument("unsupported account status"));
    }
    let result = sqlx::query(
        "UPDATE accounts SET status = ?, updated_at = ?
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(status)
    .bind(now_ms(clock))
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(result.rows_affected() > 0)
}

/// Soft-deletes the account and returns the keyring reference so the caller can
/// remove the secret. The SQLite row never contained the token.
pub async fn soft_delete(
    pool: &SqlitePool,
    id: &str,
    clock: &dyn Clock,
) -> Result<Option<DeletedAccount>, NexusError> {
    let secret_ref: Option<Option<String>> =
        sqlx::query_scalar("SELECT secret_ref FROM accounts WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;

    let Some(secret_ref) = secret_ref else {
        return Ok(None);
    };

    let now = now_ms(clock);
    sqlx::query("UPDATE accounts SET deleted_at = ?, updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;

    Ok(Some(DeletedAccount { secret_ref }))
}
