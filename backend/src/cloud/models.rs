//! Модели Cloud Storage.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CloudEntry {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub entry_type: String,
    pub size_bytes: i64,
    pub content_type: String,
    pub starred: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub trashed_at: Option<DateTime<Utc>>,
    /// Имя владельца: полезно для будущего шаринга между пользователями.
    #[sqlx(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
}

/// Полный путь папки от корня (хлебные крошки), читается запросом.
pub async fn breadcrumb(
    db: &sqlx::PgPool,
    mut entry_id: Uuid,
) -> sqlx::Result<Vec<(Uuid, String)>> {
    let mut trail = Vec::new();
    for _ in 0..64 {
        let row: Option<(Uuid, String, Option<Uuid>)> =
            sqlx::query_as("select id, name, parent_id from cloud_entries where id = $1")
                .bind(entry_id)
                .fetch_optional(db)
                .await?;
        let Some((id, name, parent)) = row else { break };
        trail.push((id, name));
        match parent {
            Some(pid) => entry_id = pid,
            None => break,
        }
    }
    trail.reverse();
    Ok(trail)
}

/// Возвращает всех потомков записи (для корзины/удаления). Без циклов.
pub async fn descendant_ids(db: &sqlx::PgPool, root_id: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "with recursive tree as (
             select id from cloud_entries where id = $1
             union all
             select c.id from cloud_entries c join tree t on c.parent_id = t.id
         )
         select id from tree",
    )
    .bind(root_id)
    .fetch_all(db)
    .await
}
