//! Мониторинг: текущие показатели, история из `system_metrics`, модульные метрики.

use std::collections::HashMap;

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::api::ApiOk;
use crate::auth::AuthUser;
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

const VIEW: &str = "system.monitoring.view";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/current", get(current))
        .route("/history", get(history))
        .route("/modules", get(modules))
        .route("/intervals", get(intervals))
}

/// Допустимые интервалы истории (минуты).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Min5,
    Min15,
    Hour1,
    Hour24,
    Days7,
}

impl Window {
    fn parse(raw: &str) -> ApiResult<Self> {
        match raw {
            "5m" | "" => Ok(Window::Min5),
            "15m" => Ok(Window::Min15),
            "1h" => Ok(Window::Hour1),
            "24h" => Ok(Window::Hour24),
            "7d" => Ok(Window::Days7),
            _ => Err(ApiError::validation(
                "Интервал должен быть 5m, 15m, 1h, 24h или 7d.",
            )),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Window::Min5 => "5m",
            Window::Min15 => "15m",
            Window::Hour1 => "1h",
            Window::Hour24 => "24h",
            Window::Days7 => "7d",
        }
    }

    fn duration(&self) -> Duration {
        match self {
            Window::Min5 => Duration::minutes(5),
            Window::Min15 => Duration::minutes(15),
            Window::Hour1 => Duration::hours(1),
            Window::Hour24 => Duration::hours(24),
            Window::Days7 => Duration::days(7),
        }
    }

    /// Размер бакета агрегации в секундах: чем длиннее интервал, тем крупнее шаг.
    fn bucket_seconds(&self) -> i64 {
        match self {
            Window::Min5 => 10,
            Window::Min15 => 20,
            Window::Hour1 => 60,
            Window::Hour24 => 600,
            Window::Days7 => 3600,
        }
    }
}

async fn intervals(
    State(_state): State<Arc<AppState>>,
    actor: AuthUser,
) -> ApiResult<ApiOk<Value>> {
    actor.require(VIEW)?;
    Ok(ApiOk(json!([
        { "key": "5m", "label": "5 минут", "seconds": 300 },
        { "key": "15m", "label": "15 минут", "seconds": 900 },
        { "key": "1h", "label": "1 час", "seconds": 3600 },
        { "key": "24h", "label": "24 часа", "seconds": 86400 },
        { "key": "7d", "label": "7 дней", "seconds": 604800 },
    ])))
}

async fn current(State(state): State<Arc<AppState>>, actor: AuthUser) -> ApiResult<ApiOk<Value>> {
    actor.require(VIEW)?;
    let metrics = state.metrics.snapshot();
    let stats = state.requests.snapshot();

    let sessions: i64 = sqlx::query_scalar(
        "select count(*) from sessions where revoked_at is null and expires_at > now()",
    )
    .fetch_one(&state.db)
    .await?;
    let users: i64 = sqlx::query_scalar("select count(*) from users where status = 'active'")
        .fetch_one(&state.db)
        .await?;
    let jobs_pending: i64 = sqlx::query_scalar(
        "select count(*) from background_jobs where status in ('pending', 'running', 'paused')",
    )
    .fetch_one(&state.db)
    .await?;
    let uploads_active: i64 = sqlx::query_scalar(
        "select
            (select count(*) from upload_sessions where status = 'active')
          + (select count(*) from minecraft_upload_sessions where status = 'active')",
    )
    .fetch_one(&state.db)
    .await?;

    let volumes: Vec<(String, String, bool, i64, i64)> = sqlx::query_as(
        "select name, state, enabled, total_bytes, available_bytes
         from storage_volumes order by name",
    )
    .fetch_all(&state.db)
    .await?;

    let (cache_hits, cache_misses) = state.session_cache_stats();

    Ok(ApiOk(json!({
        "capturedAt": metrics.captured_at,
        "uptimeSeconds": state.started_at.elapsed().as_secs(),
        "cpu": {
            "percent": metrics.cpu_percent,
            "cores": metrics.cpu_cores,
            "perCore": metrics.per_core,
            "loadAverage": metrics.load_average,
        },
        "memory": {
            "total": metrics.memory_total,
            "used": metrics.memory_used,
            "available": metrics.memory_available,
            "swapTotal": metrics.swap_total,
            "swapUsed": metrics.swap_used,
            "process": metrics.process_memory,
        },
        "network": {
            "receivedTotal": metrics.network_received,
            "transmittedTotal": metrics.network_transmitted,
            "rxRate": metrics.network_rx_rate,
            "txRate": metrics.network_tx_rate,
            "activeConnections": state.requests.active_connections() + state.realtime.active(),
            "activeConnectionsHint": "in-flight HTTP-запросы + WebSocket; TCP-соединения ОС не считаются",
        },
        "disks": metrics.disks,
        "volumes": volumes.iter().map(|(name, vol_state, enabled, total, available)| json!({
            "name": name,
            "state": vol_state,
            "enabled": enabled,
            "totalBytes": total,
            "availableBytes": available,
        })).collect::<Vec<_>>(),
        "app": {
            "sessions": sessions,
            "users": users,
            "requestsPerSecond": stats.requests_per_second,
            "averageLatencyMs": stats.average_latency_ms,
            "errorRate": stats.error_rate,
            "requestsLastMinute": stats.requests_last_minute,
            "errorsLastMinute": stats.errors_last_minute,
            "jobsPending": jobs_pending,
            "activeUploads": uploads_active,
            "activeDownloads": state.requests.active_downloads(),
            "activeWebSockets": state.realtime.active(),
            "sessionCache": {
                "hits": cache_hits,
                "misses": cache_misses,
                "hitRate": if cache_hits + cache_misses == 0 {
                    0.0
                } else {
                    cache_hits as f64 / (cache_hits + cache_misses) as f64
                },
            },
            "database": "up",
            "hostname": metrics.hostname,
            "os": metrics.os_name,
            "osVersion": metrics.os_version,
            "kernel": metrics.kernel_version,
        },
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryQuery {
    window: Option<String>,
}

async fn history(
    State(state): State<Arc<AppState>>,
    actor: AuthUser,
    query: Result<Query<HistoryQuery>, QueryRejection>,
) -> ApiResult<ApiOk<Value>> {
    actor.require(VIEW)?;
    let query = query
        .map(|Query(value)| value)
        .map_err(|error| ApiError::validation(format!("Некорректные параметры: {error}")))?;
    let window = Window::parse(query.window.as_deref().unwrap_or("5m"))?;
    let bucket = window.bucket_seconds();
    let since = Utc::now() - window.duration();

    let rows: Vec<(String, DateTime<Utc>, f64)> = sqlx::query_as(
        "select metric,
                to_timestamp(floor(extract(epoch from captured_at) / $2) * $2) as bucket,
                avg(value) as value
         from system_metrics
         where captured_at >= $1
         group by metric, bucket
         order by bucket",
    )
    .bind(since)
    .bind(bucket as f64)
    .fetch_all(&state.db)
    .await?;

    let mut series: HashMap<String, Vec<Value>> = HashMap::new();
    for (metric, bucket_at, value) in rows {
        series.entry(metric).or_default().push(json!({
            "at": bucket_at,
            "value": value,
        }));
    }

    Ok(ApiOk(json!({
        "window": window.label(),
        "bucketSeconds": bucket,
        "since": since,
        "series": series,
    })))
}

/// Реальные счётчики модулей: сборки, файлы, объекты, ошибки операций.
async fn modules(State(state): State<Arc<AppState>>, actor: AuthUser) -> ApiResult<ApiOk<Value>> {
    actor.require(VIEW)?;
    let db = &state.db;
    let metrics = state.metrics.snapshot();

    let builds: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint,
                count(*) filter (where status = 'published')::bigint
         from minecraft_builds",
    )
    .fetch_one(db)
    .await?;
    let build_bytes: i64 = sqlx::query_scalar(
        "select coalesce(sum(size_bytes), 0)::bigint
         from minecraft_build_files where kind = 'file'",
    )
    .fetch_one(db)
    .await?;
    let build_files: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(size_bytes), 0)::bigint
         from minecraft_build_files where kind = 'file'",
    )
    .fetch_one(db)
    .await?;
    let releases: i64 = sqlx::query_scalar("select count(*) from minecraft_releases")
        .fetch_one(db)
        .await?;
    let cloud: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(size_bytes), 0)::bigint
         from cloud_entries where entry_type = 'file' and trashed_at is null",
    )
    .fetch_one(db)
    .await?;
    let cloud_shares: i64 = sqlx::query_scalar(
        "select count(*) from cloud_shares where revoked_at is null and (expires_at is null or expires_at > now())",
    )
    .fetch_one(db)
    .await?;
    let objects: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(size_bytes), 0)::bigint
         from storage_objects where state = 'committed'",
    )
    .fetch_one(db)
    .await?;
    let objects_missing: i64 =
        sqlx::query_scalar("select count(*) from storage_objects where state <> 'committed'")
            .fetch_one(db)
            .await?;
    let errors_24h: i64 = sqlx::query_scalar(
        "select count(*) from audit_events
         where result = 'failure' and created_at > now() - interval '24 hours'",
    )
    .fetch_one(db)
    .await?;
    let downloads_24h: (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(bytes_sent), 0)::bigint
         from launcher_downloads where created_at > now() - interval '24 hours'",
    )
    .fetch_one(db)
    .await?;

    let module_flags: Vec<(String, bool)> =
        sqlx::query_as("select key, enabled from module_states order by key")
            .fetch_all(db)
            .await?;

    // Скорости по модулям за последний час по журналу событий.
    let module_rates: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "select
            case
              when event_type like 'cloud.%' then 'cloud'
              when event_type like 'minecraft.%' or event_type like 'launcher.%' then 'minecraft'
              else 'system'
            end as module,
            count(*) filter (where event_type like '%uploaded' or event_type like '%artifact.uploaded')::bigint as uploads,
            count(*) filter (where event_type like '%downloaded')::bigint as downloads,
            count(*) filter (where result = 'failure')::bigint as errors
         from audit_events
         where created_at > now() - interval '24 hours'
         group by module",
    )
    .fetch_all(db)
    .await?;

    let stats = state.requests.snapshot();

    Ok(ApiOk(json!({
        "minecraft": {
            "enabled": module_flags.iter().any(|(k, e)| k == "minecraft" && *e),
            "builds": builds.0,
            "published": builds.1,
            "buildBytes": build_bytes,
            "files": build_files.0,
            "fileBytes": build_files.1,
            "releases": releases,
            "uploads24h": module_rates.iter().find(|(m, _, _, _)| m == "minecraft").map(|(_, u, _, _)| *u).unwrap_or(0),
            "downloads24h": module_rates.iter().find(|(m, _, _, _)| m == "minecraft").map(|(_, _, d, _)| *d).unwrap_or(0),
            "errors24h": module_rates.iter().find(|(m, _, _, _)| m == "minecraft").map(|(_, _, _, e)| *e).unwrap_or(0),
        },
        "cloud": {
            "enabled": module_flags.iter().any(|(k, e)| k == "cloud" && *e),
            "files": cloud.0,
            "bytes": cloud.1,
            "activeShares": cloud_shares,
            "uploads24h": module_rates.iter().find(|(m, _, _, _)| m == "cloud").map(|(_, u, _, _)| *u).unwrap_or(0),
            "downloads24h": module_rates.iter().find(|(m, _, _, _)| m == "cloud").map(|(_, _, d, _)| *d).unwrap_or(0),
            "errors24h": module_rates.iter().find(|(m, _, _, _)| m == "cloud").map(|(_, _, _, e)| *e).unwrap_or(0),
            "uploadRateBps": stats.requests_per_second,
            "activeDownloads": state.requests.active_downloads(),
        },
        "storage": {
            "objects": objects.0,
            "bytes": objects.1,
            "unavailableObjects": objects_missing,
            "processReadBytesPerSec": metrics.process_read_bytes_per_sec,
            "processWriteBytesPerSec": metrics.process_write_bytes_per_sec,
            "diskIoNote": "скорость чтения/записи процесса NEXUS",
        },
        "errors24h": errors_24h,
        "launcher": {
            "downloads24h": downloads_24h.0,
            "bytes24h": downloads_24h.1,
        },
        "connections": {
            "activeRequests": state.requests.active_connections(),
            "activeDownloads": state.requests.active_downloads(),
            "activeWebSockets": state.realtime.active(),
        },
    })))
}
