//! Токены лаунчера: выдача, проверка, отзыв.
//!
//! Лаунчер не использует браузерную сессию: он получает ограниченный токен
//! (`nexus_launcher_…`), в БД хранится только его SHA-256. Токен привязан
//! к пользователю, набору scopes и сроку действия.

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password::verify_password;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const SCOPE_CATALOG: &str = "launcher.catalog";
pub const SCOPE_ARTIFACTS: &str = "launcher.artifacts";
pub const TOKEN_PREFIX: &str = "nxl_";
const TOKEN_BYTES: usize = 32;

#[derive(sqlx::FromRow)]
struct LauncherTokenRow {
    id: Uuid,
    user_id: Uuid,
    scopes: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

/// Максимальный срок жизни токена лаунчера.
const MAX_TTL_HOURS: i64 = 24 * 365;

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// Аутентифицированный клиент лаунчера.
#[derive(Debug, Clone)]
pub struct LauncherClient {
    pub token_id: Uuid,
    pub user_id: Uuid,
    pub scopes: Vec<String>,
    pub ip: String,
}

impl LauncherClient {
    pub fn can(&self, scope: &str) -> bool {
        self.scopes.iter().any(|value| value == scope)
    }

    pub fn require(&self, scope: &str) -> ApiResult<()> {
        if self.can(scope) {
            Ok(())
        } else {
            Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "LAUNCHER_SCOPE_REQUIRED",
                "Токену лаунчера не разрешено это действие.",
            )
            .with_details(serde_json::json!({ "requiredScope": scope })))
        }
    }
}

/// Извлекает токен из `Authorization: Bearer …` или `X-Launcher-Token`.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get("x-launcher-token")
        .and_then(|value| value.to_str().ok())
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

impl FromRequestParts<std::sync::Arc<AppState>> for LauncherClient {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let ip = crate::middleware::client_ip(
            &parts.headers,
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|value| value.0.ip()),
            state.config.server.trust_proxy,
        );
        authenticate(state, &parts.headers, ip)
            .await?
            .ok_or_else(|| {
                ApiError::unauthorized(
                    "LAUNCHER_TOKEN_REQUIRED",
                    "Требуется токен лаунчера в заголовке Authorization.",
                )
            })
    }
}

/// Проверяет токен, если он передан. `None` означает анонимный запрос.
///
/// Репозиторий MyLauncher не отправляет токен вовсе; эта функция нужна для
/// публичных repository-endpoints, которые дополнительно поддерживают Bearer
/// для будущих клиентов и приватных сборок.
pub async fn optional_client(
    state: &AppState,
    headers: &HeaderMap,
    ip: String,
) -> ApiResult<Option<LauncherClient>> {
    authenticate(state, headers, ip).await
}

/// Общая проверка токена для экстрактора и публичных endpoints.
pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    ip: String,
) -> ApiResult<Option<LauncherClient>> {
    let Some(token) = token_from_headers(headers) else {
        return Ok(None);
    };
    let hash = hash_token(&token);
    let row: Option<LauncherTokenRow> = sqlx::query_as(
        "select id, user_id, scopes, expires_at, revoked_at
         from access_tokens where token_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await?;
    let Some(LauncherTokenRow {
        id: token_id,
        user_id,
        scopes,
        expires_at,
        revoked_at,
    }) = row
    else {
        return Err(ApiError::unauthorized(
            "LAUNCHER_TOKEN_INVALID",
            "Токен лаунчера недействителен.",
        ));
    };
    if revoked_at.is_some() {
        return Err(ApiError::unauthorized(
            "LAUNCHER_TOKEN_REVOKED",
            "Токен лаунчера отозван.",
        ));
    }
    let now = Utc::now();
    if let Some(expires_at) = expires_at {
        if expires_at <= now {
            return Err(ApiError::unauthorized(
                "LAUNCHER_TOKEN_EXPIRED",
                "Срок действия токена лаунчера истёк.",
            ));
        }
    }

    // Отметка использования — не чаще раза в минуту, чтобы не писать на каждый запрос.
    sqlx::query(
        "update access_tokens set last_used_at = now()
         where id = $1 and (last_used_at is null or last_used_at < now() - interval '1 minute')",
    )
    .bind(token_id)
    .execute(&state.db)
    .await
    .ok();

    Ok(Some(LauncherClient {
        token_id,
        user_id,
        scopes,
        ip,
    }))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TokenRow {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenRequest {
    /// Имя устройства/клиента, чтобы администратор мог различить токены.
    pub name: String,
    /// Логин пользователя, от имени которого работает лаунчер.
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Срок жизни в часах; по умолчанию 720 (30 дней).
    #[serde(default)]
    pub expires_in_hours: Option<i64>,
}

#[derive(Debug)]
pub struct IssuedToken {
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

/// Выдаёт токен лаунчера после проверки логина и пароля пользователя.
pub async fn issue_token(state: &AppState, request: CreateTokenRequest) -> ApiResult<IssuedToken> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::validation(
            "Укажите имя клиента длиной до 80 символов.",
        ));
    }
    let username = request.username.trim();
    if username.is_empty() {
        return Err(ApiError::validation("Укажите имя пользователя."));
    }

    let scopes = normalize_scopes(request.scopes.as_deref())?;
    let hours = request.expires_in_hours.unwrap_or(24 * 30);
    if hours <= 0 || hours > MAX_TTL_HOURS {
        return Err(ApiError::validation(
            "Срок действия токена должен быть от 1 до 8760 часов.",
        ));
    }

    let limit_key = format!("launcher-token:{}", username.to_lowercase());
    if let Err(retry_after) = state.login_limiter.check(&limit_key) {
        return Err(ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "Слишком много попыток. Повторите позже.",
        )
        .with_details(serde_json::json!({ "retryAfter": retry_after })));
    }

    let row: Option<(Uuid, String, String, bool)> = sqlx::query_as(
        "select id, password_hash, status, is_superadmin from users where lower(username) = lower($1)",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, password_hash, status, is_superadmin)) = row else {
        state.login_limiter.record_failure(&limit_key);
        return Err(ApiError::invalid_credentials());
    };
    if !verify_password(request.password, password_hash).await {
        state.login_limiter.record_failure(&limit_key);
        return Err(ApiError::invalid_credentials());
    }
    if status != "active" {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "ACCOUNT_DISABLED",
            "Учётная запись отключена.",
        ));
    }
    if locked(&state.db, user_id).await? {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "ACCOUNT_LOCKED",
            "Учётная запись временно заблокирована.",
        ));
    }
    state.login_limiter.clear(&limit_key);

    // Суперадмин не должен выдавать токены лаунчера без явных scopes.
    let scopes = if is_superadmin && scopes.is_empty() {
        vec![SCOPE_CATALOG.to_owned(), SCOPE_ARTIFACTS.to_owned()]
    } else {
        scopes
    };

    let token = generate_token();
    let hash = hash_token(&token);
    let expires_at = Utc::now() + Duration::hours(hours);

    sqlx::query(
        "insert into access_tokens (user_id, name, token_hash, scopes, expires_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(name)
    .bind(&hash)
    .bind(&scopes)
    .bind(expires_at)
    .execute(&state.db)
    .await?;

    Ok(IssuedToken {
        token,
        expires_at: Some(expires_at),
        scopes,
    })
}

async fn locked(db: &PgPool, user_id: Uuid) -> ApiResult<bool> {
    let locked_until: Option<DateTime<Utc>> =
        sqlx::query_scalar("select locked_until from users where id = $1")
            .bind(user_id)
            .fetch_one(db)
            .await?;
    Ok(locked_until
        .map(|until| until > Utc::now())
        .unwrap_or(false))
}

fn normalize_scopes(raw: Option<&[String]>) -> ApiResult<Vec<String>> {
    let Some(values) = raw else {
        return Ok(vec![SCOPE_CATALOG.to_owned(), SCOPE_ARTIFACTS.to_owned()]);
    };
    if values.is_empty() {
        return Ok(vec![SCOPE_CATALOG.to_owned(), SCOPE_ARTIFACTS.to_owned()]);
    }
    let mut out = Vec::new();
    for value in values {
        let scope = value.trim();
        match scope {
            SCOPE_CATALOG | SCOPE_ARTIFACTS => {
                if !out.iter().any(|existing| existing == scope) {
                    out.push(scope.to_owned());
                }
            }
            _ => {
                return Err(ApiError::validation(format!(
                    "Неизвестный scope «{scope}»."
                )));
            }
        }
    }
    Ok(out)
}

pub async fn list_tokens(db: &PgPool) -> ApiResult<Vec<TokenRow>> {
    sqlx::query_as::<_, TokenRow>(
        "select t.id, t.name, t.scopes, t.last_used_at, t.expires_at, t.revoked_at,
                t.created_at, u.username
         from access_tokens t
         join users u on u.id = t.user_id
         order by t.created_at desc",
    )
    .fetch_all(db)
    .await
    .map_err(ApiError::from)
}

pub async fn revoke_token(db: &PgPool, id: Uuid) -> ApiResult<()> {
    let result = sqlx::query(
        "update access_tokens set revoked_at = now() where id = $1 and revoked_at is null",
    )
    .bind(id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("Активный токен не найден."));
    }
    Ok(())
}

/// Отзыв токена самим лаунчером (по предъявленному токену).
pub async fn revoke_self(db: &PgPool, token_id: Uuid) -> ApiResult<()> {
    sqlx::query("update access_tokens set revoked_at = now() where id = $1 and revoked_at is null")
        .bind(token_id)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_and_prefixed() {
        let a = hash_token("nxl_abc");
        let b = hash_token("nxl_abc");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(a, hash_token("nxl_abd"));
    }

    #[test]
    fn scopes_are_validated() {
        assert_eq!(
            normalize_scopes(None).unwrap(),
            vec![SCOPE_CATALOG.to_owned(), SCOPE_ARTIFACTS.to_owned()]
        );
        assert!(normalize_scopes(Some(&["bad.scope".to_owned()])).is_err());
        assert_eq!(
            normalize_scopes(Some(&[SCOPE_ARTIFACTS.to_owned()])).unwrap(),
            vec![SCOPE_ARTIFACTS.to_owned()]
        );
    }
}
