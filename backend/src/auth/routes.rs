use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, State};
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, extract::rejection::JsonRejection};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::ApiOk;
use crate::auth::password::{dummy_hash, hash_password, validate_password, verify_password};
use crate::auth::rate_limit::ACCOUNT_LOCK_MINUTES;
use crate::auth::session::{
    AuthUser, RoleRef, clear_cookie_header, create_session, load_user_for_token,
    session_cookie_header, truncate,
};
use crate::core::error::{ApiError, ApiResult};
use crate::middleware::client_ip;
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/password", post(change_password))
        .route("/sessions/revoke-all", post(revoke_all))
        .route("/setup-status", get(setup_status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(sqlx::FromRow)]
struct LoginUser {
    id: Uuid,
    username: String,
    password_hash: String,
    status: String,
    locked_until: Option<DateTime<Utc>>,
    failed_login_attempts: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserIdentity {
    id: Uuid,
    username: String,
    display_name: String,
    is_superadmin: bool,
    roles: Vec<RoleRef>,
    permissions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionInfo {
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    user: UserIdentity,
    csrf_token: String,
    session: SessionInfo,
    modules: HashMap<String, ModuleState>,
}

#[derive(Serialize)]
struct ModuleState {
    enabled: bool,
}

fn parse_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректное тело запроса: {e}")))
}

async fn modules_map(state: &AppState) -> HashMap<String, ModuleState> {
    let map = state.modules.snapshot(&state.db).await;
    map.into_iter()
        .map(|(key, enabled)| (key, ModuleState { enabled }))
        .collect()
}

async fn me_payload(state: &AppState, user: AuthUser) -> ApiResult<MeResponse> {
    let modules = modules_map(state).await;
    Ok(MeResponse {
        user: UserIdentity {
            id: user.id,
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            is_superadmin: user.is_superadmin,
            roles: user.roles.clone(),
            permissions: user.permissions.clone(),
        },
        csrf_token: user.csrf_token.clone(),
        session: SessionInfo {
            expires_at: user.expires_at,
        },
        modules,
    })
}

fn attach_cookie(mut response: Response, cookie: String) -> Response {
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> ApiResult<Response> {
    let body = parse_body(body)?;
    let username = body.username.trim().to_owned();
    if username.is_empty() || body.password.is_empty() {
        return Err(ApiError::validation("Укажите имя пользователя и пароль."));
    }
    let ip = client_ip(&headers, Some(peer.ip()), state.config.server.trust_proxy);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let limiter_key = format!("{ip}|{}", username.to_lowercase());

    if let Err(retry) = state.login_limiter.check(&limiter_key) {
        return Err(ApiError::rate_limited(retry));
    }

    let found = sqlx::query_as::<_, LoginUser>(
        "select id, username, password_hash, status, locked_until, failed_login_attempts
         from users where lower(username) = lower($1)",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await?;

    let Some(user) = found else {
        // Выравниваем время ответа, чтобы не раскрывать существование пользователя.
        let _ = verify_password(body.password.clone(), dummy_hash().to_owned()).await;
        state.login_limiter.record_failure(&limiter_key);
        record(
            &state.db,
            AuditEvent {
                event_type: "auth.login.failed",
                actor_id: None,
                actor_username: &username,
                target_type: "session",
                target_id: "",
                result: "failure",
                ip: &ip,
                details: json!({ "reason": "unknown_user" }),
            },
        )
        .await;
        return Err(ApiError::invalid_credentials());
    };

    if user.status != "active" {
        record(
            &state.db,
            AuditEvent {
                event_type: "auth.login.failed",
                actor_id: Some(user.id),
                actor_username: &user.username,
                target_type: "session",
                target_id: "",
                result: "failure",
                ip: &ip,
                details: json!({ "reason": "account_disabled" }),
            },
        )
        .await;
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "ACCOUNT_DISABLED",
            "Учётная запись отключена администратором.",
        ));
    }

    if let Some(until) = user.locked_until {
        if until > Utc::now() {
            return Err(ApiError::new(
                axum::http::StatusCode::LOCKED,
                "ACCOUNT_LOCKED",
                "Учётная запись временно заблокирована. Попробуйте позже.",
            )
            .with_details(json!({ "lockedUntil": until })));
        }
    }

    if !verify_password(body.password.clone(), user.password_hash.clone()).await {
        let attempts = user.failed_login_attempts + 1;
        let max_failures = state.login_max_failures().await;
        let lock = i64::from(attempts) >= max_failures;
        sqlx::query(
            "update users set failed_login_attempts = $2,
                    locked_until = case when $3
                        then now() + make_interval(mins => $4)
                        else locked_until end,
                    updated_at = now()
             where id = $1",
        )
        .bind(user.id)
        .bind(attempts)
        .bind(lock)
        .bind(ACCOUNT_LOCK_MINUTES as i32)
        .execute(&state.db)
        .await?;
        state.login_limiter.record_failure(&limiter_key);
        record(
            &state.db,
            AuditEvent {
                event_type: "auth.login.failed",
                actor_id: Some(user.id),
                actor_username: &user.username,
                target_type: "session",
                target_id: "",
                result: "failure",
                ip: &ip,
                details: json!({ "reason": "bad_password", "attempts": attempts, "locked": lock }),
            },
        )
        .await;
        if lock {
            return Err(ApiError::new(
                axum::http::StatusCode::LOCKED,
                "ACCOUNT_LOCKED",
                format!(
                    "Слишком много неудачных попыток. Учётная запись заблокирована на {ACCOUNT_LOCK_MINUTES} минут."
                ),
            ));
        }
        return Err(ApiError::invalid_credentials());
    }

    sqlx::query(
        "update users set failed_login_attempts = 0, locked_until = null,
                last_login_at = now(), updated_at = now() where id = $1",
    )
    .bind(user.id)
    .execute(&state.db)
    .await?;

    let session_ttl_hours = state.session_ttl_hours().await;
    let (token, _) = create_session(
        &state.db,
        user.id,
        session_ttl_hours,
        &ip,
        truncate(user_agent, 400).as_str(),
    )
    .await?;
    let auth_user = load_user_for_token(&state, &token).await?;
    let payload = me_payload(&state, auth_user).await?;

    state.login_limiter.clear(&limiter_key);
    record(
        &state.db,
        AuditEvent {
            event_type: "auth.login.success",
            actor_id: Some(user.id),
            actor_username: &user.username,
            target_type: "session",
            target_id: "",
            result: "success",
            ip: &ip,
            details: json!({}),
        },
    )
    .await;

    let response = ApiOk(payload).into_response();
    Ok(attach_cookie(
        response,
        session_cookie_header(
            &token,
            session_ttl_hours,
            state.config.security.cookie_secure,
        ),
    ))
}

async fn me(State(state): State<Arc<AppState>>, user: AuthUser) -> ApiResult<ApiOk<MeResponse>> {
    let payload = me_payload(&state, user).await?;
    Ok(ApiOk(payload))
}

async fn logout(State(state): State<Arc<AppState>>, user: AuthUser) -> ApiResult<Response> {
    sqlx::query("update sessions set revoked_at = now() where id = $1")
        .bind(user.session_id)
        .execute(&state.db)
        .await?;
    state.invalidate_session(user.session_id);
    record(
        &state.db,
        AuditEvent {
            event_type: "auth.logout",
            actor_id: Some(user.id),
            actor_username: &user.username,
            target_type: "session",
            target_id: "",
            result: "success",
            ip: &user.ip,
            details: json!({}),
        },
    )
    .await;
    let response = ApiOk(json!({ "ok": true })).into_response();
    Ok(attach_cookie(
        response,
        clear_cookie_header(state.config.security.cookie_secure),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    body: Result<Json<ChangePasswordRequest>, JsonRejection>,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let body = parse_body(body)?;
    validate_password(&body.new_password, state.password_min_length().await)?;
    if body.new_password == body.current_password {
        return Err(ApiError::validation(
            "Новый пароль должен отличаться от текущего.",
        ));
    }
    let hash: Option<String> = sqlx::query_scalar("select password_hash from users where id = $1")
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let hash = hash.ok_or_else(|| ApiError::not_found("Пользователь не найден."))?;
    if !verify_password(body.current_password.clone(), hash).await {
        return Err(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_CURRENT_PASSWORD",
            "Текущий пароль указан неверно.",
        ));
    }
    let new_hash = hash_password(body.new_password.clone()).await?;
    sqlx::query("update users set password_hash = $2, updated_at = now() where id = $1")
        .bind(user.id)
        .bind(new_hash)
        .execute(&state.db)
        .await?;
    // Завершаем все остальные сессии пользователя.
    sqlx::query(
        "update sessions set revoked_at = now()
         where user_id = $1 and id <> $2 and revoked_at is null",
    )
    .bind(user.id)
    .bind(user.session_id)
    .execute(&state.db)
    .await?;
    state.invalidate_user_sessions(user.id);
    record(
        &state.db,
        AuditEvent {
            event_type: "auth.password.changed",
            actor_id: Some(user.id),
            actor_username: &user.username,
            target_type: "user",
            target_id: &user.id.to_string(),
            result: "success",
            ip: &user.ip,
            details: json!({}),
        },
    )
    .await;
    Ok(ApiOk(json!({ "ok": true })))
}

async fn revoke_all(State(state): State<Arc<AppState>>, user: AuthUser) -> ApiResult<Response> {
    sqlx::query("update sessions set revoked_at = now() where user_id = $1 and revoked_at is null")
        .bind(user.id)
        .execute(&state.db)
        .await?;
    state.invalidate_user_sessions(user.id);
    record(
        &state.db,
        AuditEvent {
            event_type: "auth.sessions.revoked",
            actor_id: Some(user.id),
            actor_username: &user.username,
            target_type: "user",
            target_id: &user.id.to_string(),
            result: "success",
            ip: &user.ip,
            details: json!({ "scope": "self", "all": true }),
        },
    )
    .await;
    let response = ApiOk(json!({ "ok": true })).into_response();
    Ok(attach_cookie(
        response,
        clear_cookie_header(state.config.security.cookie_secure),
    ))
}

async fn setup_status(State(state): State<Arc<AppState>>) -> ApiResult<ApiOk<serde_json::Value>> {
    let count: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&state.db)
        .await?;
    Ok(ApiOk(json!({
        "needsSetup": count == 0,
        "minPasswordLength": state.password_min_length().await,
    })))
}
