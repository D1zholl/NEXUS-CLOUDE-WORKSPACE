use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::routing::{get, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

pub const BACKGROUND_KEY: &str = "appearance.background";
pub const BACKGROUND_DEFAULTS_KEY: &str = "appearance.background.defaults";
pub const BACKGROUND_QUALITY_KEY: &str = "appearance.background.quality";
pub const INTERFACE_KEY: &str = "appearance.interface";

/// Внешний вид интерфейса (K4). Тема пока одна — тёмная неоновая; хранится
/// акцент, который применяется к CSS-переменным без перезагрузки.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InterfaceSettings {
    pub accent: String,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            accent: "cyan".into(),
        }
    }
}

impl InterfaceSettings {
    pub fn sanitized(mut self) -> Self {
        if !matches!(self.accent.as_str(), "cyan" | "magenta" | "green" | "amber") {
            self.accent = "cyan".into();
        }
        self
    }
}

pub fn me_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(get_my_settings))
        .route("/settings/{key}", put(put_my_setting))
        .route("/usage", get(get_my_usage))
}

/// Публичный роутер оформления.
///
/// Экран входа и публичные ссылки открываются без сессии, но должны получать
/// тот же 3D-фон, который администратор задал системным значением. Маршрут
/// возвращает только эффективные параметры фона — без пользовательских данных
/// и без остальных системных настроек.
pub fn public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/background", get(get_public_background_defaults))
        // Системный default меняется администратором и должен применяться сразу,
        // в том числе на форме входа с чистыми cookie. Промежуточные кеши не
        // должны отдавать старое значение.
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
}

async fn get_my_usage(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    Ok(ApiOk(
        crate::cloud::quota::usage_summary(&state.db, user.id).await?,
    ))
}

/// Параметры 3D-фона. Сервер валидирует и нормализует значения,
/// чтобы клиент не мог задать неограниченную нагрузку на GPU.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackgroundSettings {
    pub enabled: bool,
    pub preset: String,
    pub palette: String,
    pub intensity: f64,
    pub quality: String,
    pub fps_limit: u32,
    pub count: u32,
    pub size_min: f64,
    pub size_max: f64,
    pub speed: f64,
    pub rotation: f64,
    pub opacity: f64,
    pub glow: f64,
    pub spacing: f64,
    pub depth: f64,
    pub spread: f64,
    pub wireframe: bool,
    pub stars: u32,
    pub star_brightness: f64,
    pub twinkle: f64,
    pub star_size: f64,
    pub star_depth: f64,
    pub background_speed: f64,
    pub parallax: f64,
    pub mouse_sensitivity: f64,
    pub perspective: f64,
    pub camera_amplitude: f64,
    pub vignette: f64,
    pub grain: f64,
    pub aberration: f64,
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: "flow".into(),
            palette: "neon".into(),
            intensity: 1.0,
            quality: "auto".into(),
            fps_limit: 0,
            count: 260,
            size_min: 0.35,
            size_max: 1.45,
            speed: 1.0,
            rotation: 1.0,
            opacity: 0.62,
            glow: 0.9,
            spacing: 1.0,
            depth: 1.0,
            spread: 1.0,
            wireframe: false,
            stars: 1200,
            star_brightness: 0.8,
            twinkle: 0.5,
            star_size: 0.5,
            star_depth: 1.0,
            background_speed: 1.0,
            parallax: 0.5,
            mouse_sensitivity: 0.5,
            perspective: 1.0,
            camera_amplitude: 0.4,
            vignette: 0.6,
            grain: 0.3,
            aberration: 0.25,
        }
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

impl BackgroundSettings {
    pub fn sanitized(mut self) -> Self {
        self.count = self.count.clamp(40, 700);
        self.size_min = clamp(self.size_min, 0.2, 2.0);
        self.size_max = clamp(self.size_max, 0.25, 3.0);
        if self.size_max < self.size_min {
            std::mem::swap(&mut self.size_min, &mut self.size_max);
        }
        self.speed = clamp(self.speed, 0.0, 3.0);
        self.rotation = clamp(self.rotation, 0.0, 3.0);
        self.opacity = clamp(self.opacity, 0.05, 1.0);
        self.glow = clamp(self.glow, 0.0, 2.5);
        self.spacing = clamp(self.spacing, 0.2, 2.5);
        self.depth = clamp(self.depth, 0.2, 2.0);
        self.spread = clamp(self.spread, 0.2, 2.0);
        self.intensity = clamp(self.intensity, 0.0, 2.0);
        self.stars = self.stars.min(2500);
        self.star_brightness = clamp(self.star_brightness, 0.0, 1.0);
        self.twinkle = clamp(self.twinkle, 0.0, 1.0);
        self.star_size = clamp(self.star_size, 0.1, 3.0);
        self.star_depth = clamp(self.star_depth, 0.2, 2.0);
        self.background_speed = clamp(self.background_speed, 0.0, 3.0);
        self.parallax = clamp(self.parallax, 0.0, 1.0);
        self.mouse_sensitivity = clamp(self.mouse_sensitivity, 0.0, 1.0);
        self.perspective = clamp(self.perspective, 0.5, 2.0);
        self.camera_amplitude = clamp(self.camera_amplitude, 0.0, 1.0);
        self.vignette = clamp(self.vignette, 0.0, 1.0);
        self.grain = clamp(self.grain, 0.0, 1.0);
        self.aberration = clamp(self.aberration, 0.0, 1.0);
        if !matches!(self.quality.as_str(), "auto" | "high" | "eco") {
            self.quality = "auto".into();
        }
        if !matches!(
            self.preset.as_str(),
            "flow" | "aurora" | "magma" | "ghost" | "custom"
        ) {
            self.preset = "flow".into();
        }
        if !matches!(
            self.palette.as_str(),
            "neon" | "aurora" | "magma" | "ice" | "acid"
        ) {
            self.palette = "neon".into();
        }
        if !matches!(self.fps_limit, 0 | 30 | 60 | 120) {
            self.fps_limit = 60;
        }
        self
    }
}

fn validate_setting_value(key: &str, value: Value) -> ApiResult<Value> {
    match key {
        BACKGROUND_KEY | BACKGROUND_DEFAULTS_KEY => {
            let parsed: BackgroundSettings = serde_json::from_value(value)
                .map_err(|e| ApiError::validation(format!("Некорректные параметры фона: {e}")))?;
            serde_json::to_value(parsed.sanitized())
                .map_err(|e| ApiError::validation(format!("Некорректные параметры фона: {e}")))
        }
        INTERFACE_KEY => {
            let parsed: InterfaceSettings = serde_json::from_value(value).map_err(|e| {
                ApiError::validation(format!("Некорректные параметры интерфейса: {e}"))
            })?;
            serde_json::to_value(parsed.sanitized()).map_err(|e| {
                ApiError::validation(format!("Некорректные параметры интерфейса: {e}"))
            })
        }
        other => crate::system::settings_catalog::validate(other, value).map_err(|message| {
            let code = if crate::system::settings_catalog::find(other).is_none() {
                "UNKNOWN_SETTING"
            } else {
                "VALIDATION_ERROR"
            };
            ApiError::new(axum::http::StatusCode::UNPROCESSABLE_ENTITY, code, message)
        }),
    }
}

fn parse_key(path: Result<Path<String>, PathRejection>) -> ApiResult<String> {
    path.map(|Path(v)| v)
        .map_err(|_| ApiError::validation("Некорректный ключ настройки."))
}

/// Собирает эффективные параметры фона по умолчанию: встроенные значения,
/// затем `appearance.background.quality`, затем `appearance.background.defaults`.
/// Недостающие поля всегда заполняются встроенными значениями.
fn background_defaults_from(quality: Option<&Value>, defaults: Option<&Value>) -> ApiResult<Value> {
    let mut value =
        serde_json::to_value(BackgroundSettings::default()).map_err(ApiError::internal)?;
    if let Some(overrides) = defaults.and_then(Value::as_object) {
        if let Some(target) = value.as_object_mut() {
            for (key, item) in overrides {
                target.insert(key.clone(), item.clone());
            }
        }
    }
    // Отдельный ключ качества применяется последним и перекрывает значение
    // внутри JSON-снимка `appearance.background.defaults`.
    if let Some(quality) = quality.and_then(Value::as_str) {
        if matches!(quality, "auto" | "high" | "eco") {
            value["quality"] = Value::String(quality.to_owned());
        }
    }
    let parsed: BackgroundSettings = serde_json::from_value(value)
        .map_err(|e| ApiError::validation(format!("Некорректные параметры фона: {e}")))?;
    serde_json::to_value(parsed.sanitized()).map_err(ApiError::internal)
}

/// Эффективные параметры фона для пользователей без личных настроек.
///
/// Значение не кешируется отдельно от `SettingsCache`: после изменения
/// `appearance.*` оно пересчитывается из БД, поэтому новый пользователь и
/// пользователь без собственных настроек сразу получают новое значение.
pub async fn effective_background_defaults(state: &AppState) -> ApiResult<Value> {
    let quality = state.settings.json(&state.db, BACKGROUND_QUALITY_KEY).await;
    let defaults = state
        .settings
        .json(&state.db, BACKGROUND_DEFAULTS_KEY)
        .await;
    background_defaults_from(quality.as_ref(), defaults.as_ref())
}

/// Фон для неаутентифицированных страниц (вход, публичные ссылки).
async fn get_public_background_defaults(
    State(state): State<Arc<AppState>>,
) -> ApiResult<ApiOk<Value>> {
    Ok(ApiOk(effective_background_defaults(&state).await?))
}

/// B7: эффективные значения всех ключей — то, что реально действует сейчас.
///
/// Для обычных ключей это значение из `system_settings` (или значение из
/// конфигурации, пока ключ не сохранён). Для `network.*` — конфигурация,
/// применённая при старте службы, а не только что сохранённое значение.
pub async fn effective_settings(state: &AppState) -> ApiResult<Value> {
    const MIB: u64 = 1024 * 1024;
    const GIB: i64 = 1024 * 1024 * 1024;

    let quota_cloud =
        crate::cloud::quota::default_quota(&state.db, crate::cloud::quota::Scope::Cloud).await?
            / GIB;
    let quota_minecraft =
        crate::cloud::quota::default_quota(&state.db, crate::cloud::quota::Scope::Minecraft)
            .await?
            / GIB;
    let background = effective_background_defaults(state).await?;
    let quality = background.get("quality").cloned().unwrap_or(Value::Null);

    Ok(json!({
        "limits.max_upload_mb": (state.max_upload_bytes().await / MIB) as i64,
        "limits.chunk_max_mb": (state.chunk_max_bytes().await / MIB) as i64,
        "limits.upload_ttl_hours": state.upload_ttl_hours().await,
        "limits.share_ttl_hours": state.share_ttl_hours().await,
        "limits.export_ttl_hours": state.export_ttl_hours().await,
        "limits.trash_retention_days": state.trash_retention_days().await,
        "limits.quota_cloud_gb": quota_cloud,
        "limits.quota_minecraft_gb": quota_minecraft,
        "jobs.max_concurrent": state.jobs_max_concurrent().await,
        "cloud.upload.concurrency": state.upload_concurrency().await,
        "security.session_ttl_hours": state.session_ttl_hours().await,
        "security.login_max_failures": state.login_max_failures().await,
        "security.password_min_length": state.password_min_length().await,
        // Restart-only: действует конфигурация, собранная при старте.
        "network.allowed_origins": state.config.security.allowed_origins.clone(),
        "network.trust_proxy": state.config.server.trust_proxy,
        "appearance.background.quality": quality,
        "appearance.background.defaults": background,
    }))
}

/// B5: переносит restart-only сетевые настройки из `system_settings` в
/// конфигурацию. Вызывается в `serve()` после подключения к БД и **до**
/// создания `AppState`/`build_router`, поэтому CORS и `trust_proxy` меняются
/// только при следующем старте службы.
pub async fn apply_startup_overrides(db: &sqlx::PgPool, config: &mut crate::config::Config) {
    let cache = crate::system::settings_cache::SettingsCache::new();

    if let Some(value) = cache.json(db, "network.allowed_origins").await {
        if let Some(items) = value.as_array() {
            config.security.allowed_origins = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
    }

    if let Some(value) = cache.json(db, "network.trust_proxy").await {
        if let Some(trust_proxy) = value.as_bool() {
            config.server.trust_proxy = trust_proxy;
        }
    }
}

/// Сохраняет системную настройку, применяет побочные эффекты и пишет аудит.
///
/// Единая точка применения: HTTP-обработчик и тесты используют один путь.
pub async fn apply_system_setting(
    state: &AppState,
    key: &str,
    value: Value,
    actor_id: Option<Uuid>,
    actor_username: &str,
    ip: &str,
) -> ApiResult<Value> {
    let value = validate_setting_value(key, value)?;
    let old_value: Option<Value> =
        sqlx::query_scalar("select value from system_settings where key = $1")
            .bind(key)
            .fetch_optional(&state.db)
            .await?;

    // Старый эффективный fallback нужен, чтобы после изменения сбросить только
    // те пользовательские копии, которые в точности повторяли системный дефолт.
    let old_background_default = if key == BACKGROUND_DEFAULTS_KEY || key == BACKGROUND_QUALITY_KEY
    {
        let quality: Option<Value> =
            sqlx::query_scalar("select value from system_settings where key = $1")
                .bind(BACKGROUND_QUALITY_KEY)
                .fetch_optional(&state.db)
                .await?;
        let defaults: Option<Value> =
            sqlx::query_scalar("select value from system_settings where key = $1")
                .bind(BACKGROUND_DEFAULTS_KEY)
                .fetch_optional(&state.db)
                .await?;
        Some(background_defaults_from(
            quality.as_ref(),
            defaults.as_ref(),
        )?)
    } else {
        None
    };

    sqlx::query(
        "insert into system_settings (key, value, updated_by, updated_at)
         values ($1, $2, $3, now())
         on conflict (key) do update set value = excluded.value,
             updated_by = excluded.updated_by, updated_at = now()",
    )
    .bind(key)
    .bind(&value)
    .bind(actor_id)
    .execute(&state.db)
    .await?;

    // TTL 10 с остаётся страховкой, но конкретное изменение видно сразу.
    state.settings.invalidate().await;

    // B6: квоты по умолчанию хранятся в байтах и применяются без перезапуска.
    let quota_scope = match key {
        "limits.quota_cloud_gb" => Some(crate::cloud::quota::Scope::Cloud),
        "limits.quota_minecraft_gb" => Some(crate::cloud::quota::Scope::Minecraft),
        _ => None,
    };
    if let Some(scope) = quota_scope {
        let gigabytes = value.as_i64().unwrap_or(0);
        let bytes = gigabytes
            .checked_mul(1024 * 1024 * 1024)
            .ok_or_else(|| ApiError::validation("Квота слишком велика."))?;
        crate::cloud::quota::set_default_quota(&state.db, scope, bytes).await?;
    }

    // B1: пользователи, сохранившие ровно старый системный fallback, получают
    // новый. Индивидуальные настройки, отличающиеся от дефолта, не затрагиваются.
    if let Some(old_default) = old_background_default {
        let new_default = effective_background_defaults(state).await?;
        if old_default != new_default {
            sqlx::query("delete from user_settings where key = $1 and value = $2")
                .bind(BACKGROUND_KEY)
                .bind(&old_default)
                .execute(&state.db)
                .await?;
        }
    }

    record(
        &state.db,
        AuditEvent {
            event_type: "settings.system.updated",
            actor_id,
            actor_username,
            target_type: "system_settings",
            target_id: key,
            result: "success",
            ip,
            details: json!({ "key": key, "old": old_value, "new": value }),
        },
    )
    .await;

    Ok(value)
}

fn parse_json(body: Result<Json<Value>, JsonRejection>) -> ApiResult<Value> {
    body.map(|Json(v)| v)
        .map_err(|e| ApiError::validation(format!("Некорректное тело запроса: {e}")))
}

async fn get_my_settings(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<ApiOk<HashMap<String, Value>>> {
    let mut settings: HashMap<String, Value> = sqlx::query_as::<_, (String, Value)>(
        "select key, value from user_settings where user_id = $1 order by key",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();

    if !settings.contains_key(BACKGROUND_KEY) {
        // Fallback всегда полный: встроенные значения + quality + defaults.
        settings.insert(
            BACKGROUND_KEY.to_owned(),
            effective_background_defaults(&state).await?,
        );
    }
    if !settings.contains_key(INTERFACE_KEY) {
        settings.insert(
            INTERFACE_KEY.to_owned(),
            serde_json::to_value(InterfaceSettings::default().sanitized())
                .map_err(|e| ApiError::internal(format!("Интерфейс: {e}")))?,
        );
    }
    Ok(ApiOk(settings))
}

async fn put_my_setting(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    let key = parse_key(path)?;
    if key != BACKGROUND_KEY && key != INTERFACE_KEY {
        return Err(ApiError::validation(
            "Эта настройка недоступна для изменения пользователем.",
        ));
    }
    let value = validate_setting_value(&key, parse_json(body)?)?;
    sqlx::query(
        "insert into user_settings (user_id, key, value, updated_at)
         values ($1, $2, $3, now())
         on conflict (user_id, key) do update set value = excluded.value, updated_at = now()",
    )
    .bind(user.id)
    .bind(&key)
    .bind(&value)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        AuditEvent {
            event_type: "settings.user.updated",
            actor_id: Some(user.id),
            actor_username: &user.username,
            target_type: "user_settings",
            target_id: &key,
            result: "success",
            ip: &user.ip,
            details: json!({ "key": key }),
        },
    )
    .await;
    Ok(ApiOk(value))
}

pub fn system_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(get_system_settings))
        .route("/settings/catalog", get(get_settings_catalog))
        .route("/settings/{key}", put(put_system_setting))
}

async fn get_settings_catalog(
    State(_state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("system.view")?;
    Ok(ApiOk(crate::system::settings_catalog::catalog_json()))
}

async fn get_system_settings(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require("system.settings.manage")?;
    let rows =
        sqlx::query_as::<_, (String, Value)>("select key, value from system_settings order by key")
            .fetch_all(&state.db)
            .await?;
    let mut values = serde_json::Map::new();
    let mut meta = serde_json::Map::new();
    for (key, value) in rows {
        values.insert(key.clone(), value);
        if let Some(spec) = crate::system::settings_catalog::find(&key) {
            meta.insert(
                key,
                serde_json::json!({
                    "requiresRestart": spec.requires_restart,
                    "kind": spec.kind.label(),
                }),
            );
        }
    }
    let effective = effective_settings(&state).await?;
    Ok(ApiOk(serde_json::json!({
        "values": values,
        "meta": meta,
        "catalog": crate::system::settings_catalog::catalog_json(),
        "effective": effective,
    })))
}

async fn put_system_setting(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require("system.settings.manage")?;
    let key = parse_key(path)?;
    let value = apply_system_setting(
        &state,
        &key,
        parse_json(body)?,
        Some(actor.id),
        &actor.username,
        &actor.ip,
    )
    .await?;
    Ok(ApiOk(value))
}
