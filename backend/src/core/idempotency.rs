//! Идемпотентность опасных операций (PLAN §8.7).
//!
//! Клиент передаёт заголовок `Idempotency-Key`. Первый запрос выполняется,
//! его результат сохраняется; повтор с тем же ключом и тем же телом получает
//! сохранённый ответ и не меняет состояние. Другой хэш тела — ошибка `422`,
//! незавершённая параллельная операция — `409`.

use std::future::Future;

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::error::{ApiError, ApiResult};
use crate::api::ApiOk;
use crate::state::AppState;

/// Публичное имя заголовка.
pub const HEADER: &str = "idempotency-key";

/// Ключ должен быть достаточно коротким и безопасным для индекса.
pub const KEY_MAX: usize = 255;

/// Результат начала операции.
pub enum Outcome {
    /// Ключ новый — обработчик выполняет операцию.
    Fresh,
    /// Операция уже завершена: нужно вернуть сохранённый результат.
    Replay { status: u16, body: Value },
    /// Повтор невозможен (другой хэш или операция ещё выполняется).
    Conflict(ApiError),
}

/// Читает ключ из заголовков. Отсутствие заголовка — обычный запрос.
pub fn key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn validate_key(key: &str) -> ApiResult<()> {
    if key.is_empty() || key.len() > KEY_MAX {
        return Err(ApiError::validation(
            "Idempotency-Key должен содержать от 1 до 255 символов.",
        ));
    }
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return Err(ApiError::validation(
            "Idempotency-Key может содержать только ASCII-буквы, цифры, '-', '_', '.' и ':'.",
        ));
    }
    Ok(())
}

/// Стабильный хэш тела запроса: одинаковый запрос — одинаковый хэш.
pub fn hash_request<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hex::encode(hasher.finalize())
}

/// Начинает идемпотентную операцию.
pub async fn begin(
    db: &PgPool,
    actor_id: Uuid,
    scope: &str,
    key: &str,
    request_hash: &str,
) -> ApiResult<Outcome> {
    validate_key(key)?;

    let inserted = sqlx::query(
        "insert into idempotency_keys (key, scope, actor_id, request_hash)
         values ($1, $2, $3, $4)
         on conflict (actor_id, scope, key) do nothing",
    )
    .bind(key)
    .bind(scope)
    .bind(actor_id)
    .bind(request_hash)
    .execute(db)
    .await?
    .rows_affected();

    if inserted > 0 {
        return Ok(Outcome::Fresh);
    }

    let row: Option<(String, String, Option<i32>, Option<Value>)> = sqlx::query_as(
        "select request_hash, status, response_status, response_body
         from idempotency_keys
         where actor_id = $1 and scope = $2 and key = $3",
    )
    .bind(actor_id)
    .bind(scope)
    .bind(key)
    .fetch_optional(db)
    .await?;

    let Some((stored_hash, status, response_status, response_body)) = row else {
        // Строка исчезла между вставкой и чтением — считаем операцию новой.
        return Ok(Outcome::Fresh);
    };

    if stored_hash != request_hash {
        return Ok(Outcome::Conflict(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "IDEMPOTENCY_KEY_REUSED",
            "Этот Idempotency-Key уже использован для другого запроса.",
        )));
    }

    if status == "completed" {
        return Ok(Outcome::Replay {
            status: response_status.unwrap_or(200).clamp(100, 599) as u16,
            body: response_body.unwrap_or(Value::Null),
        });
    }

    Ok(Outcome::Conflict(ApiError::new(
        axum::http::StatusCode::CONFLICT,
        "IDEMPOTENCY_IN_PROGRESS",
        "Операция с этим Idempotency-Key уже выполняется. Повторите позже.",
    )))
}

/// Фиксирует успешный результат операции.
pub async fn complete(
    db: &PgPool,
    actor_id: Uuid,
    scope: &str,
    key: &str,
    status: u16,
    body: &Value,
) {
    let result = sqlx::query(
        "update idempotency_keys
         set status = 'completed', response_status = $4, response_body = $5, completed_at = now()
         where actor_id = $1 and scope = $2 and key = $3",
    )
    .bind(actor_id)
    .bind(scope)
    .bind(key)
    .bind(status as i32)
    .bind(body)
    .execute(db)
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, "не удалось сохранить результат идемпотентной операции");
    }
}

/// Отменяет бронь ключа при ошибке, чтобы клиент мог повторить запрос.
pub async fn release(db: &PgPool, actor_id: Uuid, scope: &str, key: &str) {
    let _ = sqlx::query(
        "delete from idempotency_keys
         where actor_id = $1 and scope = $2 and key = $3 and status = 'in_progress'",
    )
    .bind(actor_id)
    .bind(scope)
    .bind(key)
    .execute(db)
    .await;
}

/// Удаляет ключи старше суток и «зависшие» незавершённые брони.
pub async fn cleanup(db: &PgPool) -> ApiResult<u64> {
    let removed = sqlx::query(
        "delete from idempotency_keys
         where created_at < now() - interval '24 hours'",
    )
    .execute(db)
    .await?
    .rows_affected();
    Ok(removed)
}

/// Выполняет опасную операцию идемпотентно.
///
/// Единая точка для всех изменяющих эндпоинтов, которым нужен
/// `Idempotency-Key`: проверяет ключ, при первом запросе выполняет `fut`,
/// сохраняет результат и при повторе возвращает его без повторного изменения
/// состояния. Ошибка выполнения освобождает бронь, чтобы клиент мог повторить
/// запрос. Без заголовка операция выполняется как обычная.
///
/// `fut` возвращает только данные ответа (`data`); успешный статус — `200`,
/// как у всех подключённых операций (`PATCH`, `POST`).
pub async fn run_idempotent<F>(
    state: &AppState,
    actor_id: Uuid,
    scope: &str,
    headers: &HeaderMap,
    request_hash: String,
    fut: F,
) -> ApiResult<Response>
where
    F: Future<Output = ApiResult<Value>>,
{
    let Some(key) = key_from_headers(headers) else {
        return Ok(ApiOk(fut.await?).into_response());
    };

    match begin(&state.db, actor_id, scope, &key, &request_hash).await? {
        Outcome::Fresh => {}
        Outcome::Replay { status, body } => return Ok(replay_response(status, body)),
        Outcome::Conflict(error) => return Err(error),
    }

    let value = match fut.await {
        Ok(value) => value,
        Err(error) => {
            release(&state.db, actor_id, scope, &key).await;
            return Err(error);
        }
    };

    complete(&state.db, actor_id, scope, &key, 200, &value).await;
    Ok(ApiOk(value).into_response())
}

/// Готовый ответ на повтор: сериализованный JSON тела `ApiOk`.
pub fn replay_response(status: u16, body: Value) -> Response {
    let code = axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK);
    (code, axum::Json(json!({ "success": true, "data": body }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_rejects_unsafe_values() {
        assert!(validate_key("abc-123").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("with space").is_err());
        assert!(validate_key(&"x".repeat(KEY_MAX + 1)).is_err());
    }

    #[test]
    fn request_hash_is_stable() {
        let a = hash_request(&serde_json::json!({ "strategy": "replace" }));
        let b = hash_request(&serde_json::json!({ "strategy": "replace" }));
        let c = hash_request(&serde_json::json!({ "strategy": "auto" }));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }
}
