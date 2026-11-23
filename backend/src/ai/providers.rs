//! Provider configurations и операции через назначенный Runtime.
//!
//! Backend не обращается к `127.0.0.1` PolyStrata: запрос уходит на компьютер
//! Runtime по защищённому каналу. API key хранится зашифрованным и никогда не
//! возвращается frontend.

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::ai::models::ProviderRow;
use crate::ai::runtime::protocol::{ProviderRequest, ProviderResponse, kind};
use crate::ai::runtime::{self, RuntimeReply};
use crate::ai::{PendingRequest, errors};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn load_provider(db: &PgPool, id: Uuid) -> ApiResult<ProviderRow> {
    sqlx::query_as::<_, ProviderRow>(
        "select id, name, provider_type, base_url, api_key_cipher, default_model,
                generation, limits, runtime_node_id, enabled, status,
                last_checked_at, last_error, last_models, created_by, created_at, updated_at
           from ai_provider_configs where id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Провайдер не найден."))
}

pub async fn list_providers(db: &PgPool, include_disabled: bool) -> ApiResult<Vec<ProviderRow>> {
    Ok(sqlx::query_as::<_, ProviderRow>(
        "select id, name, provider_type, base_url, api_key_cipher, default_model,
                generation, limits, runtime_node_id, enabled, status,
                last_checked_at, last_error, last_models, created_by, created_at, updated_at
           from ai_provider_configs
          where ($1 or enabled = true)
          order by lower(name)",
    )
    .bind(include_disabled)
    .fetch_all(db)
    .await?)
}

pub async fn decrypted_key(state: &AppState, provider_id: Uuid) -> ApiResult<Option<String>> {
    let cipher: Option<String> =
        sqlx::query_scalar("select api_key_cipher from ai_provider_configs where id = $1")
            .bind(provider_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    match cipher {
        Some(value) => Ok(Some(state.ai.secrets.decrypt(&value)?)),
        None => Ok(None),
    }
}

pub fn validate_base_url(value: &str) -> ApiResult<()> {
    let trimmed = value.trim();
    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|_| ApiError::validation("Base URL провайдера должен быть корректным URL."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::validation(
            "Base URL провайдера должен использовать http или https.",
        ));
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ApiError::validation(
            "Base URL не должен содержать учётные данные.",
        ));
    }
    Ok(())
}

/// Runtime, через который доступен провайдер.
pub fn provider_node(provider: &ProviderRow) -> ApiResult<Uuid> {
    provider
        .runtime_node_id
        .ok_or_else(|| ApiError::validation("Провайдеру не назначен Runtime Node."))
}

/// Выполняет диагностическую операцию провайдера через Runtime.
pub async fn provider_operation(
    state: &AppState,
    provider: &ProviderRow,
    operation: &str,
) -> ApiResult<ProviderResponse> {
    if !provider.enabled {
        return Err(errors::provider_unavailable());
    }
    let node_id = provider_node(provider)?;
    let api_key = decrypted_key(state, provider.id).await?;
    let request = ProviderRequest {
        operation: operation.to_owned(),
        base_url: provider.base_url.clone(),
        api_key: api_key.clone(),
        model: (!provider.default_model.is_empty()).then(|| provider.default_model.clone()),
        timeout_seconds: 20,
    };
    let wire_payload = serde_json::to_value(&request)
        .map_err(|error| ApiError::internal(format!("provider request json: {error}")))?;
    let mut stored = wire_payload.clone();
    if let Some(object) = stored.as_object_mut() {
        object.remove("apiKey");
        object.insert("providerId".to_owned(), json!(provider.id));
    }

    let message_id = Uuid::new_v4();
    let (tx, rx) = oneshot::channel();
    state
        .ai
        .runtime
        .pending_register(message_id, node_id, PendingRequest::Reply(tx));
    if let Err(error) = runtime::send_command(
        state,
        node_id,
        message_id,
        kind::PROVIDER_REQUEST,
        None,
        None,
        wire_payload,
        Some(stored),
    )
    .await
    {
        let _ = state.ai.runtime.pending_remove(message_id);
        return Err(error);
    }

    match tokio::time::timeout(std::time::Duration::from_secs(35), rx).await {
        Ok(Ok(RuntimeReply::ProviderOp(response))) => {
            if response.ok {
                Ok(response)
            } else {
                Err(ApiError::new(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "AI_PROVIDER_UNAVAILABLE",
                    response
                        .error
                        .unwrap_or_else(|| "PolyStrata недоступна через Runtime.".to_owned()),
                ))
            }
        }
        Ok(Ok(_)) | Ok(Err(_)) => Err(ApiError::unavailable(
            "AI_PROVIDER_TIMEOUT",
            "Runtime не ответил на запрос провайдера. Повторите попытку.",
            5,
        )),
        Err(_) => {
            let _ = state.ai.runtime.pending_remove(message_id);
            Err(ApiError::unavailable(
                "AI_PROVIDER_TIMEOUT",
                "Истекло время ожидания ответа Runtime.",
                5,
            ))
        }
    }
}

/// Обновляет сохранённый статус провайдера после проверки.
pub async fn store_probe_result(
    db: &PgPool,
    provider_id: Uuid,
    models: &[String],
    error: Option<&str>,
) {
    let status = if error.is_some() { "error" } else { "ok" };
    let _ = sqlx::query(
        "update ai_provider_configs
            set status = $2, last_checked_at = now(), last_error = $3,
                last_models = $4, updated_at = now()
          where id = $1",
    )
    .bind(provider_id)
    .bind(status)
    .bind(error)
    .bind(json!(models))
    .execute(db)
    .await;
}

/// Проверка доступности перед стартом чата/задачи.
pub async fn ensure_usable(
    state: &AppState,
    provider_id: Uuid,
    model: &str,
) -> ApiResult<ProviderRow> {
    let provider = load_provider(&state.db, provider_id).await?;
    if !provider.enabled {
        return Err(errors::provider_unavailable());
    }
    if model.trim().is_empty() && provider.default_model.trim().is_empty() {
        return Err(ApiError::validation("Не выбрана модель провайдера."));
    }
    let node = provider_node(&provider)?;
    let status: Option<String> =
        sqlx::query_scalar("select status from ai_runtime_nodes where id = $1")
            .bind(node)
            .fetch_optional(&state.db)
            .await?;
    match status.as_deref() {
        Some("revoked") => return Err(errors::runtime_revoked()),
        Some("disabled") => {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "AI_RUNTIME_DISABLED",
                "Runtime провайдера отключён администратором.",
            ));
        }
        _ => {}
    }
    if !state.ai.runtime.is_online(node) {
        return Err(errors::runtime_offline());
    }
    Ok(provider)
}

/// Публичные поля провайдера для обычного пользователя.
pub fn user_view(provider: &ProviderRow) -> Value {
    json!({
        "id": provider.id,
        "name": provider.name,
        "defaultModel": provider.default_model,
        "models": provider.last_models,
        "status": provider.status,
        "runtimeNodeId": provider.runtime_node_id,
        "lastCheckedAt": provider.last_checked_at,
    })
}

pub async fn touch_status(db: &PgPool, provider_id: Uuid, status: &str, error: Option<&str>) {
    let _ = sqlx::query(
        "update ai_provider_configs set status = $2, last_error = $3, updated_at = now() where id = $1",
    )
    .bind(provider_id)
    .bind(status)
    .bind(error)
    .execute(db)
    .await;
}
