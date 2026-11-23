//! Исполнитель системных фоновых задач.
//!
//! Очередь обслуживает `scheduler.rs`: он атомарно переводит задачу в
//! `running`, а этот модуль выполняет работу и пишет `result`/`error`.
//!
//! Итог каждой задачи сохраняется в `result`, ошибка — в `error`.
//! Системная ошибка дополнительно дублируется в `audit_events` (`job.failed`).
//!
//! Между единицами работы исполнитель проверяет флаги `cancel_requested` и
//! `pause_requested`. Отмена прерывает цикл сразу, пауза — после завершения
//! текущей единицы: задача переходит в `paused`, а слот планировщика
//! освобождается, потому что `execute` возвращается.

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;
use crate::system::audit::{AuditEvent, record};

/// Флаги управления между единицами работы.
///
/// `PauseAfterUnit` означает: текущую единицу нужно довести до конца и только
/// затем перевести задачу в `paused` — ровно то поведение, которое требует план
/// («пауза останавливает после текущей единицы»).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Continue,
    PauseAfterUnit,
    Cancel,
}

/// Выполняет прямую задачу вне планировщика (тесты и ручной запуск).
///
/// Задача захватывается так же, как это делает планировщик, поэтому один
/// и тот же `id` не будет выполнен дважды.
pub async fn run_job(state: Arc<AppState>, job_id: Uuid) {
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update background_jobs
            set status = 'running', started_at = coalesce(started_at, now()), updated_at = now()
          where id = $1 and status = 'pending'
          returning id",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    if claimed.is_some() {
        execute(&state, job_id).await;
    }
}

/// Выполняет задачу, уже переведённую в `running` планировщиком.
pub async fn execute(state: &AppState, job_id: Uuid) {
    let row: Option<(String, Value)> =
        sqlx::query_as("select job_type, payload from background_jobs where id = $1")
            .bind(job_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some((job_type, payload)) = row else {
        return;
    };

    if boundary(state, job_id).await == Boundary::Cancel {
        mark_cancelled(state, job_id).await;
        publish_cancelled(state, job_id, &job_type);
        return;
    }

    let result = match job_type.as_str() {
        "system.storage.check" => storage_check(state, job_id).await,
        "system.objects.verify" => verify_objects(state, job_id, &payload).await,
        "system.uploads.cleanup" => cleanup_uploads(state, job_id).await,
        "system.shares.cleanup" => cleanup_shares(state, job_id).await,
        "system.trash.cleanup" => cleanup_trash(state, job_id).await,
        "system.metrics.prune" => metrics_prune(state, job_id).await,
        "system.manifests.regenerate" => regenerate_manifests(state, job_id).await,
        "system.idempotency.cleanup" => cleanup_idempotency(state, job_id).await,
        // Формирование ZIP уже существовало; теперь его запускает планировщик.
        "cloud.export" => crate::cloud::exports::run_export(state, job_id)
            .await
            .map_err(|error| error.to_string()),
        other => Err(format!("неизвестный тип задачи: {other}")),
    };

    match result {
        Ok(summary) => {
            // Обработчик мог остановиться на паузе между единицами работы.
            let paused: Option<String> = sqlx::query_scalar(
                "select status from background_jobs where id = $1 and status = 'paused'",
            )
            .bind(job_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            if paused.is_some() {
                return;
            }

            let final_status: Option<String> = sqlx::query_scalar(
                "update background_jobs
                 set status = case when cancel_requested then 'cancelled' else 'completed' end,
                     progress = case when cancel_requested then progress else 100 end,
                     done_units = case when cancel_requested then done_units
                                       else coalesce(total_units, 0) end,
                     pause_requested = false,
                     finished_at = now(), updated_at = now(), result = $2, error = null
                 where id = $1
                 returning status",
            )
            .bind(job_id)
            .bind(summary)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            match final_status.as_deref() {
                Some("completed") => state.realtime.publish_system(
                    "job.completed",
                    json!({ "jobId": job_id, "jobType": job_type, "progress": 100.0 }),
                ),
                Some("cancelled") => publish_cancelled(state, job_id, &job_type),
                _ => {}
            }
        }
        Err(error) => {
            let message: String = error.chars().take(500).collect();
            let _ = sqlx::query(
                "update background_jobs
                 set status = 'failed', error = $2, pause_requested = false,
                     finished_at = now(), updated_at = now()
                 where id = $1",
            )
            .bind(job_id)
            .bind(&message)
            .execute(&state.db)
            .await;

            state.realtime.publish_system(
                "job.failed",
                json!({ "jobId": job_id, "jobType": job_type, "error": message }),
            );

            // C4: системная ошибка задачи дублируется в журнал аудита.
            record(
                &state.db,
                AuditEvent {
                    event_type: "job.failed",
                    actor_id: None,
                    actor_username: "system",
                    target_type: "job",
                    target_id: &job_id.to_string(),
                    result: "error",
                    ip: "",
                    details: json!({ "jobType": job_type, "error": message }),
                },
            )
            .await;
        }
    }
}

fn publish_cancelled(state: &AppState, job_id: Uuid, job_type: &str) {
    state.realtime.publish_system(
        "job.cancelled",
        json!({ "jobId": job_id, "jobType": job_type }),
    );
}

async fn mark_cancelled(state: &AppState, job_id: Uuid) {
    let _ = sqlx::query(
        "update background_jobs
            set status = 'cancelled', pause_requested = false,
                finished_at = coalesce(finished_at, now()), updated_at = now()
          where id = $1",
    )
    .bind(job_id)
    .execute(&state.db)
    .await;
}

/// Переводит задачу в `paused` на границе единиц работы.
///
/// Обновление выполняется только для `running`, поэтому повторный вызов после
/// завершения задачи ничего не меняет и событие не публикуется.
async fn mark_paused(state: &AppState, job_id: Uuid) -> bool {
    let paused_at = chrono::Utc::now();
    let updated: Option<Uuid> = sqlx::query_scalar(
        "update background_jobs
            set status = 'paused', paused_at = $2, pause_requested = false, updated_at = now()
          where id = $1 and status = 'running'
          returning id",
    )
    .bind(job_id)
    .bind(paused_at)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    if updated.is_some() {
        state.realtime.publish_system(
            "job.paused",
            json!({ "jobId": job_id, "pausedAt": paused_at }),
        );
    }
    updated.is_some()
}

/// Читает флаги управления задачей одной выборкой.
async fn boundary(state: &AppState, job_id: Uuid) -> Boundary {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "select cancel_requested, pause_requested from background_jobs where id = $1",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    match row {
        Some((true, _)) => Boundary::Cancel,
        Some((_, true)) => Boundary::PauseAfterUnit,
        _ => Boundary::Continue,
    }
}

async fn set_progress(state: &AppState, job_id: Uuid, done: i64, total: i64) {
    let progress = if total > 0 {
        (done as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    let _ = sqlx::query(
        "update background_jobs
         set done_units = $2, total_units = $3, progress = $4, updated_at = now()
         where id = $1",
    )
    .bind(job_id)
    .bind(done)
    .bind(total)
    .bind(progress)
    .execute(&state.db)
    .await;
    state.realtime.publish_system(
        "job.progress",
        json!({ "jobId": job_id, "done": done, "total": total, "progress": progress }),
    );
}

/// Сверяет объекты хранилища с диском: размеры и наличие файлов.
///
/// Перед проверкой объектов обновляется состояние каждого тома. Если состояние
/// изменилось (`online` → `offline`/`error` или обратно), публикуется
/// системное событие `storage.volume.updated` — ровно как у ручной проверки.
async fn storage_check(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let volumes: Vec<(Uuid, String)> =
        sqlx::query_as("select id, state from storage_volumes order by id")
            .fetch_all(&state.db)
            .await
            .map_err(|error| error.to_string())?;
    let mut volumes_changed = 0i64;
    for (volume_id, previous_state) in volumes {
        if boundary(state, job_id).await == Boundary::Cancel {
            break;
        }
        match crate::storage::service::check_volume(&state.db, volume_id).await {
            Ok(outcome) => {
                if outcome.row.state != previous_state {
                    volumes_changed += 1;
                    state.realtime.publish_system(
                        "storage.volume.updated",
                        json!({
                            "volumeId": outcome.row.id,
                            "name": outcome.row.name,
                            "state": outcome.row.state,
                            "previousState": previous_state,
                            "totalBytes": outcome.row.total_bytes,
                            "availableBytes": outcome.row.available_bytes,
                        }),
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%volume_id, %error, "не удалось проверить том");
            }
        }
    }

    let rows: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "select o.id, o.object_key, v.root_path, o.size_bytes
         from storage_objects o
         join storage_volumes v on v.id = o.volume_id
         where o.state = 'committed'",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|error| error.to_string())?;

    let total = rows.len() as i64;
    let mut checked = 0i64;
    let mut missing = 0i64;
    let mut mismatched = 0i64;

    for (object_id, key, root_path, size) in rows {
        let control = boundary(state, job_id).await;
        if control == Boundary::Cancel {
            break;
        }
        let root = std::path::PathBuf::from(&root_path);
        let location = crate::storage::engine::ObjectLocation {
            volume_id: Uuid::nil(),
            absolute: crate::storage::engine::ObjectLocation::objects_root(&root).join(&key),
            key,
            root,
        };
        match crate::storage::engine::stat_object(&location) {
            Ok(actual) if actual as i64 == size => {}
            Ok(_) => {
                mismatched += 1;
                let _ = sqlx::query(
                    "update storage_objects set state = 'missing', verified_at = now() where id = $1",
                )
                .bind(object_id)
                .execute(&state.db)
                .await;
            }
            Err(_) => {
                missing += 1;
                let _ = sqlx::query(
                    "update storage_objects set state = 'missing', verified_at = now() where id = $1",
                )
                .bind(object_id)
                .execute(&state.db)
                .await;
            }
        }
        checked += 1;
        if checked % 50 == 0 {
            set_progress(state, job_id, checked, total).await;
        }
        if control == Boundary::PauseAfterUnit {
            set_progress(state, job_id, checked, total).await;
            mark_paused(state, job_id).await;
            break;
        }
    }

    set_progress(state, job_id, checked, total).await;
    Ok(json!({
        "checked": checked,
        "missing": missing,
        "sizeMismatch": mismatched,
        "total": total,
        "volumesChanged": volumes_changed,
    }))
}

/// Сверяет SHA-256 объектов с записью в БД поверх проверки размеров.
///
/// `payload.mode`: `full` — все committed-объекты, `sample` — выборка
/// (по умолчанию 10 %, `payload.samplePercent` от 1 до 100).
async fn verify_objects(state: &AppState, job_id: Uuid, payload: &Value) -> Result<Value, String> {
    let mode = match payload.get("mode").and_then(Value::as_str) {
        Some("full") => "full",
        _ => "sample",
    };
    let percent = payload
        .get("samplePercent")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .clamp(1, 100);

    let committed: i64 =
        sqlx::query_scalar("select count(*) from storage_objects where state = 'committed'")
            .fetch_one(&state.db)
            .await
            .map_err(|error| error.to_string())?;

    type ObjectRow = (Uuid, String, String, i64, String);
    let rows: Vec<ObjectRow> = if mode == "full" {
        sqlx::query_as(
            "select o.id, o.object_key, v.root_path, o.size_bytes, o.sha256
             from storage_objects o
             join storage_volumes v on v.id = o.volume_id
             where o.state = 'committed'
             order by o.created_at, o.id",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|error| error.to_string())?
    } else {
        let limit = if committed == 0 {
            0
        } else {
            ((committed * percent + 99) / 100).clamp(1, committed)
        };
        sqlx::query_as(
            "select o.id, o.object_key, v.root_path, o.size_bytes, o.sha256
             from storage_objects o
             join storage_volumes v on v.id = o.volume_id
             where o.state = 'committed'
             order by md5(o.id::text || $1)
             limit $2",
        )
        .bind(job_id.to_string())
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .map_err(|error| error.to_string())?
    };

    let total = rows.len() as i64;
    let mut checked = 0i64;
    let mut verified = 0i64;
    let mut sha_mismatch = 0i64;
    let mut size_mismatch = 0i64;
    let mut missing = 0i64;
    let mut mismatched_objects: Vec<Uuid> = Vec::new();

    for (object_id, key, root_path, size, expected_sha) in rows {
        let control = boundary(state, job_id).await;
        if control == Boundary::Cancel {
            break;
        }
        let root = std::path::PathBuf::from(&root_path);
        let location = crate::storage::engine::ObjectLocation {
            volume_id: Uuid::nil(),
            absolute: crate::storage::engine::ObjectLocation::objects_root(&root).join(&key),
            key,
            root,
        };

        match crate::storage::engine::stat_object(&location) {
            Ok(actual) if actual as i64 == size => {
                match crate::storage::hashing::hash_file(&location.absolute).await {
                    Ok(actual_sha) if actual_sha == expected_sha => {
                        verified += 1;
                        let _ = sqlx::query(
                            "update storage_objects set verified_at = now() where id = $1",
                        )
                        .bind(object_id)
                        .execute(&state.db)
                        .await;
                    }
                    Ok(_) => {
                        // Файл существует, но содержимое не совпало с БД:
                        // не скрываем объект из выдачи, фиксируем в отчёте.
                        sha_mismatch += 1;
                        if mismatched_objects.len() < 100 {
                            mismatched_objects.push(object_id);
                        }
                        let _ = sqlx::query(
                            "update storage_objects set verified_at = now() where id = $1",
                        )
                        .bind(object_id)
                        .execute(&state.db)
                        .await;
                        tracing::warn!(%object_id, "SHA-256 объекта не совпадает с БД");
                    }
                    Err(_) => {
                        missing += 1;
                        mark_object_missing(&state.db, object_id).await;
                    }
                }
            }
            Ok(_) => {
                size_mismatch += 1;
                mark_object_missing(&state.db, object_id).await;
            }
            Err(_) => {
                missing += 1;
                mark_object_missing(&state.db, object_id).await;
            }
        }

        checked += 1;
        if checked % 25 == 0 {
            set_progress(state, job_id, checked, total).await;
        }
        if control == Boundary::PauseAfterUnit {
            set_progress(state, job_id, checked, total).await;
            mark_paused(state, job_id).await;
            break;
        }
    }

    set_progress(state, job_id, checked, total).await;
    Ok(json!({
        "mode": mode,
        "samplePercent": if mode == "sample" { Some(percent) } else { None },
        "checked": checked,
        "verified": verified,
        "shaMismatch": sha_mismatch,
        "sizeMismatch": size_mismatch,
        "missing": missing,
        "mismatched": mismatched_objects,
        "totalCommitted": committed,
        "total": total,
    }))
}

async fn mark_object_missing(db: &PgPool, object_id: Uuid) {
    let _ = sqlx::query(
        "update storage_objects set state = 'missing', verified_at = now() where id = $1",
    )
    .bind(object_id)
    .execute(db)
    .await;
}

async fn cleanup_uploads(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let cloud = crate::cloud::uploads::cleanup_expired(state)
        .await
        .map_err(|error| error.to_string())?;
    let minecraft = crate::minecraft::uploads::cleanup_expired(state)
        .await
        .map_err(|error| error.to_string())?;
    let export_ttl_hours = state.export_ttl_hours().await;
    let exports = crate::cloud::exports::cleanup_exports(&state.db, export_ttl_hours)
        .await
        .map_err(|error| error.to_string())?;
    set_progress(state, job_id, 1, 1).await;
    Ok(json!({
        "cloudSessions": cloud,
        "minecraftSessions": minecraft,
        "exports": exports,
    }))
}

async fn cleanup_shares(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let deleted = sqlx::query(
        "delete from cloud_shares
         where (expires_at is not null and expires_at < now() - interval '30 days')
            or (revoked_at is not null and revoked_at < now() - interval '30 days')",
    )
    .execute(&state.db)
    .await
    .map_err(|error| error.to_string())?
    .rows_affected();
    let tokens = sqlx::query(
        "delete from access_tokens
         where (expires_at is not null and expires_at < now() - interval '30 days')
            or (revoked_at is not null and revoked_at < now() - interval '30 days')",
    )
    .execute(&state.db)
    .await
    .map_err(|error| error.to_string())?
    .rows_affected();
    set_progress(state, job_id, 1, 1).await;
    Ok(json!({ "shares": deleted, "tokens": tokens }))
}

/// E4: удаляет записи корзины старше `limits.trash_retention_days`.
async fn cleanup_trash(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let retention_days = state.trash_retention_days().await;
    let (entries, files) = crate::cloud::service::cleanup_trash(&state.db, retention_days)
        .await
        .map_err(|error| error.message)?;
    set_progress(state, job_id, 1, 1).await;
    record(
        &state.db,
        AuditEvent {
            event_type: "cloud.trash.cleaned",
            actor_id: None,
            actor_username: "system",
            target_type: "cloud_trash",
            target_id: "",
            result: "success",
            ip: "",
            details: json!({
                "entries": entries,
                "files": files,
                "retentionDays": retention_days,
            }),
        },
    )
    .await;
    Ok(json!({
        "entries": entries,
        "files": files,
        "retentionDays": retention_days,
    }))
}

async fn metrics_prune(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let removed =
        sqlx::query("delete from system_metrics where captured_at < now() - interval '7 days'")
            .execute(&state.db)
            .await
            .map_err(|error| error.to_string())?
            .rows_affected();
    let jobs = sqlx::query(
        "delete from background_jobs
         where finished_at is not null and finished_at < now() - interval '30 days'",
    )
    .execute(&state.db)
    .await
    .map_err(|error| error.to_string())?
    .rows_affected();
    set_progress(state, job_id, 1, 1).await;
    Ok(json!({ "metrics": removed, "jobs": jobs }))
}

/// G1/C4: аддитивная регенерация манифестов опубликованных релизов.
///
/// Пересборка выполняется из неизменяемого снимка релиза, поэтому файлы
/// (`path`, `size`, `sha256`) не могут измениться. Допускается только
/// добавление `icon`; любой другой диф считается нарушением неизменяемости:
/// такой релиз пропускается и попадает в отчёт, документ не перезаписывается.
/// Каждое фактическое изменение пишется в аудит.
async fn regenerate_manifests(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let actor: Option<(Option<Uuid>, String)> = sqlx::query_as(
        "select j.created_by, coalesce(u.username, 'system')
         from background_jobs j
         left join users u on u.id = j.created_by
         where j.id = $1",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| error.to_string())?;
    let (actor_id, actor_username) = actor.unwrap_or((None, "system".to_owned()));

    // Детерминированный обход: порядок не зависит от плана БД.
    let releases: Vec<(Uuid, Uuid, Value)> = sqlx::query_as(
        "select r.id, r.build_id, r.manifest
         from minecraft_releases r
         where r.status in ('published', 'deprecated')
         order by r.build_id, r.id",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|error| error.to_string())?;

    let total = releases.len() as i64;
    let mut updated = 0i64;
    let mut unchanged = 0i64;
    let mut skipped = 0i64;
    let mut skipped_releases: Vec<Value> = Vec::new();
    let mut done = 0i64;

    for (release_id, build_id, previous) in releases {
        let control = boundary(state, job_id).await;
        if control == Boundary::Cancel {
            break;
        }

        let next = crate::minecraft::manifests::build_manifest(&state.db, build_id, release_id)
            .await
            .map_err(|error| error.to_string())?;
        crate::minecraft::manifests::validate_manifest(&next).map_err(|error| error.to_string())?;

        if previous == next {
            unchanged += 1;
        } else if crate::minecraft::manifests::regeneration_is_additive(&previous, &next) {
            sqlx::query("update minecraft_releases set manifest = $2 where id = $1")
                .bind(release_id)
                .bind(&next)
                .execute(&state.db)
                .await
                .map_err(|error| error.to_string())?;
            updated += 1;
            record(
                &state.db,
                AuditEvent {
                    event_type: "minecraft.manifest.regenerated",
                    actor_id,
                    actor_username: &actor_username,
                    target_type: "minecraft_release",
                    target_id: &release_id.to_string(),
                    result: "success",
                    ip: "",
                    details: json!({
                        "jobId": job_id,
                        "buildId": build_id,
                        "icon": next.get("icon").is_some(),
                    }),
                },
            )
            .await;
        } else {
            skipped += 1;
            if skipped_releases.len() < 50 {
                skipped_releases.push(json!({
                    "releaseId": release_id,
                    "buildId": build_id,
                    "reason": "non_additive_change",
                }));
            }
            tracing::warn!(
                %release_id,
                %build_id,
                "регенерация манифеста пропущена: изменение не аддитивно"
            );
        }

        done += 1;
        if done % 25 == 0 {
            set_progress(state, job_id, done, total).await;
        }
        if control == Boundary::PauseAfterUnit {
            set_progress(state, job_id, done, total).await;
            mark_paused(state, job_id).await;
            break;
        }
    }

    set_progress(state, job_id, done, total).await;
    Ok(json!({
        "scanned": done,
        "total": total,
        "updated": updated,
        "unchanged": unchanged,
        "skipped": skipped,
        "skippedReleases": skipped_releases,
    }))
}

async fn cleanup_idempotency(state: &AppState, job_id: Uuid) -> Result<Value, String> {
    let removed = crate::core::idempotency::cleanup(&state.db)
        .await
        .map_err(|error| error.to_string())?;
    set_progress(state, job_id, 1, 1).await;
    Ok(json!({ "keys": removed }))
}
