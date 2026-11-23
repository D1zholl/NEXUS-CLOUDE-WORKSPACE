use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// Запись журнала событий. Секреты и содержимое защищённых данных сюда не попадают.
pub struct AuditEvent<'a> {
    pub event_type: &'a str,
    pub actor_id: Option<Uuid>,
    pub actor_username: &'a str,
    pub target_type: &'a str,
    pub target_id: &'a str,
    pub result: &'a str,
    pub ip: &'a str,
    pub details: Value,
}

/// Аудит не должен ломать основную операцию: ошибка записи только логируется.
pub async fn record(pool: &PgPool, event: AuditEvent<'_>) {
    let result = sqlx::query(
        "insert into audit_events
           (event_type, actor_id, actor_username, target_type, target_id, result, ip, details)
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(event.event_type)
    .bind(event.actor_id)
    .bind(event.actor_username)
    .bind(event.target_type)
    .bind(event.target_id)
    .bind(event.result)
    .bind(event.ip)
    .bind(event.details)
    .execute(pool)
    .await;

    if let Err(error) = result {
        tracing::warn!(%error, event_type = event.event_type, "не удалось записать аудит");
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub event_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_username: String,
    pub target_type: String,
    pub target_id: String,
    pub result: String,
    pub ip: String,
    pub details: Value,
}
