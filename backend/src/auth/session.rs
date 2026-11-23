use std::collections::HashSet;

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "nexus_session";
pub const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RoleRef {
    pub id: Uuid,
    pub key: String,
    pub name: String,
}

#[derive(Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub is_superadmin: bool,
    pub roles: Vec<RoleRef>,
    pub permissions: Vec<String>,
    pub session_id: Uuid,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
    pub ip: String,
}

impl AuthUser {
    pub fn can(&self, permission: &str) -> bool {
        self.is_superadmin || self.permissions.iter().any(|p| p == permission)
    }

    pub fn require(&self, permission: &str) -> ApiResult<()> {
        if self.can(permission) {
            Ok(())
        } else {
            Err(ApiError::forbidden(permission))
        }
    }
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn csrf_token(secret: &str, session_token: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(b"csrf:");
    mac.update(session_token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_csrf(secret: &str, session_token: &str, provided: &str) -> bool {
    let expected = csrf_token(secret, session_token);
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim().to_owned())
}

pub fn session_cookie_header(token: &str, ttl_hours: i64, secure: bool) -> String {
    let max_age = ttl_hours.max(1) * 3600;
    let mut value =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if secure {
        value.push_str("; Secure");
    }
    value
}

pub fn clear_cookie_header(secure: bool) -> String {
    let mut value = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        value.push_str("; Secure");
    }
    value
}

pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    ttl_hours: i64,
    ip: &str,
    user_agent: &str,
) -> ApiResult<(String, DateTime<Utc>)> {
    let token = generate_token();
    let token_hash = hash_token(&token);
    let expires_at = Utc::now() + chrono::Duration::hours(ttl_hours);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at, ip, user_agent)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(ip)
    .bind(truncate(user_agent, 400))
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    session_id: Uuid,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    username: String,
    display_name: String,
    status: String,
    is_superadmin: bool,
    locked_until: Option<DateTime<Utc>>,
    ip: String,
}

/// Проверяет токен сессии, продлевает её и загружает эффективные права.
///
/// Результат кэшируется на [`crate::state::SESSION_CACHE_TTL`]: при активной
/// панели один и тот же пользователь делает десятки запросов в секунду, и
/// четыре SQL-запроса на каждый из них не нужны. Явная инвалидация выполняется
/// при выходе, отзыве сессий, смене пароля и изменении прав.
pub async fn load_user_for_token(state: &AppState, token: &str) -> ApiResult<AuthUser> {
    let token_hash = hash_token(token);
    if let Some(cached) = state.cached_session(&token_hash) {
        return Ok(cached);
    }
    state.note_session_cache_miss();

    let row = sqlx::query_as::<_, SessionRow>(
        "select s.id as session_id, s.user_id, s.expires_at, s.last_seen_at, s.ip,
                u.username, u.display_name, u.status, u.is_superadmin, u.locked_until
         from sessions s
         join users u on u.id = s.user_id
         where s.token_hash = $1 and s.revoked_at is null",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        return Err(ApiError::session_expired());
    };
    let now = Utc::now();
    if row.expires_at <= now {
        return Err(ApiError::session_expired());
    }
    if row.status != "active" {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "ACCOUNT_DISABLED",
            "Учётная запись отключена администратором.",
        ));
    }
    if let Some(until) = row.locked_until {
        if until > now {
            return Err(ApiError::new(
                axum::http::StatusCode::LOCKED,
                "ACCOUNT_LOCKED",
                "Учётная запись временно заблокирована из-за неудачных попыток входа.",
            )
            .with_details(serde_json::json!({ "locked_until": until })));
        }
    }

    // Скользящее продление — не чаще раза в 5 минут, а не на каждый запрос.
    let extend = now.signed_duration_since(row.last_seen_at) >= chrono::Duration::minutes(5);
    let new_expires = if extend {
        let expires = now + chrono::Duration::hours(state.session_ttl_hours().await);
        sqlx::query("update sessions set last_seen_at = now(), expires_at = $2 where id = $1")
            .bind(row.session_id)
            .bind(expires)
            .execute(&state.db)
            .await?;
        expires
    } else {
        row.expires_at
    };

    let roles = sqlx::query_as::<_, RoleRef>(
        "select r.id, r.key, r.name
         from roles r
         join user_roles ur on ur.role_id = r.id
         where ur.user_id = $1
         order by r.name",
    )
    .bind(row.user_id)
    .fetch_all(&state.db)
    .await?;

    let permissions = if row.is_superadmin {
        crate::permissions::catalog::all_keys()
    } else {
        sqlx::query_scalar::<_, String>(
            "select distinct p.key from (
                 select rp.permission_key as key
                 from user_roles ur
                 join role_permissions rp on rp.role_id = ur.role_id
                 where ur.user_id = $1
                 union
                 select up.permission_key
                 from user_permissions up
                 where up.user_id = $1 and up.granted
             ) p
             where p.key not in (
                 select permission_key from user_permissions
                 where user_id = $1 and granted = false
             )
             order by p.key",
        )
        .bind(row.user_id)
        .fetch_all(&state.db)
        .await?
    };

    let mut unique = HashSet::new();
    unique.extend(permissions);

    let auth = AuthUser {
        id: row.user_id,
        username: row.username,
        display_name: row.display_name,
        is_superadmin: row.is_superadmin,
        roles,
        permissions: unique.into_iter().collect(),
        session_id: row.session_id,
        csrf_token: csrf_token(&state.config.security.secret, token),
        expires_at: new_expires,
        ip: row.ip,
    };
    state.cache_session(token_hash, auth.clone());
    Ok(auth)
}

pub fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
