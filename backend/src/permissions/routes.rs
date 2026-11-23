use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::routing::{get, patch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::permissions::catalog;
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const MANAGE: &str = "system.users.manage";
const VIEW: &str = "system.view";

pub fn roles_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_roles).post(create_role))
        .route("/{id}", patch(update_role).delete(delete_role))
}

pub fn permissions_router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list_permissions))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub is_system: bool,
    pub permissions: Vec<String>,
    pub user_count: i64,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    is_system: bool,
    permissions: Value,
    user_count: i64,
}

impl From<RoleRow> for RoleDto {
    fn from(row: RoleRow) -> Self {
        Self {
            id: row.id,
            key: row.key,
            name: row.name,
            description: row.description,
            is_system: row.is_system,
            permissions: serde_json::from_value(row.permissions).unwrap_or_default(),
            user_count: row.user_count,
        }
    }
}

const ROLE_SELECT: &str = "
    select r.id, r.key, r.name, r.description, r.is_system,
           coalesce(
             (select jsonb_agg(rp.permission_key order by rp.permission_key)
              from role_permissions rp where rp.role_id = r.id),
             '[]'::jsonb
           ) as permissions,
           (select count(*) from user_roles ur where ur.role_id = r.id) as user_count
    from roles r
";

async fn fetch_role(pool: &PgPool, id: Uuid) -> ApiResult<RoleDto> {
    let row = sqlx::query_as::<_, RoleRow>(&format!("{ROLE_SELECT} where r.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Роль не найдена."))?;
    Ok(row.into())
}

fn validate_role_key(key: &str) -> ApiResult<()> {
    let valid = (3..=48).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(ApiError::validation(
            "Ключ роли: 3–48 символов, латиница в нижнем регистре, цифры, точка, дефис, подчёркивание; начинается с буквы.",
        ));
    }
    Ok(())
}

fn validate_permissions(permissions: &[String]) -> ApiResult<()> {
    for key in permissions {
        if !catalog::is_valid(key) {
            return Err(ApiError::validation(format!("Неизвестное право: {key}")));
        }
    }
    Ok(())
}

async fn list_roles(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Vec<RoleDto>>> {
    actor.require(MANAGE)?;
    let rows =
        sqlx::query_as::<_, RoleRow>(&format!("{ROLE_SELECT} order by r.is_system desc, r.name"))
            .fetch_all(&state.db)
            .await?;
    Ok(ApiOk(rows.into_iter().map(RoleDto::from).collect()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRoleRequest {
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: Vec<String>,
}

async fn create_role(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<CreateRoleRequest>, JsonRejection>,
) -> ApiResult<ApiOk<RoleDto>> {
    actor.require(MANAGE)?;
    let body = body
        .map(|axum::Json(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректное тело запроса: {e}")))?;
    let key = body.key.trim().to_lowercase();
    validate_role_key(&key)?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::validation("Название роли: от 1 до 80 символов."));
    }
    validate_permissions(&body.permissions)?;

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "insert into roles (key, name, description) values ($1, $2, $3) returning id",
    )
    .bind(&key)
    .bind(name)
    .bind(body.description.trim())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(ref db) = e {
            if db.code().as_deref() == Some("23505") {
                return ApiError::conflict("ROLE_KEY_TAKEN", "Роль с таким ключом уже существует.");
            }
        }
        ApiError::internal(e)
    })?;
    for permission in &body.permissions {
        sqlx::query(
            "insert into role_permissions (role_id, permission_key) values ($1, $2)
             on conflict do nothing",
        )
        .bind(id)
        .bind(permission)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    state.invalidate_all_sessions();

    let role = fetch_role(&state.db, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "role.created",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "role",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "key": key, "permissions": body.permissions }),
        },
    )
    .await;
    Ok(ApiOk(role))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRoleRequest {
    name: Option<String>,
    description: Option<String>,
    permissions: Option<Vec<String>>,
}

async fn update_role(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<axum::Json<UpdateRoleRequest>, JsonRejection>,
) -> ApiResult<ApiOk<RoleDto>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(v)| v)
        .map_err(|_| ApiError::validation("Некорректный идентификатор роли."))?;
    let body = body
        .map(|axum::Json(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректное тело запроса: {e}")))?;

    let existing =
        sqlx::query_as::<_, (bool, String)>("select is_system, key from roles where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Роль не найдена."))?;
    if existing.0 {
        return Err(ApiError::conflict(
            "SYSTEM_ROLE",
            "Системную роль нельзя изменять.",
        ));
    }
    if let Some(permissions) = &body.permissions {
        validate_permissions(permissions)?;
    }
    let name = match body.name {
        Some(name) => {
            let name = name.trim().to_owned();
            if name.is_empty() || name.chars().count() > 80 {
                return Err(ApiError::validation("Название роли: от 1 до 80 символов."));
            }
            Some(name)
        }
        None => None,
    };

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "update roles set name = coalesce($2, name),
                description = coalesce($3, description), updated_at = now()
         where id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(&body.description)
    .execute(&mut *tx)
    .await?;

    if let Some(permissions) = &body.permissions {
        sqlx::query("delete from role_permissions where role_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for permission in permissions {
            sqlx::query(
                "insert into role_permissions (role_id, permission_key) values ($1, $2)
                 on conflict do nothing",
            )
            .bind(id)
            .bind(permission)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    state.invalidate_all_sessions();

    let role = fetch_role(&state.db, id).await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "role.updated",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "role",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "key": existing.1 }),
        },
    )
    .await;
    Ok(ApiOk(role))
}

async fn delete_role(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(v)| v)
        .map_err(|_| ApiError::validation("Некорректный идентификатор роли."))?;
    let existing =
        sqlx::query_as::<_, (bool, String)>("select is_system, key from roles where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Роль не найдена."))?;
    if existing.0 {
        return Err(ApiError::conflict(
            "SYSTEM_ROLE",
            "Системную роль нельзя удалить.",
        ));
    }
    let user_count: i64 = sqlx::query_scalar("select count(*) from user_roles where role_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if user_count > 0 {
        return Err(ApiError::conflict(
            "ROLE_IN_USE",
            "Роль назначена пользователям. Сначала переназначьте их.",
        )
        .with_details(json!({ "userCount": user_count })));
    }
    sqlx::query("delete from roles where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    state.invalidate_all_sessions();
    record(
        &state.db,
        AuditEvent {
            event_type: "role.deleted",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "role",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "key": existing.1 }),
        },
    )
    .await;
    Ok(ApiOk(json!({ "deleted": true })))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PermissionDto {
    pub key: String,
    pub module: String,
    pub label: String,
}

async fn list_permissions(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Vec<PermissionDto>>> {
    actor.require(VIEW)?;
    let rows = sqlx::query_as::<_, PermissionDto>(
        "select key, module, label from permissions order by module, key",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(ApiOk(rows))
}
