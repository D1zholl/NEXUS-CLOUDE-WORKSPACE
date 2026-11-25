//! Транзакционное резервирование квот AI Workspace.
//!
//! Измеренный `ai_workspaces.usage_*` — это снимок на момент последней
//! операции. Перед изменяющей операцией backend под advisory-lock пользователя
//! проверяет `usage + активные резервы + ожидаемое изменение` и вставляет
//! резерв. Поэтому два параллельных запроса (и разные соединения Runtime)
//! видят резервы друг друга и не могут вместе превысить лимит.
//!
//! После подтверждения Runtime фактический usage записывается в workspace,
//! резерв переводится в `confirmed`. При известной ошибке — `released`.
//! При неизвестном результате (`AI_EXECUTION_UNKNOWN`, timeout) резерв
//! остаётся `uncertain`: место не освобождается, пока администратор или
//! последующий замер не подтвердят состояние.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ai::models::WorkspaceRow;
use crate::ai::runtime::protocol::WorkspaceUsage;
use crate::ai::{tools, workspace_files, workspaces};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Резерв живёт дольше максимального времени инструмента (600 с) и grace.
const RESERVATION_TTL_SECONDS: i64 = 20 * 60;
/// Неизвестный результат не освобождает место автоматически; через 7 дней
/// запись помечается expired, чтобы квота не была заблокирована навсегда.
const UNCERTAIN_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct Reservation {
    pub id: Uuid,
    pub operation_id: String,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub bytes: i64,
    pub files: i32,
}

#[derive(Debug)]
enum Estimate {
    Delta { bytes: i64, files: i64 },
    Remaining,
}

fn is_mutating(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "fs.write"
            | "fs.write_base64"
            | "fs.copy"
            | "fs.move"
            | "fs.delete"
            | "fs.mkdir"
            | "fs.apply_patch"
            | "program.run"
    ) || tool_id.starts_with("mcp.")
}

/// Оценивает верхнюю границу изменения workspace. Для неизвестных MCP и
/// длительных процессов резервируется всё оставшееся место workspace.
pub async fn reserve_for_tool(
    state: &AppState,
    workspace: &WorkspaceRow,
    tool_id: &str,
    arguments: &Value,
    operation_id: &str,
) -> ApiResult<Option<Reservation>> {
    if !is_mutating(tool_id) {
        return Ok(None);
    }
    let estimate = estimate_change(state, workspace, tool_id, arguments).await?;
    reserve(state, workspace, tool_id, operation_id, estimate, arguments).await
}

async fn estimate_change(
    state: &AppState,
    workspace: &WorkspaceRow,
    tool_id: &str,
    arguments: &Value,
) -> ApiResult<Estimate> {
    match tool_id {
        "fs.write" => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let size = arguments
                .get("content")
                .and_then(Value::as_str)
                .map(|value| value.len() as i64)
                .unwrap_or(0);
            let (exists, old) = target_info(state, workspace, path).await;
            Ok(Estimate::Delta {
                bytes: (size - old).max(0),
                files: if exists { 0 } else { 1 },
            })
        }
        "fs.write_base64" => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let encoded = arguments
                .get("contentBase64")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let size = BASE64
                .decode(encoded)
                .map(|bytes| bytes.len() as i64)
                .unwrap_or_else(|_| (encoded.len() as i64 * 3) / 4);
            let (exists, old) = target_info(state, workspace, path).await;
            Ok(Estimate::Delta {
                bytes: (size - old).max(0),
                files: if exists { 0 } else { 1 },
            })
        }
        "fs.copy" => {
            let from = arguments
                .get("from")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let to = arguments
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (_, source) = target_info(state, workspace, from).await;
            let (target_exists, target_size) = target_info(state, workspace, to).await;
            Ok(Estimate::Delta {
                bytes: (source - target_size).max(0),
                files: if target_exists { 0 } else { 1 },
            })
        }
        "fs.move" => {
            // Перемещение внутри workspace не меняет суммарный размер.
            Ok(Estimate::Delta { bytes: 0, files: 0 })
        }
        "fs.delete" | "fs.mkdir" => Ok(Estimate::Delta { bytes: 0, files: 0 }),
        "fs.apply_patch" => {
            let patch = arguments
                .get("patch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (added, new_files) = patch_estimate(patch);
            let mut files = 0;
            for path in new_files {
                let (exists, _) = target_info(state, workspace, &path).await;
                if !exists {
                    files += 1;
                }
            }
            Ok(Estimate::Delta {
                bytes: added,
                files,
            })
        }
        "program.run" => Ok(Estimate::Remaining),
        other if other.starts_with("mcp.") => {
            // MCP-инструмент может менять workspace, но его схема не описывает
            // объём. Безопасный отказ от конкурентной записи: резервируем всё
            // оставшееся место на время вызова.
            Ok(Estimate::Remaining)
        }
        _ => Ok(Estimate::Delta { bytes: 0, files: 0 }),
    }
}

async fn target_info(state: &AppState, workspace: &WorkspaceRow, path: &str) -> (bool, i64) {
    if path.trim().is_empty() {
        return (false, 0);
    }
    match workspace_files::execute_tool(state, workspace, "fs.stat", json!({ "path": path })).await
    {
        Ok(outcome) => (
            true,
            outcome
                .result
                .get("size")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        ),
        Err(error) if error.code == "AI_FILE_NOT_FOUND" => (false, 0),
        // Ошибка чтения метаданных трактуется консервативно: считаем, что файла
        // нет, и резервируем полный размер + один файл.
        Err(_) => (false, 0),
    }
}

/// Верхняя граница прироста от unified diff и список затронутых путей.
fn patch_estimate(patch: &str) -> (i64, Vec<String>) {
    let mut added = 0i64;
    let mut files = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            let raw = path.split('\t').next().unwrap_or("").trim();
            if raw == "/dev/null" {
                continue;
            }
            let normalized = raw
                .strip_prefix("b/")
                .or_else(|| raw.strip_prefix("a/"))
                .unwrap_or(raw)
                .trim_start_matches("./")
                .replace('\\', "/");
            if !normalized.is_empty() && !files.contains(&normalized) {
                files.push(normalized);
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += line.len().saturating_sub(1) as i64 + 1;
        }
    }
    (added, files)
}

/// Резервирование явно заданного изменения. Используется тестами и
/// административными сценариями; обычные операции вычисляют оценку сами.
pub async fn reserve_delta(
    state: &AppState,
    workspace: &WorkspaceRow,
    kind: &str,
    operation_id: &str,
    bytes: i64,
    files: i32,
) -> ApiResult<Option<Reservation>> {
    reserve(
        state,
        workspace,
        kind,
        operation_id,
        Estimate::Delta {
            bytes: bytes.max(0),
            files: files.max(0) as i64,
        },
        &json!({}),
    )
    .await
}

/// Проверяет квоты и вставляет резерв. Возвращает `None`, если у пользователя
/// и workspace нет числовых лимитов.
async fn reserve(
    state: &AppState,
    workspace: &WorkspaceRow,
    kind: &str,
    operation_id: &str,
    estimate: Estimate,
    arguments: &Value,
) -> ApiResult<Option<Reservation>> {
    workspaces::ensure_quota(&state.db, workspace.owner_id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("nexus.ai.quota:{}", workspace.owner_id))
        .execute(&mut *tx)
        .await?;

    // Истёкшие резервы не должны блокировать новые операции.
    sqlx::query(
        "update ai_quota_reservations
            set status = 'expired', resolved_at = now()
          where user_id = $1 and status = 'active' and expires_at < now()",
    )
    .bind(workspace.owner_id)
    .execute(&mut *tx)
    .await?;

    if let Some(existing) = load_reservation(&mut tx, operation_id).await? {
        // Повторная доставка операции: тот же резерв, без двойного списания.
        if matches!(existing.status.as_str(), "active" | "confirmed") {
            tx.commit().await?;
            if existing.status == "confirmed" {
                return Ok(None);
            }
            return Ok(Some(existing.into_reservation()));
        }
        // После известной ошибки повторная попытка может выполниться снова.
        sqlx::query(
            "update ai_quota_reservations
                set status = 'active', bytes = 0, files = 0,
                    expires_at = now() + make_interval(secs => $2), resolved_at = null
              where id = $1",
        )
        .bind(existing.id)
        .bind(RESERVATION_TTL_SECONDS as f64)
        .execute(&mut *tx)
        .await?;
    }

    let quota = sqlx::query_as::<_, (i64, i32, i64, i64)>(
        "select quota_bytes, max_files, usage_bytes, usage_files
           from ai_workspaces
          where id = $1 and deleted_at is null and status = 'active'
          for update",
    )
    .bind(workspace.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::conflict(
            "AI_WORKSPACE_UNAVAILABLE",
            "Рабочее пространство удалено или приостановлено.",
        )
    })?;
    let user = sqlx::query_as::<_, (i64, i32, i64)>(
        "select max_storage_bytes, max_files, max_file_bytes
           from ai_user_quotas where user_id = $1 for update",
    )
    .bind(workspace.owner_id)
    .fetch_one(&mut *tx)
    .await?;

    if quota.0 == 0 && quota.1 == 0 && user.0 == 0 && user.1 == 0 {
        // Ни у workspace, ни у пользователя нет числовых лимитов.
        tx.commit().await?;
        return Ok(None);
    }

    let (ws_res_bytes, ws_res_files): (i64, i64) = sqlx::query_as(
        "select coalesce(sum(bytes), 0)::bigint, coalesce(sum(files), 0)::bigint
           from ai_quota_reservations
          where workspace_id = $1 and status in ('active', 'uncertain')
            and (status = 'uncertain' or expires_at > now())",
    )
    .bind(workspace.id)
    .fetch_one(&mut *tx)
    .await?;
    let (user_res_bytes, user_res_files): (i64, i64) = sqlx::query_as(
        "select coalesce(sum(bytes), 0)::bigint, coalesce(sum(files), 0)::bigint
           from ai_quota_reservations
          where user_id = $1 and status in ('active', 'uncertain')
            and (status = 'uncertain' or expires_at > now())",
    )
    .bind(workspace.owner_id)
    .fetch_one(&mut *tx)
    .await?;
    let (user_used_bytes, user_used_files): (i64, i64) = sqlx::query_as(
        "select coalesce(sum(usage_bytes), 0)::bigint, coalesce(sum(usage_files), 0)::bigint
           from ai_workspaces where owner_id = $1 and deleted_at is null",
    )
    .bind(workspace.owner_id)
    .fetch_one(&mut *tx)
    .await?;

    let used_ws_bytes = quota.2 + ws_res_bytes;
    let used_ws_files = quota.3 + ws_res_files;
    let used_user_bytes = user_used_bytes + user_res_bytes;
    let used_user_files = user_used_files + user_res_files;

    let (bytes, files) = match estimate {
        Estimate::Delta { bytes, files } => (bytes.max(0), files.max(0)),
        Estimate::Remaining => {
            let ws_left = if quota.0 > 0 {
                (quota.0 - used_ws_bytes).max(0)
            } else {
                i64::MAX
            };
            let user_left = if user.0 > 0 {
                (user.0 - used_user_bytes).max(0)
            } else {
                i64::MAX
            };
            let files_ws = if quota.1 > 0 {
                (quota.1 as i64 - used_ws_files).max(0)
            } else {
                i64::MAX
            };
            let files_user = if user.1 > 0 {
                (user.1 as i64 - used_user_files).max(0)
            } else {
                i64::MAX
            };
            let bytes = ws_left.min(user_left);
            let files = files_ws.min(files_user);
            if bytes == i64::MAX && files == i64::MAX {
                (0, 0)
            } else {
                (
                    if bytes == i64::MAX { 0 } else { bytes },
                    if files == i64::MAX { 0 } else { files },
                )
            }
        }
    };

    if quota.0 > 0 && used_ws_bytes + bytes > quota.0 {
        record_reject(
            &mut tx,
            workspace,
            kind,
            bytes,
            files,
            used_ws_bytes,
            used_ws_files,
            quota.0,
            quota.1,
            "AI_STORAGE_QUOTA_EXCEEDED",
        )
        .await?;
        tx.commit().await?;
        return Err(ApiError::new(
            axum::http::StatusCode::INSUFFICIENT_STORAGE,
            "AI_STORAGE_QUOTA_EXCEEDED",
            "Операция превышает лимит хранилища workspace.",
        )
        .with_details(json!({
            "usedBytes": used_ws_bytes,
            "requestedBytes": bytes,
            "limitBytes": quota.0,
        })));
    }
    if quota.1 > 0 && used_ws_files + files > quota.1 as i64 {
        record_reject(
            &mut tx,
            workspace,
            kind,
            bytes,
            files,
            used_ws_bytes,
            used_ws_files,
            quota.0,
            quota.1,
            "AI_FILES_QUOTA_EXCEEDED",
        )
        .await?;
        tx.commit().await?;
        return Err(ApiError::new(
            axum::http::StatusCode::INSUFFICIENT_STORAGE,
            "AI_FILES_QUOTA_EXCEEDED",
            "Операция превышает лимит файлов workspace.",
        )
        .with_details(json!({
            "usedFiles": used_ws_files,
            "requestedFiles": files,
            "limitFiles": quota.1,
        })));
    }
    if user.0 > 0 && used_user_bytes + bytes > user.0 {
        record_reject(
            &mut tx,
            workspace,
            kind,
            bytes,
            files,
            used_user_bytes,
            used_user_files,
            user.0,
            user.1,
            "AI_STORAGE_QUOTA_EXCEEDED",
        )
        .await?;
        tx.commit().await?;
        return Err(ApiError::new(
            axum::http::StatusCode::INSUFFICIENT_STORAGE,
            "AI_STORAGE_QUOTA_EXCEEDED",
            "Операция превышает пользовательский лимит хранилища.",
        )
        .with_details(json!({
            "usedBytes": used_user_bytes,
            "requestedBytes": bytes,
            "limitBytes": user.0,
        })));
    }
    if user.1 > 0 && used_user_files + files > user.1 as i64 {
        record_reject(
            &mut tx,
            workspace,
            kind,
            bytes,
            files,
            used_user_bytes,
            used_user_files,
            user.0,
            user.1,
            "AI_FILES_QUOTA_EXCEEDED",
        )
        .await?;
        tx.commit().await?;
        return Err(ApiError::new(
            axum::http::StatusCode::INSUFFICIENT_STORAGE,
            "AI_FILES_QUOTA_EXCEEDED",
            "Операция превышает пользовательский лимит файлов.",
        )
        .with_details(json!({
            "usedFiles": used_user_files,
            "requestedFiles": files,
            "limitFiles": user.1,
        })));
    }

    let id: Uuid = sqlx::query_scalar(
        "insert into ai_quota_reservations
            (operation_id, user_id, workspace_id, kind, bytes, files, status,
             metadata, expires_at)
         values ($1, $2, $3, $4, $5, $6, 'active', $7,
                 now() + make_interval(secs => $8))
         on conflict (operation_id) do update
            set bytes = excluded.bytes, files = excluded.files,
                status = 'active', expires_at = excluded.expires_at,
                resolved_at = null, metadata = excluded.metadata
         returning id",
    )
    .bind(operation_id)
    .bind(workspace.owner_id)
    .bind(workspace.id)
    .bind(kind)
    .bind(bytes)
    .bind(files as i32)
    .bind(json!({
        "tool": kind,
        "argumentsPreview": arguments_preview(arguments),
    }))
    .bind(RESERVATION_TTL_SECONDS as f64)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "insert into ai_quota_events
            (reservation_id, workspace_id, user_id, event, requested_bytes,
             requested_files, used_bytes, used_files, limit_bytes, limit_files)
         values ($1, $2, $3, 'reserved', $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(workspace.id)
    .bind(workspace.owner_id)
    .bind(bytes)
    .bind(files as i32)
    .bind(used_ws_bytes)
    .bind(used_ws_files as i32)
    .bind(quota.0)
    .bind(quota.1)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(Reservation {
        id,
        operation_id: operation_id.to_owned(),
        user_id: workspace.owner_id,
        workspace_id: workspace.id,
        kind: kind.to_owned(),
        bytes,
        files: files as i32,
    }))
}

fn arguments_preview(arguments: &Value) -> Value {
    let mut preview = arguments.clone();
    if let Some(object) = preview.as_object_mut() {
        for key in ["content", "contentBase64", "patch"] {
            if let Some(value) = object.get_mut(key) {
                let size = value.as_str().map(str::len).unwrap_or(0);
                *value = Value::String(format!("<{size} bytes>"));
            }
        }
    }
    preview
}

#[derive(Debug)]
struct ExistingReservation {
    id: Uuid,
    operation_id: String,
    user_id: Uuid,
    workspace_id: Uuid,
    kind: String,
    bytes: i64,
    files: i32,
    status: String,
}

impl ExistingReservation {
    fn into_reservation(self) -> Reservation {
        Reservation {
            id: self.id,
            operation_id: self.operation_id,
            user_id: self.user_id,
            workspace_id: self.workspace_id,
            kind: self.kind,
            bytes: self.bytes,
            files: self.files,
        }
    }
}

async fn load_reservation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: &str,
) -> ApiResult<Option<ExistingReservation>> {
    let row = sqlx::query_as::<_, (Uuid, String, Uuid, Uuid, String, i64, i32, String)>(
        "select id, operation_id, user_id, workspace_id, kind, bytes, files, status
           from ai_quota_reservations where operation_id = $1 for update",
    )
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(id, operation_id, user_id, workspace_id, kind, bytes, files, status)| {
            ExistingReservation {
                id,
                operation_id,
                user_id,
                workspace_id,
                kind,
                bytes,
                files,
                status,
            }
        },
    ))
}

#[allow(clippy::too_many_arguments)]
async fn record_reject(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: &WorkspaceRow,
    kind: &str,
    bytes: i64,
    files: i64,
    used_bytes: i64,
    used_files: i64,
    limit_bytes: i64,
    limit_files: i32,
    code: &str,
) -> ApiResult<()> {
    sqlx::query(
        "insert into ai_quota_events
            (workspace_id, user_id, event, requested_bytes, requested_files,
             used_bytes, used_files, limit_bytes, limit_files, error)
         values ($1, $2, 'rejected', $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(workspace.id)
    .bind(workspace.owner_id)
    .bind(bytes)
    .bind(files as i32)
    .bind(used_bytes)
    .bind(used_files as i32)
    .bind(limit_bytes)
    .bind(limit_files)
    .bind(code)
    .execute(&mut **tx)
    .await?;
    let _ = kind;
    Ok(())
}

/// Подтверждает резерв фактическим usage и освобождает избыток.
pub async fn confirm(
    state: &AppState,
    reservation: Reservation,
    usage: Option<&WorkspaceUsage>,
) -> ApiResult<()> {
    match usage {
        Some(usage) if usage.error.is_none() => {
            let workspace = workspaces::load_workspace(&state.db, reservation.workspace_id).await?;
            workspaces::record_usage(
                &state.db,
                reservation.workspace_id,
                workspace.runtime_node_id,
                usage,
            )
            .await?;
        }
        Some(_) => {
            mark_uncertain(state, &reservation, "runtime reported usage error").await?;
            return Ok(());
        }
        None => {
            // Инструмент не вернул usage. Синхронный refresh заблокировал бы
            // агентский цикл на тайм-аут Runtime, поэтому:
            // - нулевой резерв (delete/move/mkdir) подтверждаем сразу;
            // - ненулевой оставляем uncertain до следующего замера/решения
            //   администратора. Место не освобождается вслепую.
            if reservation.bytes > 0 || reservation.files > 0 {
                mark_uncertain(state, &reservation, "usage unavailable after operation").await?;
                return Ok(());
            }
        }
    }
    sqlx::query(
        "update ai_quota_reservations
            set status = 'confirmed', resolved_at = now()
          where id = $1 and status = 'active'",
    )
    .bind(reservation.id)
    .execute(&state.db)
    .await?;
    record_event(state, &reservation, "confirmed", None).await;
    Ok(())
}

/// Известная ошибка без подтверждённого side effect освобождает резерв.
pub async fn finish_error(
    state: &AppState,
    reservation: Reservation,
    error: &ApiError,
    tool_id: &str,
) {
    let preflight_known = matches!(
        error.code.as_str(),
        "AI_TOOL_NOT_ALLOWED"
            | "AI_TOOL_INVALID_ARGUMENTS"
            | "AI_TOOL_DISABLED"
            | "AI_WORKSPACE_PATH_DENIED"
            | "AI_STORAGE_QUOTA_EXCEEDED"
            | "AI_FILES_QUOTA_EXCEEDED"
            | "AI_RUNTIME_OFFLINE"
    );
    let side_effect_possible = tool_id == "program.run" || tool_id.starts_with("mcp.");
    let uncertain = matches!(
        error.code.as_str(),
        "AI_TOOL_TIMEOUT" | "AI_EXECUTION_UNKNOWN" | "AI_RUNTIME_DISCONNECTED"
    ) || (side_effect_possible && !preflight_known);
    if uncertain {
        let _ = mark_uncertain(state, &reservation, &error.code).await;
    } else {
        release(state, &reservation, &error.code).await;
    }
}

pub async fn release(state: &AppState, reservation: &Reservation, reason: &str) {
    let _ = sqlx::query(
        "update ai_quota_reservations
            set status = 'released', resolved_at = now(),
                metadata = metadata || jsonb_build_object('releaseReason', $2)
          where id = $1 and status = 'active'",
    )
    .bind(reservation.id)
    .bind(reason)
    .execute(&state.db)
    .await;
    record_event(state, reservation, "released", Some(reason)).await;
}

pub async fn mark_uncertain(
    state: &AppState,
    reservation: &Reservation,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query(
        "update ai_quota_reservations
            set status = 'uncertain', resolved_at = null,
                metadata = metadata || jsonb_build_object('uncertainReason', $2)
          where id = $1 and status = 'active'",
    )
    .bind(reservation.id)
    .bind(reason)
    .execute(&state.db)
    .await?;
    record_event(state, reservation, "uncertain", Some(reason)).await;
    Ok(())
}

async fn record_event(
    state: &AppState,
    reservation: &Reservation,
    event: &str,
    error: Option<&str>,
) {
    let _ = sqlx::query(
        "insert into ai_quota_events
            (reservation_id, workspace_id, user_id, event, requested_bytes,
             requested_files, error)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(reservation.id)
    .bind(reservation.workspace_id)
    .bind(reservation.user_id)
    .bind(event)
    .bind(reservation.bytes)
    .bind(reservation.files)
    .bind(error)
    .execute(&state.db)
    .await;
}

/// Освобождает все резервы workspace (например, при удалении).
pub async fn release_workspace(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
) -> ApiResult<u64> {
    Ok(sqlx::query(
        "update ai_quota_reservations
            set status = 'released', resolved_at = now()
          where workspace_id = $1 and status in ('active', 'uncertain')",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Истечение зависших резервов. Вызывается фоновым janitor и вручную в тестах.
pub async fn cleanup_expired(state: &AppState) -> ApiResult<(u64, u64)> {
    let active = sqlx::query(
        "update ai_quota_reservations
            set status = 'expired', resolved_at = now()
          where status = 'active' and expires_at < now()",
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    let uncertain = sqlx::query(
        "update ai_quota_reservations
            set status = 'expired', resolved_at = now(),
                metadata = metadata || jsonb_build_object('expiredUncertain', true)
          where status = 'uncertain'
            and created_at < now() - make_interval(secs => $1)",
    )
    .bind(UNCERTAIN_TTL_SECONDS as f64)
    .execute(&state.db)
    .await?
    .rows_affected();
    Ok((active, uncertain))
}

pub fn spawn_janitor(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(300));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match cleanup_expired(&state).await {
                Ok((active, uncertain)) if active + uncertain > 0 => {
                    tracing::info!(active, uncertain, "истёкшие резервы AI-квот освобождены");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "очистка резервов AI-квот"),
            }
        }
    });
}

/// Административное освобождение зависшего/неопределённого резерва.
pub async fn admin_release(state: &AppState, id: Uuid) -> ApiResult<Option<Reservation>> {
    let row = sqlx::query_as::<_, (Uuid, String, Uuid, Uuid, String, i64, i32)>(
        "select id, operation_id, user_id, workspace_id, kind, bytes, files
           from ai_quota_reservations
          where id = $1 and status in ('active', 'uncertain')",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((id, operation_id, user_id, workspace_id, kind, bytes, files)) = row else {
        return Ok(None);
    };
    let reservation = Reservation {
        id,
        operation_id,
        user_id,
        workspace_id,
        kind,
        bytes,
        files,
    };
    release(state, &reservation, "admin release").await;
    Ok(Some(reservation))
}

pub async fn metrics(state: &AppState) -> ApiResult<Value> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "select status, count(*), coalesce(sum(bytes), 0)::bigint,
                coalesce(sum(files), 0)::bigint
           from ai_quota_reservations group by status order by status",
    )
    .fetch_all(&state.db)
    .await?;
    let mut statuses = serde_json::Map::new();
    for (status, count, bytes, files) in rows {
        statuses.insert(
            status,
            json!({ "count": count, "bytes": bytes, "files": files }),
        );
    }
    Ok(json!({ "statuses": statuses }))
}

/// Проверка, какие инструменты участвуют в резервировании (для тестов/док).
pub fn mutating_tools() -> Vec<String> {
    tools::catalog()
        .iter()
        .filter(|tool| is_mutating(&tool.id))
        .map(|tool| tool.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_estimate_counts_added_bytes_and_files() {
        let patch = "--- a/src/a.txt\n+++ b/src/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+line\n";
        let (added, files) = patch_estimate(patch);
        assert_eq!(files, vec!["src/a.txt".to_owned()]);
        assert!(added >= 8);
    }

    #[test]
    fn mutating_tools_include_all_write_paths() {
        let tools = mutating_tools();
        for required in [
            "fs.write",
            "fs.write_base64",
            "fs.copy",
            "fs.move",
            "fs.delete",
            "fs.mkdir",
            "fs.apply_patch",
            "program.run",
        ] {
            assert!(tools.iter().any(|tool| tool == required), "нет {required}");
        }
        assert!(!tools.iter().any(|tool| tool == "fs.read"));
    }
}
