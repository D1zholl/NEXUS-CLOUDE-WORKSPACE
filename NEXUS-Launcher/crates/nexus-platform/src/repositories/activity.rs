use nexus_core::{Clock, NexusError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use super::now_ms;
use crate::db::DbError;
use crate::security::redaction::{redact_text, sanitize_details};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
    pub summary: String,
    pub details_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Appends an activity record. Details are sanitized to remove secret-looking
/// fields before they reach SQLite.
pub async fn log(
    pool: &SqlitePool,
    clock: &dyn Clock,
    kind: &str,
    entity_kind: Option<&str>,
    entity_id: Option<&str>,
    summary: &str,
    details: Option<Value>,
) -> Result<i64, NexusError> {
    let kind = kind.trim();
    if kind.is_empty() || kind.len() > 64 {
        return Err(NexusError::invalid_argument("activity kind is invalid"));
    }
    let summary: String = summary.chars().take(512).collect();
    let summary = redact_text(&summary);
    let details = details.map(|value| sanitize_details(&value).to_string());
    let now = now_ms(clock);

    let result = sqlx::query(
        "INSERT INTO activity_logs
            (at, kind, entity_kind, entity_id, summary, details_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(kind)
    .bind(entity_kind)
    .bind(entity_id)
    .bind(&summary)
    .bind(&details)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;

    Ok(result.last_insert_rowid())
}

pub async fn list(pool: &SqlitePool, limit: u32) -> Result<Vec<ActivityRow>, NexusError> {
    let limit = limit.clamp(1, 1_000);
    sqlx::query_as::<_, ActivityRow>(
        "SELECT id, at, kind, entity_kind, entity_id, summary, details_json,
                created_at, updated_at
         FROM activity_logs
         ORDER BY at DESC, id DESC
         LIMIT ?",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
}

/// Retention policy: 90 days / 20 000 rows by default.
pub async fn purge(
    pool: &SqlitePool,
    older_than_ms: i64,
    max_rows: u32,
) -> Result<u64, NexusError> {
    let max_rows = max_rows.max(1);
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;

    let old = sqlx::query("DELETE FROM activity_logs WHERE at < ?")
        .bind(older_than_ms)
        .execute(&mut *transaction)
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?
        .rows_affected();

    let excess = sqlx::query(
        "DELETE FROM activity_logs
         WHERE id NOT IN (
             SELECT id FROM activity_logs ORDER BY at DESC, id DESC LIMIT ?
         )",
    )
    .bind(i64::from(max_rows))
    .execute(&mut *transaction)
    .await
    .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?
    .rows_affected();

    transaction
        .commit()
        .await
        .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
    Ok(old + excess)
}
