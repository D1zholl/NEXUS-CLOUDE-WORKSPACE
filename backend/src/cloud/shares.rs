//! Ссылки общего доступа Cloud Storage.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::service;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ShareRow {
    pub id: Uuid,
    pub entry_id: Uuid,
    pub entry_name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_downloads: Option<i32>,
    pub download_count: i32,
    pub has_password: bool,
    pub auth_only: bool,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active: bool,
}

pub fn hash_share_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShareRequest {
    /// Срок действия в часах; `0` — бессрочно.
    #[serde(default)]
    pub expires_in_hours: Option<i64>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub max_downloads: Option<i32>,
    /// Ссылка доступна только пользователям с действующей cookie-сессией.
    #[serde(default)]
    pub auth_only: bool,
}

pub async fn create_share(
    state: &AppState,
    owner_id: Uuid,
    entry_id: Uuid,
    request: CreateShareRequest,
) -> ApiResult<(ShareRow, String)> {
    let db = &state.db;
    let entry = service::load_entry(db, owner_id, entry_id).await?;
    if entry.trashed_at.is_some() {
        return Err(ApiError::validation(
            "Нельзя поделиться записью из корзины.",
        ));
    }
    if entry.entry_type != "file" {
        return Err(ApiError::validation(
            "Общий доступ пока поддерживается только для файлов.",
        ));
    }

    let hours = match request.expires_in_hours {
        Some(hours) => hours,
        None => state.share_ttl_hours().await,
    };
    if !(0..=24 * 365).contains(&hours) {
        return Err(ApiError::validation(
            "Срок действия ссылки должен быть от 0 до 8760 часов.",
        ));
    }
    if let Some(max) = request.max_downloads {
        if max <= 0 || max > 1_000_000 {
            return Err(ApiError::validation(
                "Лимит скачиваний должен быть положительным числом.",
            ));
        }
    }
    let password_hash = match request.password.as_deref().map(str::trim) {
        Some(password) if !password.is_empty() => {
            if password.chars().count() < 4 {
                return Err(ApiError::validation(
                    "Пароль ссылки должен содержать не менее 4 символов.",
                ));
            }
            Some(hash_password(password).await?)
        }
        _ => None,
    };

    let token = generate_token();
    let token_hash = hash_share_token(&token);
    let expires_at = if hours == 0 {
        None
    } else {
        Some(Utc::now() + Duration::hours(hours))
    };

    let id: Uuid = sqlx::query_scalar(
        "insert into cloud_shares
           (entry_id, owner_id, token_hash, password_hash, expires_at, max_downloads, auth_only)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id",
    )
    .bind(entry_id)
    .bind(owner_id)
    .bind(&token_hash)
    .bind(&password_hash)
    .bind(expires_at)
    .bind(request.max_downloads)
    .bind(request.auth_only)
    .fetch_one(db)
    .await?;

    let row = load_share(db, owner_id, id).await?;
    Ok((row, token))
}

async fn hash_password(password: &str) -> ApiResult<String> {
    crate::auth::password::hash_password(password.to_owned()).await
}

pub async fn verify_share_password(hash: &str, password: &str) -> bool {
    crate::auth::password::verify_password(password.to_owned(), hash.to_owned()).await
}

async fn load_share(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<ShareRow> {
    sqlx::query_as::<_, ShareRow>(
        "select s.id, s.entry_id, e.name as entry_name, s.created_at, s.expires_at,
                s.max_downloads, s.download_count,
                (s.password_hash is not null) as has_password,
                s.auth_only,
                s.revoked_at,
                (s.revoked_at is null
                 and (s.expires_at is null or s.expires_at > now())
                 and (s.max_downloads is null or s.download_count < s.max_downloads)) as active
         from cloud_shares s
         join cloud_entries e on e.id = s.entry_id
         where s.id = $1 and s.owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Ссылка не найдена."))
}

pub async fn list_shares(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Option<Uuid>,
) -> ApiResult<Vec<ShareRow>> {
    sqlx::query_as::<_, ShareRow>(
        "select s.id, s.entry_id, e.name as entry_name, s.created_at, s.expires_at,
                s.max_downloads, s.download_count,
                (s.password_hash is not null) as has_password,
                s.auth_only,
                s.revoked_at,
                (s.revoked_at is null
                 and (s.expires_at is null or s.expires_at > now())
                 and (s.max_downloads is null or s.download_count < s.max_downloads)) as active
         from cloud_shares s
         join cloud_entries e on e.id = s.entry_id
         where s.owner_id = $1
           and ($2::uuid is null or s.entry_id = $2)
         order by s.created_at desc",
    )
    .bind(owner_id)
    .bind(entry_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}

pub async fn revoke_share(db: &PgPool, owner_id: Uuid, id: Uuid) -> ApiResult<()> {
    let result = sqlx::query(
        "update cloud_shares set revoked_at = now()
         where id = $1 and owner_id = $2 and revoked_at is null",
    )
    .bind(id)
    .bind(owner_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("Активная ссылка не найдена."));
    }
    Ok(())
}

/// Публичная информация о ссылке (без раскрытия путей и идентификаторов хранения).
pub struct PublicShare {
    pub share_id: Uuid,
    pub entry_id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub size_bytes: i64,
    pub content_type: String,
    /// Внутренний хэш пароля: нужен для выпуска и проверки тикета.
    /// Наружу не сериализуется — публичный ответ содержит только `has_password`.
    pub password_hash: Option<String>,
    pub has_password: bool,
    pub auth_only: bool,
    pub download_count: i32,
    pub max_downloads: Option<i32>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct PublicShareRow {
    share_id: Uuid,
    entry_id: Uuid,
    owner_id: Uuid,
    name: String,
    size_bytes: i64,
    content_type: String,
    password_hash: Option<String>,
    auth_only: bool,
    download_count: i32,
    max_downloads: Option<i32>,
    expires_at: Option<DateTime<Utc>>,
}

pub async fn resolve_public_share(db: &PgPool, token: &str) -> ApiResult<PublicShare> {
    let token_hash = hash_share_token(token);
    let row: Option<PublicShareRow> = sqlx::query_as(
        "select s.id as share_id, s.entry_id, s.owner_id, e.name, e.size_bytes, e.content_type,
                s.password_hash, s.auth_only, s.download_count, s.max_downloads, s.expires_at
         from cloud_shares s
         join cloud_entries e on e.id = s.entry_id
         where s.token_hash = $1
           and s.revoked_at is null
           and e.trashed_at is null
           and (s.expires_at is null or s.expires_at > now())
           and (s.max_downloads is null or s.download_count < s.max_downloads)",
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await?;

    row.map(public_share_from_row)
        .ok_or_else(|| ApiError::not_found("Ссылка недействительна, истекла или отозвана."))
}

/// Загружает ссылку для `unlock`: лимит скачиваний проверяется отдельно,
/// атомарным `UPDATE`, чтобы вернуть `410` вместо общего `404`.
pub async fn resolve_share_for_unlock(db: &PgPool, token: &str) -> ApiResult<PublicShare> {
    let token_hash = hash_share_token(token);
    let row: Option<PublicShareRow> = sqlx::query_as(
        "select s.id as share_id, s.entry_id, s.owner_id, e.name, e.size_bytes, e.content_type,
                s.password_hash, s.auth_only, s.download_count, s.max_downloads, s.expires_at
         from cloud_shares s
         join cloud_entries e on e.id = s.entry_id
         where s.token_hash = $1
           and s.revoked_at is null
           and e.trashed_at is null
           and (s.expires_at is null or s.expires_at > now())",
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await?;

    row.map(public_share_from_row)
        .ok_or_else(|| ApiError::not_found("Ссылка недействительна, истекла или отозвана."))
}

/// Загружает ссылку по идентификатору после проверки тикета. Лимит здесь не
/// применяется: счётчик уже увеличен при выпуске тикета.
pub async fn resolve_shared_by_id(db: &PgPool, share_id: Uuid) -> ApiResult<PublicShare> {
    let row: Option<PublicShareRow> = sqlx::query_as(
        "select s.id as share_id, s.entry_id, s.owner_id, e.name, e.size_bytes, e.content_type,
                s.password_hash, s.auth_only, s.download_count, s.max_downloads, s.expires_at
         from cloud_shares s
         join cloud_entries e on e.id = s.entry_id
         where s.id = $1
           and s.revoked_at is null
           and e.trashed_at is null
           and (s.expires_at is null or s.expires_at > now())",
    )
    .bind(share_id)
    .fetch_optional(db)
    .await?;

    row.map(public_share_from_row)
        .ok_or_else(|| ApiError::not_found("Ссылка недействительна, истекла или отозвана."))
}

fn public_share_from_row(row: PublicShareRow) -> PublicShare {
    PublicShare {
        share_id: row.share_id,
        entry_id: row.entry_id,
        owner_id: row.owner_id,
        name: row.name,
        size_bytes: row.size_bytes,
        content_type: row.content_type,
        has_password: row.password_hash.is_some(),
        password_hash: row.password_hash,
        auth_only: row.auth_only,
        download_count: row.download_count,
        max_downloads: row.max_downloads,
        expires_at: row.expires_at,
    }
}

pub async fn check_share_password(db: &PgPool, token: &str, password: &str) -> ApiResult<bool> {
    let token_hash = hash_share_token(token);
    let hash: Option<Option<String>> = sqlx::query_scalar(
        "select password_hash from cloud_shares where token_hash = $1 and revoked_at is null",
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await?;
    match hash {
        Some(None) => Ok(true),
        Some(Some(hash)) => Ok(verify_share_password(&hash, password).await),
        None => Ok(false),
    }
}

/// Атомарно увеличивает счётчик скачиваний.
///
/// Возвращает `true`, если лимит ещё не был исчерпан и счётчик увеличен.
/// Условия в `WHERE` проверяются в той же транзакции, что и инкремент, поэтому
/// параллельные запросы не могут обойти `max_downloads`.
pub async fn register_download_atomic(db: &PgPool, share_id: Uuid) -> ApiResult<bool> {
    let updated: Option<i32> = sqlx::query_scalar(
        "update cloud_shares s
            set download_count = s.download_count + 1
          where s.id = $1
            and s.revoked_at is null
            and (s.expires_at is null or s.expires_at > now())
            and (s.max_downloads is null or s.download_count < s.max_downloads)
            and exists (
                select 1 from cloud_entries e
                 where e.id = s.entry_id and e.trashed_at is null
            )
          returning s.download_count",
    )
    .bind(share_id)
    .fetch_optional(db)
    .await?;
    Ok(updated.is_some())
}

/// Совместимый путь без тикета: инкремент счётчика.
pub async fn register_download(db: &PgPool, share_id: Uuid) -> ApiResult<()> {
    register_download_atomic(db, share_id).await.map(|_| ())
}

pub fn share_public_url(public_url: &str, token: &str) -> String {
    format!("{}/s/{token}", public_url.trim_end_matches('/'))
}

pub fn share_payload(row: &ShareRow, token: &str, public_url: &str) -> serde_json::Value {
    json!({
        "share": row,
        "token": token,
        "url": share_public_url(public_url, token),
    })
}
