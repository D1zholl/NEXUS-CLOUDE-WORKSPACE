//! Фоновые задачи: список, пауза/возобновление, отмена, постановка задач.

use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

const VIEW: &str = "system.view";
const MANAGE: &str = "system.jobs.manage";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_jobs))
        .route("/{id}", get(get_job))
        .route("/{id}/pause", post(pause_job))
        .route("/{id}/resume", post(resume_job))
        .route("/{id}/cancel", post(cancel_job))
        .route("/maintenance", post(run_maintenance))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: Uuid,
    pub job_type: String,
    pub status: String,
    pub progress: f64,
    pub total_units: Option<i64>,
    pub done_units: Option<i64>,
    pub error: Option<String>,
    pub payload: Value,
    pub cancellable: bool,
    pub pause_requested: bool,
    pub paused_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub actor: Option<String>,
}

const JOB_SELECT: &str = "select j.id, j.job_type, j.status, j.progress, j.total_units,
        j.done_units, j.error, j.payload, j.cancellable, j.pause_requested, j.paused_at,
        j.created_at, j.started_at, j.finished_at, u.username as actor
     from background_jobs j
     left join users u on u.id = j.created_by";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    status: Option<String>,
    job_type: Option<String>,
    limit: Option<i64>,
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require(VIEW)?;
    let query = query
        .map(|Query(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректные параметры: {error}")))?;

    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(status) = query.status.as_deref().filter(|v| !v.trim().is_empty()) {
        binds.push(status.to_owned());
        conditions.push(format!("j.status = ${}", binds.len()));
    }
    if let Some(job_type) = query.job_type.as_deref().filter(|v| !v.trim().is_empty()) {
        binds.push(job_type.to_owned());
        conditions.push(format!("j.job_type = ${}", binds.len()));
    }
    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!(" where {}", conditions.join(" and "))
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let sql = format!(
        "{JOB_SELECT}{where_sql} order by j.created_at desc limit ${}",
        binds.len() + 1
    );
    let mut rows = sqlx::query_as::<_, JobRow>(&sql);
    for bind in &binds {
        rows = rows.bind(bind);
    }
    let items = rows.bind(limit).fetch_all(&state.db).await?;

    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "select count(*) filter (where status = 'pending')::bigint,
                count(*) filter (where status = 'running')::bigint,
                count(*) filter (where status = 'paused')::bigint,
                count(*) filter (where status = 'failed')::bigint,
                count(*) filter (where status = 'completed')::bigint
         from background_jobs",
    )
    .fetch_one(&state.db)
    .await?;

    let max_concurrent = state.jobs_max_concurrent().await;

    Ok(ApiOk(json!({
        "items": items,
        "pending": counts.0,
        "running": counts.1,
        "paused": counts.2,
        "failed": counts.3,
        "completed": counts.4,
        "maxConcurrent": max_concurrent,
    })))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<JobRow>> {
    actor.require(VIEW)?;
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))?;
    sqlx::query_as::<_, JobRow>(&format!("{JOB_SELECT} where j.id = $1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .map(ApiOk)
        .ok_or_else(|| ApiError::not_found("Задача не найдена."))
}

/// Приостанавливает задачу.
///
/// `pending` переводится в `paused` сразу; `running` получает `pause_requested`
/// и останавливается на границе текущей единицы работы, после чего исполнитель
/// выставляет `paused`, `paused_at` и освобождает слот планировщика.
async fn pause_job(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<JobRow>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))?;

    let row: Option<(String, bool)> =
        sqlx::query_as("select status, cancellable from background_jobs where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((status, cancellable)) = row else {
        return Err(ApiError::not_found("Задача не найдена."));
    };
    if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(ApiError::conflict("JOB_FINISHED", "Задача уже завершена."));
    }
    if status == "paused" {
        return Err(ApiError::conflict(
            "JOB_ALREADY_PAUSED",
            "Задача уже на паузе.",
        ));
    }
    if !cancellable {
        return Err(ApiError::conflict(
            "JOB_NOT_PAUSABLE",
            "Эту задачу нельзя приостановить.",
        ));
    }

    let paused_at = Utc::now();
    let updated_status: Option<String> = if status == "pending" {
        sqlx::query_scalar(
            "update background_jobs
                set status = 'paused', paused_at = $2, pause_requested = false, updated_at = now()
              where id = $1 and status = 'pending'
              returning status",
        )
        .bind(id)
        .bind(paused_at)
        .fetch_optional(&state.db)
        .await?
    } else {
        sqlx::query_scalar(
            "update background_jobs
                set pause_requested = true, updated_at = now()
              where id = $1 and status = 'running'
              returning status",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await?
    };
    let Some(updated_status) = updated_status else {
        return Err(ApiError::conflict(
            "JOB_STATE_CHANGED",
            "Состояние задачи изменилось. Обновите список.",
        ));
    };

    if updated_status == "paused" {
        // Для `running` событие публикует исполнитель в момент фактической паузы,
        // чтобы клиент не показывал «Пауза» до освобождения слота.
        state
            .realtime
            .publish_system("job.paused", json!({ "jobId": id, "pausedAt": paused_at }));
    }

    record(
        &state.db,
        AuditEvent {
            event_type: "jobs.pause_requested",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "job",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "from": status, "immediate": updated_status == "paused" }),
        },
    )
    .await;

    let row = sqlx::query_as::<_, JobRow>(&format!("{JOB_SELECT} where j.id = $1"))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(ApiOk(row))
}

/// Возвращает задачу из `paused` в очередь: сбрасывает флаг и время паузы.
async fn resume_job(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<JobRow>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))?;

    let status: Option<String> =
        sqlx::query_scalar("select status from background_jobs where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some(status) = status else {
        return Err(ApiError::not_found("Задача не найдена."));
    };
    if status != "paused" {
        return Err(ApiError::conflict(
            "JOB_NOT_PAUSED",
            "Возобновить можно только задачу на паузе.",
        ));
    }

    let resumed: Option<Uuid> = sqlx::query_scalar(
        "update background_jobs
            set status = 'pending', pause_requested = false, paused_at = null, updated_at = now()
          where id = $1 and status = 'paused'
          returning id",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    if resumed.is_none() {
        return Err(ApiError::conflict(
            "JOB_STATE_CHANGED",
            "Состояние задачи изменилось. Обновите список.",
        ));
    }

    state
        .realtime
        .publish_system("job.resumed", json!({ "jobId": id }));

    record(
        &state.db,
        AuditEvent {
            event_type: "jobs.resumed",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "job",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;

    let row = sqlx::query_as::<_, JobRow>(&format!("{JOB_SELECT} where j.id = $1"))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(ApiOk(row))
}

/// Запрашивает отмену задачи. Обработчик задачи проверяет флаг и завершается.
/// Задача на паузе отменяется сразу, без ожидания исполнителя.
async fn cancel_job(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    path: Result<Path<Uuid>, PathRejection>,
) -> ApiResult<ApiOk<JobRow>> {
    actor.require(MANAGE)?;
    let id = path
        .map(|Path(id)| id)
        .map_err(|_| ApiError::validation("Некорректный идентификатор."))?;

    let row: Option<(String, bool)> =
        sqlx::query_as("select status, cancellable from background_jobs where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((status, cancellable)) = row else {
        return Err(ApiError::not_found("Задача не найдена."));
    };
    if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(ApiError::conflict("JOB_FINISHED", "Задача уже завершена."));
    }
    if !cancellable {
        return Err(ApiError::conflict(
            "JOB_NOT_CANCELLABLE",
            "Эту задачу нельзя прервать.",
        ));
    }

    let cancelled_status: Option<String> = sqlx::query_scalar(
        "update background_jobs
         set cancel_requested = true,
             pause_requested = false,
             status = case when status in ('pending', 'paused') then 'cancelled' else status end,
             finished_at = case when status in ('pending', 'paused') then now() else finished_at end,
             updated_at = now()
         where id = $1
         returning status",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    if cancelled_status.as_deref() == Some("cancelled") {
        state
            .realtime
            .publish_system("job.cancelled", json!({ "jobId": id }));
    }

    record(
        &state.db,
        AuditEvent {
            event_type: "jobs.cancel_requested",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "job",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({}),
        },
    )
    .await;

    let row = sqlx::query_as::<_, JobRow>(&format!("{JOB_SELECT} where j.id = $1"))
        .bind(id)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;
    Ok(ApiOk(row))
}

/// Ставит системную задачу обслуживания (выполняется воркером).
async fn run_maintenance(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    body: Result<axum::Json<MaintenanceRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require(MANAGE)?;
    let body = body
        .map(|axum::Json(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректное тело запроса: {error}")))?;

    let (job_type, label) = match body.kind.as_str() {
        "storage_check" => ("system.storage.check", "Проверка целостности объектов"),
        "verify_objects" => ("system.objects.verify", "Сверка SHA-256 объектов"),
        "cleanup_uploads" => ("system.uploads.cleanup", "Очистка временных загрузок"),
        "cleanup_shares" => ("system.shares.cleanup", "Удаление истёкших ссылок"),
        "cleanup_trash" => ("system.trash.cleanup", "Автоочистка корзины"),
        "metrics_prune" => ("system.metrics.prune", "Очистка старых метрик"),
        "manifests_regenerate" => (
            "system.manifests.regenerate",
            "Перегенерация манифестов релизов",
        ),
        other => {
            return Err(ApiError::validation(format!(
                "Неизвестный тип обслуживания: {other}"
            )));
        }
    };

    // Режим сверки SHA относится только к system.objects.verify.
    let mut payload = json!({ "kind": body.kind, "label": label });
    if job_type == "system.objects.verify" {
        let mode = body.mode.as_deref().unwrap_or("sample");
        if !matches!(mode, "full" | "sample") {
            return Err(ApiError::validation(
                "Режим сверки должен быть full или sample.",
            ));
        }
        let percent = body.sample_percent.unwrap_or(10);
        if !(1..=100).contains(&percent) {
            return Err(ApiError::validation(
                "Доля выборки должна быть от 1 до 100 процентов.",
            ));
        }
        payload = json!({
            "kind": body.kind,
            "label": label,
            "mode": mode,
            "samplePercent": percent,
        });
    }

    let id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, created_by)
         values ($1, 'pending', $2, true, $3)
         returning id",
    )
    .bind(job_type)
    .bind(payload)
    .bind(actor.id)
    .fetch_one(&state.db)
    .await?;

    // Очередь обрабатывает планировщик (system/scheduler.rs): задача больше
    // не может «застрять» в pending из-за занятого лимита.
    record(
        &state.db,
        AuditEvent {
            event_type: "jobs.maintenance_started",
            actor_id: Some(actor.id),
            actor_username: &actor.username,
            target_type: "job",
            target_id: &id.to_string(),
            result: "success",
            ip: &actor.ip,
            details: json!({ "kind": body.kind }),
        },
    )
    .await;

    Ok(ApiOk(json!({ "id": id, "jobType": job_type })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaintenanceRequest {
    kind: String,
    /// system.objects.verify: `full` — все объекты, `sample` — выборка.
    mode: Option<String>,
    /// Доля выборки в процентах (1–100), по умолчанию 10.
    sample_percent: Option<i64>,
}
