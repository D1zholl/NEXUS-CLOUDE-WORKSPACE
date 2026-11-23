//! One file transfer.
//!
//! Responsibilities:
//! * validate HTTP status and response metadata;
//! * resume with `Range` + `If-Range` and never mix versions;
//! * stream to a staging file while applying the global speed limit;
//! * verify size/SHA-256 before atomically moving the file into its root;
//! * retry transient failures with exponential backoff and jitter;
//! * cooperate with pause/cancel at every await point.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use nexus_core::{Clock, NexusError};
use reqwest::StatusCode;
use sqlx::SqlitePool;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::security::fs::checksum::{FileDigest, Sha256Digest, hash_file};
use crate::security::fs::{SafeFs, SafeRelativePath};
use crate::security::net::UrlPolicy;

use super::control::{TaskCommand, TaskControl, sleep_or_stop};
use super::http::{
    DownloadHttpConfig, RequestError, content_length, content_range, etag, if_range_value,
    last_modified, retry_after, send_download_request,
};
use super::limiter::{ConcurrencyLimiter, SpeedLimiter};
use super::progress::{FileMeter, TaskMeter};
use super::staging::{self, StagingMeta};
use super::store::{self, FileRow};
use super::types::{DownloadFailure, DownloadRoot, FileStatus};

pub(crate) struct WorkerContext {
    pub pool: SqlitePool,
    pub clock: Arc<dyn Clock>,
    pub fs: SafeFs,
    pub policy: UrlPolicy,
    pub http: DownloadHttpConfig,
    pub limiter: Arc<SpeedLimiter>,
    pub concurrency: Arc<ConcurrencyLimiter>,
    pub max_attempts: u32,
    pub max_file_bytes: u64,
    pub retry_base: Duration,
    pub retry_max: Duration,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum FileOutcome {
    Completed { bytes: u64, skipped: bool },
    Paused { bytes: u64 },
    Cancelled { bytes: u64 },
    Failed { error: DownloadFailure, bytes: u64 },
}

struct AttemptResult {
    outcome: AttemptOutcome,
}

enum AttemptOutcome {
    Completed {
        bytes: u64,
    },
    Paused {
        bytes: u64,
    },
    Cancelled {
        bytes: u64,
    },
    Retry {
        offset: u64,
        reset: bool,
        failure: DownloadFailure,
        retry_after: Option<Duration>,
    },
    Failed(DownloadFailure),
}

pub(crate) async fn download_file(
    ctx: &WorkerContext,
    task_id: &str,
    file: FileRow,
    control: &TaskControl,
    meter: Arc<TaskMeter>,
    file_meter: Arc<FileMeter>,
) -> FileOutcome {
    if let Some(stop) = stop_now(control, file_meter.bytes_done()) {
        return stop;
    }

    match maybe_skip_existing(ctx, &file).await {
        Ok(Some(size)) => {
            file_meter.set_done(size);
            file_meter.set_total(size);
            file_meter.set_status(FileStatus::Skipped);
            if let Err(error) =
                store::mark_file_skipped(&ctx.pool, &file.id, size, ctx.clock.as_ref()).await
            {
                tracing::warn!(file_id = %file.id, code = %error.code(), "cannot persist skipped file");
            }
            return FileOutcome::Completed {
                bytes: 0,
                skipped: true,
            };
        }
        Ok(None) => {}
        Err(error) => {
            let failure = DownloadFailure::from_nexus(&error, false).with_file(&file.id);
            mark_failed(ctx, &file, &file_meter, &failure, 0, None).await;
            return FileOutcome::Failed {
                error: failure,
                bytes: file_meter.bytes_done(),
            };
        }
    }

    let permit = match acquire_slot(ctx, control).await {
        Ok(permit) => permit,
        Err(stop) => return stop,
    };
    // Keep the concurrency slot for the whole transfer, including retries.
    let _permit = permit;

    if let Err(error) = staging::ensure_staging_dir(&ctx.fs, task_id) {
        let failure = DownloadFailure::from_nexus(&error, false).with_file(&file.id);
        mark_failed(ctx, &file, &file_meter, &failure, 0, None).await;
        return FileOutcome::Failed {
            error: failure,
            bytes: file_meter.bytes_done(),
        };
    }

    let mut state = match reconcile_staging(ctx, task_id, &file, &file_meter).await {
        Ok(state) => state,
        Err(error) => {
            let failure = DownloadFailure::from_nexus(&error, false).with_file(&file.id);
            mark_failed(ctx, &file, &file_meter, &failure, 0, None).await;
            return FileOutcome::Failed {
                error: failure,
                bytes: file_meter.bytes_done(),
            };
        }
    };

    // A crash can happen after the last byte was written but before the file
    // was moved. Verify and install it without touching the network.
    if let Some(expected) = expected_size(&file)
        && state.offset == expected
        && let Some(sha256) = &file.sha256
    {
        match verify_and_commit(ctx, task_id, &file, &file_meter, state.offset, sha256).await {
            AttemptOutcome::Completed { bytes } => {
                return FileOutcome::Completed {
                    bytes,
                    skipped: false,
                };
            }
            AttemptOutcome::Retry { reset, failure, .. } => {
                if reset {
                    reset_staging(ctx, task_id, &file, &file_meter, &mut state).await;
                }
                let _ = store::record_error(
                    &ctx.pool,
                    task_id,
                    Some(&file.id),
                    0,
                    &failure.code,
                    &failure.message,
                    failure.http_status,
                    failure.retryable,
                    ctx.clock.as_ref(),
                )
                .await;
            }
            AttemptOutcome::Failed(failure) => {
                mark_failed(ctx, &file, &file_meter, &failure, state.offset, None).await;
                return FileOutcome::Failed {
                    error: failure,
                    bytes: state.offset,
                };
            }
            _ => {}
        }
    }

    let mut attempt = 0u32;
    let mut last_failure: Option<DownloadFailure> = None;

    loop {
        if let Some(stop) = stop_now(control, state.offset) {
            return stop;
        }
        if attempt >= ctx.max_attempts {
            let failure = last_failure.unwrap_or_else(|| {
                DownloadFailure::new(
                    "DOWNLOAD_FAILED",
                    "Не удалось загрузить файл после нескольких попыток.",
                    true,
                )
                .with_file(&file.id)
            });
            mark_failed(
                ctx,
                &file,
                &file_meter,
                &failure,
                state.offset,
                failure.http_status,
            )
            .await;
            return FileOutcome::Failed {
                error: failure,
                bytes: state.offset,
            };
        }
        attempt += 1;

        if let Err(error) =
            store::mark_file_running(&ctx.pool, &file.id, None, ctx.clock.as_ref()).await
        {
            tracing::warn!(file_id = %file.id, code = %error.code(), "cannot persist running file");
        }
        file_meter.set_status(FileStatus::Running);

        let result = attempt_once(
            ctx,
            task_id,
            &file,
            control,
            &meter,
            &file_meter,
            &mut state,
        )
        .await;

        match result.outcome {
            AttemptOutcome::Completed { bytes } => {
                return FileOutcome::Completed {
                    bytes,
                    skipped: false,
                };
            }
            AttemptOutcome::Paused { bytes } => {
                let _ =
                    store::mark_file_paused(&ctx.pool, &file.id, bytes, ctx.clock.as_ref()).await;
                file_meter.set_status(FileStatus::Paused);
                return FileOutcome::Paused { bytes };
            }
            AttemptOutcome::Cancelled { bytes } => {
                file_meter.set_status(FileStatus::Paused);
                return FileOutcome::Cancelled { bytes };
            }
            AttemptOutcome::Retry {
                offset,
                reset,
                failure,
                retry_after,
            } => {
                let _ = store::record_error(
                    &ctx.pool,
                    task_id,
                    Some(&file.id),
                    i64::from(attempt),
                    &failure.code,
                    &failure.message,
                    failure.http_status,
                    failure.retryable,
                    ctx.clock.as_ref(),
                )
                .await;
                last_failure = Some(failure.clone());

                if attempt >= ctx.max_attempts {
                    mark_failed(
                        ctx,
                        &file,
                        &file_meter,
                        &failure,
                        offset,
                        failure.http_status,
                    )
                    .await;
                    return FileOutcome::Failed {
                        error: failure,
                        bytes: offset,
                    };
                }

                if reset {
                    reset_staging(ctx, task_id, &file, &file_meter, &mut state).await;
                } else {
                    state.offset = offset;
                    state.meta.bytes_done = offset;
                    file_meter.set_done(offset);
                }

                let delay = retry_after
                    .unwrap_or_else(|| retry_delay(ctx.retry_base, ctx.retry_max, attempt));
                if let Some(stop) = sleep_or_stop(control, delay).await {
                    return match stop {
                        TaskCommand::Pause => FileOutcome::Paused {
                            bytes: state.offset,
                        },
                        _ => FileOutcome::Cancelled {
                            bytes: state.offset,
                        },
                    };
                }
            }
            AttemptOutcome::Failed(failure) => {
                let _ = store::record_error(
                    &ctx.pool,
                    task_id,
                    Some(&file.id),
                    i64::from(attempt),
                    &failure.code,
                    &failure.message,
                    failure.http_status,
                    false,
                    ctx.clock.as_ref(),
                )
                .await;
                mark_failed(
                    ctx,
                    &file,
                    &file_meter,
                    &failure,
                    state.offset,
                    failure.http_status,
                )
                .await;
                return FileOutcome::Failed {
                    error: failure,
                    bytes: state.offset,
                };
            }
        }
    }
}

struct StagingState {
    offset: u64,
    meta: StagingMeta,
}

async fn reset_staging(
    ctx: &WorkerContext,
    task_id: &str,
    file: &FileRow,
    file_meter: &Arc<FileMeter>,
    state: &mut StagingState,
) {
    staging::remove_part(&ctx.fs, task_id, &file.id);
    staging::remove_meta(&ctx.fs, task_id, &file.id);
    state.offset = 0;
    state.meta = StagingMeta::new(&file.url, file.sha256.clone(), expected_size(file));
    file_meter.set_done(0);
    let _ = store::update_file_progress(&ctx.pool, &file.id, 0, ctx.clock.as_ref()).await;
}

async fn reconcile_staging(
    ctx: &WorkerContext,
    task_id: &str,
    file: &FileRow,
    file_meter: &Arc<FileMeter>,
) -> Result<StagingState, NexusError> {
    let mut meta = staging::read_meta(&ctx.fs, task_id, &file.id)?
        .unwrap_or_else(|| StagingMeta::new(&file.url, file.sha256.clone(), expected_size(file)));

    let raw_len = staging::staged_len(&ctx.fs, task_id, &file.id)?.unwrap_or(0);
    let db_offset = u64::try_from(file.resume_offset.max(0)).unwrap_or(0);

    let compatible = meta.is_compatible(&file.url, file.sha256.as_deref());
    let validators_match = meta.etag.as_deref() == file.etag.as_deref()
        && meta.last_modified.as_deref() == file.last_modified.as_deref()
        && meta.content_length
            == file
                .content_length
                .and_then(|value| u64::try_from(value).ok());
    let has_resume_identity = if_range_value(meta.etag.as_deref(), meta.last_modified.as_deref())
        .is_some()
        || meta.content_length.is_some()
        || file.content_length.is_some();

    let mut offset = if compatible && validators_match && has_resume_identity {
        raw_len.min(meta.bytes_done.max(db_offset))
    } else {
        0
    };

    // Sidecars are written on a 1s throttle. Truncating to the recorded offset
    // guarantees that an interrupted write never leaves bytes of unknown
    // provenance in the file.
    if raw_len > offset {
        truncate_staged(ctx, task_id, &file.id, offset).await?;
    }
    if offset > raw_len {
        offset = raw_len;
    }

    if offset == 0 {
        staging::remove_part(&ctx.fs, task_id, &file.id);
        staging::remove_meta(&ctx.fs, task_id, &file.id);
        meta = StagingMeta::new(&file.url, file.sha256.clone(), expected_size(file));
    }

    meta.bytes_done = offset;
    file_meter.set_done(offset);
    if let Some(total) = meta
        .content_length
        .or_else(|| {
            file.content_length
                .and_then(|value| u64::try_from(value).ok())
        })
        .or_else(|| expected_size(file))
    {
        file_meter.set_total(total);
    }

    let part = staging::part_rel(task_id, &file.id)?;
    let _ = store::set_file_staging(
        &ctx.pool,
        &file.id,
        Some(part.as_path().to_string_lossy().as_ref()),
        ctx.clock.as_ref(),
    )
    .await;

    Ok(StagingState { offset, meta })
}

#[allow(clippy::too_many_arguments)]
async fn attempt_once(
    ctx: &WorkerContext,
    task_id: &str,
    file: &FileRow,
    control: &TaskControl,
    meter: &Arc<TaskMeter>,
    file_meter: &Arc<FileMeter>,
    state: &mut StagingState,
) -> AttemptResult {
    let offset = state.offset;
    let validator = if offset > 0 {
        if_range_value(
            state.meta.etag.as_deref(),
            state.meta.last_modified.as_deref(),
        )
    } else {
        None
    };

    let response = match send_download_request(
        &ctx.policy,
        &ctx.http,
        &file.url,
        offset,
        validator.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let retryable = error.retryable;
            let failure = request_failure(&error, retryable).with_file(&file.id);
            return AttemptResult {
                outcome: if retryable {
                    AttemptOutcome::Retry {
                        offset,
                        reset: false,
                        failure,
                        retry_after: None,
                    }
                } else {
                    AttemptOutcome::Failed(failure)
                },
            };
        }
    };

    let status = response.status();
    let status_u16 = status.as_u16();
    let response_etag = etag(&response);
    let response_last_modified = last_modified(&response);
    let response_length = content_length(&response);
    let response_range = content_range(&response);

    if status == StatusCode::PARTIAL_CONTENT {
        let Some(range) = response_range else {
            return retry(
                offset,
                true,
                http_failure(
                    status_u16,
                    true,
                    "Ответ 206 не содержит корректного Content-Range.",
                ),
            );
        };
        if range.start != offset {
            return retry(
                offset,
                true,
                http_failure(
                    status_u16,
                    true,
                    "Сервер вернул диапазон, не совпадающий с сохранённой позицией.",
                ),
            );
        }
        if let Some(total) = range.total {
            if let Some(expected) = expected_size(file)
                && total != expected
            {
                return retry(
                    offset,
                    true,
                    size_failure("Размер удалённого файла изменился; загрузка начнётся заново."),
                );
            }
            if let Some(previous) = state.meta.content_length.or_else(|| {
                file.content_length
                    .and_then(|value| u64::try_from(value).ok())
            }) && total != previous
            {
                return retry(
                    offset,
                    true,
                    size_failure("Размер удалённого файла изменился; загрузка начнётся заново."),
                );
            }
            state.meta.content_length = Some(total);
            file_meter.set_total(total);
        }
        if remote_changed(
            state.meta.etag.as_deref().or(file.etag.as_deref()),
            state
                .meta
                .last_modified
                .as_deref()
                .or(file.last_modified.as_deref()),
            response_etag.as_deref(),
            response_last_modified.as_deref(),
            offset,
        ) {
            return retry(
                offset,
                true,
                http_failure(
                    status_u16,
                    true,
                    "Удалённый файл изменился; части разных версий не объединяются.",
                ),
            );
        }
        if response_etag.is_some() {
            state.meta.etag = response_etag;
        }
        if response_last_modified.is_some() {
            state.meta.last_modified = response_last_modified;
        }
        let _ = store::update_file_remote(
            &ctx.pool,
            &file.id,
            state.meta.etag.as_deref(),
            state.meta.last_modified.as_deref(),
            state.meta.content_length,
            status_u16,
            ctx.clock.as_ref(),
        )
        .await;
    } else if status == StatusCode::OK {
        if offset > 0 {
            return retry(
                0,
                true,
                http_failure(
                    status_u16,
                    true,
                    "Сервер проигнорировал Range; загрузка начнётся с нуля.",
                ),
            );
        }
        if let Some(total) = response_length {
            if let Some(expected) = expected_size(file)
                && total != expected
            {
                return retry(
                    0,
                    true,
                    size_failure("Размер удалённого файла изменился; загрузка начнётся заново."),
                );
            }
            state.meta.content_length = Some(total);
            file_meter.set_total(total);
        }
        state.meta.etag = response_etag;
        state.meta.last_modified = response_last_modified;
        let _ = store::update_file_remote(
            &ctx.pool,
            &file.id,
            state.meta.etag.as_deref(),
            state.meta.last_modified.as_deref(),
            state.meta.content_length,
            status_u16,
            ctx.clock.as_ref(),
        )
        .await;
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        return retry(
            0,
            true,
            http_failure(
                status_u16,
                true,
                "Сервер отклонил диапазон; загрузка начнётся с нуля.",
            ),
        );
    } else if status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        return AttemptResult {
            outcome: AttemptOutcome::Retry {
                offset,
                reset: false,
                failure: http_failure(
                    status_u16,
                    true,
                    &format!("Сервер вернул HTTP {status_u16}."),
                ),
                retry_after: retry_after(&response),
            },
        };
    } else {
        return AttemptResult {
            outcome: AttemptOutcome::Failed(http_failure(
                status_u16,
                false,
                &format!("Сервер вернул HTTP {status_u16}."),
            )),
        };
    }

    stream_response(
        ctx,
        task_id,
        file,
        control,
        meter,
        file_meter,
        state,
        response,
        status_u16,
        response_length,
        response_range.and_then(|range| range.total),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn stream_response(
    ctx: &WorkerContext,
    task_id: &str,
    file: &FileRow,
    control: &TaskControl,
    meter: &Arc<TaskMeter>,
    file_meter: &Arc<FileMeter>,
    state: &mut StagingState,
    response: reqwest::Response,
    status: u16,
    response_length: Option<u64>,
    range_total: Option<u64>,
) -> AttemptResult {
    let path = match staging::absolute_part_path(&ctx.fs, task_id, &file.id) {
        Ok(path) => path,
        Err(error) => {
            return AttemptResult {
                outcome: AttemptOutcome::Failed(
                    DownloadFailure::from_nexus(&error, false).with_file(&file.id),
                ),
            };
        }
    };

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true).write(true);
    if state.offset == 0 {
        options.create(true);
    }
    let mut handle = match options.open(&path).await {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && state.offset > 0 => {
            return retry(
                0,
                true,
                DownloadFailure::new(
                    "DOWNLOAD_STAGING_LOST",
                    "Промежуточный файл был потерян; загрузка начнётся заново.",
                    true,
                )
                .with_file(&file.id),
            );
        }
        Err(error) => {
            return AttemptResult {
                outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
            };
        }
    };

    if let Err(error) = handle.set_len(state.offset).await {
        return AttemptResult {
            outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
        };
    }
    if let Err(error) = handle.seek(std::io::SeekFrom::Start(state.offset)).await {
        return AttemptResult {
            outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
        };
    }

    let mut written = state.offset;
    let mut stream = response.bytes_stream();
    let mut receiver = control.subscribe();
    let mut last_meta = Instant::now();
    let expected_total = if status == 206 {
        range_total
    } else {
        response_length
    };
    let expected = expected_size(file);
    let max_size = ctx.max_file_bytes;

    loop {
        if control.command() != TaskCommand::Run {
            let _ = handle.flush().await;
            let _ = handle.sync_all().await;
            state.offset = written;
            state.meta.bytes_done = written;
            let _ =
                staging::write_meta(&ctx.fs, task_id, &file.id, &state.meta, ctx.clock.as_ref());
            file_meter.set_done(written);
            return stop_result(control, written);
        }

        let next = tokio::select! {
            biased;
            changed = receiver.changed() => {
                if changed.is_err() {
                    let _ = handle.flush().await;
                    let _ = handle.sync_all().await;
                    return AttemptResult {
                        outcome: AttemptOutcome::Cancelled { bytes: written },
                    };
                }
                continue;
            }
            next = stream.next() => next,
        };

        let Some(chunk) = next else {
            break;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = handle.flush().await;
                let _ = handle.sync_all().await;
                state.offset = written;
                state.meta.bytes_done = written;
                let _ = staging::write_meta(
                    &ctx.fs,
                    task_id,
                    &file.id,
                    &state.meta,
                    ctx.clock.as_ref(),
                );
                file_meter.set_done(written);
                return AttemptResult {
                    outcome: AttemptOutcome::Retry {
                        offset: written,
                        reset: false,
                        failure: DownloadFailure::new(
                            "DOWNLOAD_INTERRUPTED",
                            format!("Соединение прервано во время передачи файла: {error}"),
                            true,
                        )
                        .with_file(&file.id)
                        .with_status(status),
                        retry_after: None,
                    },
                };
            }
        };

        let next_len = written.saturating_add(chunk.len() as u64);
        if next_len > max_size {
            return AttemptResult {
                outcome: AttemptOutcome::Failed(
                    size_failure("Файл превышает допустимый размер.").with_file(&file.id),
                ),
            };
        }
        if let Some(expected) = expected
            && next_len > expected
        {
            return retry(
                written,
                true,
                size_failure("Ответ сервера больше ожидаемого размера файла."),
            );
        }

        tokio::select! {
            _ = ctx.limiter.acquire(chunk.len()) => {}
            changed = receiver.changed() => {
                if changed.is_err() {
                    let _ = handle.flush().await;
                    let _ = handle.sync_all().await;
                    return AttemptResult {
                        outcome: AttemptOutcome::Cancelled { bytes: written },
                    };
                }
                continue;
            }
        }

        if let Err(error) = handle.write_all(&chunk).await {
            let _ = handle.flush().await;
            return AttemptResult {
                outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
            };
        }
        written = next_len;
        file_meter.add(chunk.len() as u64);
        meter.note_session_bytes(chunk.len() as u64);

        if last_meta.elapsed() >= Duration::from_secs(1) {
            state.offset = written;
            state.meta.bytes_done = written;
            let _ =
                staging::write_meta(&ctx.fs, task_id, &file.id, &state.meta, ctx.clock.as_ref());
            last_meta = Instant::now();
        }
    }

    if let Err(error) = handle.flush().await {
        return AttemptResult {
            outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
        };
    }
    if let Err(error) = handle.sync_all().await {
        return AttemptResult {
            outcome: AttemptOutcome::Failed(disk_failure(&error).with_file(&file.id)),
        };
    }
    drop(handle);

    state.offset = written;
    state.meta.bytes_done = written;
    let _ = staging::write_meta(&ctx.fs, task_id, &file.id, &state.meta, ctx.clock.as_ref());

    if let Some(total) = expected_total
        && written != total
    {
        return retry(
            written,
            written > total,
            size_failure("Получено неверное количество байт; передача будет продолжена."),
        );
    }
    if let Some(expected) = expected
        && written != expected
    {
        return retry(
            written,
            written > expected,
            size_failure("Размер загруженного файла не совпадает с ожидаемым."),
        );
    }

    match verify_and_commit(
        ctx,
        task_id,
        file,
        file_meter,
        written,
        file.sha256.as_deref().unwrap_or(""),
    )
    .await
    {
        AttemptOutcome::Completed { bytes } => AttemptResult {
            outcome: AttemptOutcome::Completed { bytes },
        },
        AttemptOutcome::Failed(failure) => AttemptResult {
            outcome: AttemptOutcome::Failed(failure),
        },
        AttemptOutcome::Retry {
            offset,
            reset,
            failure,
            retry_after,
        } => AttemptResult {
            outcome: AttemptOutcome::Retry {
                offset,
                reset,
                failure,
                retry_after,
            },
        },
        AttemptOutcome::Paused { bytes } => AttemptResult {
            outcome: AttemptOutcome::Paused { bytes },
        },
        AttemptOutcome::Cancelled { bytes } => AttemptResult {
            outcome: AttemptOutcome::Cancelled { bytes },
        },
    }
}

async fn verify_and_commit(
    ctx: &WorkerContext,
    task_id: &str,
    file: &FileRow,
    file_meter: &Arc<FileMeter>,
    expected_bytes: u64,
    expected_sha256: &str,
) -> AttemptOutcome {
    let part = match staging::part_rel(task_id, &file.id) {
        Ok(part) => part,
        Err(error) => {
            return AttemptOutcome::Failed(
                DownloadFailure::from_nexus(&error, false).with_file(&file.id),
            );
        }
    };
    let path = match staging::absolute_part_path(&ctx.fs, task_id, &file.id) {
        Ok(path) => path,
        Err(error) => {
            return AttemptOutcome::Failed(
                DownloadFailure::from_nexus(&error, false).with_file(&file.id),
            );
        }
    };

    let digest = match hash_async(path, ctx.max_file_bytes).await {
        Ok(digest) => digest,
        Err(error) => {
            return AttemptOutcome::Failed(
                DownloadFailure::from_nexus(&error, true).with_file(&file.id),
            );
        }
    };

    if !expected_sha256.is_empty() {
        let expected = match Sha256Digest::from_hex(expected_sha256) {
            Ok(expected) => expected,
            Err(error) => {
                return AttemptOutcome::Failed(
                    DownloadFailure::from_nexus(&error, false).with_file(&file.id),
                );
            }
        };
        if !digest.sha256.constant_time_eq(&expected) {
            reset_after_verify_failure(ctx, task_id, &file.id);
            return AttemptOutcome::Retry {
                offset: 0,
                reset: true,
                failure: DownloadFailure::new(
                    "DOWNLOAD_HASH_MISMATCH",
                    "SHA-256 загруженного файла не совпадает с ожидаемым.",
                    true,
                )
                .with_file(&file.id),
                retry_after: None,
            };
        }
    }

    if let Some(expected) = expected_size(file)
        && digest.size != expected
    {
        reset_after_verify_failure(ctx, task_id, &file.id);
        return AttemptOutcome::Retry {
            offset: 0,
            reset: true,
            failure: DownloadFailure::new(
                "DOWNLOAD_SIZE_MISMATCH",
                "Размер загруженного файла не совпадает с ожидаемым.",
                true,
            )
            .with_file(&file.id),
            retry_after: None,
        };
    }
    if expected_bytes != digest.size {
        reset_after_verify_failure(ctx, task_id, &file.id);
        return AttemptOutcome::Retry {
            offset: 0,
            reset: true,
            failure: DownloadFailure::new(
                "DOWNLOAD_SIZE_MISMATCH",
                "Размер загруженного файла изменился во время проверки.",
                true,
            )
            .with_file(&file.id),
            retry_after: None,
        };
    }

    let destination = match destination_rel(file) {
        Ok(destination) => destination,
        Err(error) => {
            return AttemptOutcome::Failed(
                DownloadFailure::from_nexus(&error, false).with_file(&file.id),
            );
        }
    };

    let fs = ctx.fs.clone();
    let part_for_move = part.clone();
    let destination_for_move = destination.clone();
    let move_result =
        tokio::task::spawn_blocking(move || fs.rename(&part_for_move, &destination_for_move, true))
            .await;
    match move_result {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            return AttemptOutcome::Failed(
                DownloadFailure::from_nexus(&error, true).with_file(&file.id),
            );
        }
        Err(error) => {
            return AttemptOutcome::Failed(
                DownloadFailure::new(
                    "DOWNLOAD_FAILED",
                    format!("Не удалось переместить файл: {error}"),
                    true,
                )
                .with_file(&file.id),
            );
        }
    }

    // A rename must not hide a truncated or substituted destination.
    if !expected_sha256.is_empty() {
        let fs = ctx.fs.clone();
        let destination_for_hash = destination.clone();
        let max = ctx.max_file_bytes;
        let installed =
            tokio::task::spawn_blocking(move || fs.hash(&destination_for_hash, max)).await;
        match installed {
            Ok(Ok(installed)) => {
                let expected = Sha256Digest::from_hex(expected_sha256).ok();
                let valid = installed.size == digest.size
                    && expected
                        .map(|expected| installed.sha256.constant_time_eq(&expected))
                        .unwrap_or(true);
                if !valid {
                    let _ = ctx.fs.remove_file(&destination);
                    reset_after_verify_failure(ctx, task_id, &file.id);
                    return AttemptOutcome::Retry {
                        offset: 0,
                        reset: true,
                        failure: DownloadFailure::new(
                            "DOWNLOAD_HASH_MISMATCH",
                            "Установленный файл не прошёл повторную проверку SHA-256.",
                            true,
                        )
                        .with_file(&file.id),
                        retry_after: None,
                    };
                }
            }
            Ok(Err(error)) => {
                let _ = ctx.fs.remove_file(&destination);
                return AttemptOutcome::Failed(
                    DownloadFailure::from_nexus(&error, true).with_file(&file.id),
                );
            }
            Err(error) => {
                let _ = ctx.fs.remove_file(&destination);
                return AttemptOutcome::Failed(
                    DownloadFailure::new(
                        "DOWNLOAD_FAILED",
                        format!("Не удалось проверить установленный файл: {error}"),
                        true,
                    )
                    .with_file(&file.id),
                );
            }
        }
    }

    staging::remove_meta(&ctx.fs, task_id, &file.id);
    if let Err(error) =
        store::mark_file_completed(&ctx.pool, &file.id, digest.size, ctx.clock.as_ref()).await
    {
        tracing::warn!(file_id = %file.id, code = %error.code(), "cannot persist completed file");
    }
    file_meter.set_done(digest.size);
    file_meter.set_total(digest.size);
    file_meter.set_status(FileStatus::Completed);

    AttemptOutcome::Completed { bytes: digest.size }
}

fn reset_after_verify_failure(ctx: &WorkerContext, task_id: &str, file_id: &str) {
    staging::remove_part(&ctx.fs, task_id, file_id);
    staging::remove_meta(&ctx.fs, task_id, file_id);
}

async fn maybe_skip_existing(
    ctx: &WorkerContext,
    file: &FileRow,
) -> Result<Option<u64>, NexusError> {
    let destination = destination_rel(file)?;
    let resolved = ctx.fs.resolve(&destination)?;
    if !resolved.exists {
        return Ok(None);
    }
    if resolved.is_dir {
        return Err(NexusError::conflict("destination is a directory"));
    }
    let metadata = std::fs::metadata(&resolved.absolute).map_err(|error| {
        NexusError::internal(format!(
            "cannot inspect destination {}: {error}",
            resolved.absolute.display()
        ))
    })?;
    let len = metadata.len();

    if let Some(raw_hash) = &file.sha256 {
        if let Some(expected) = expected_size(file)
            && len != expected
        {
            return Ok(None);
        }
        let expected = Sha256Digest::from_hex(raw_hash)?;
        let path = resolved.absolute.clone();
        let max = ctx.max_file_bytes;
        let digest = hash_async(path, max).await?;
        if digest.size == len && digest.sha256.constant_time_eq(&expected) {
            return Ok(Some(len));
        }
        return Ok(None);
    }

    if let Some(expected) = expected_size(file)
        && len == expected
    {
        return Ok(Some(len));
    }
    Ok(None)
}

pub(crate) fn destination_rel(file: &FileRow) -> Result<SafeRelativePath, NexusError> {
    let root = DownloadRoot::from_db_str(&file.root_kind)
        .ok_or_else(|| NexusError::internal("unknown download root"))?;
    let relative = SafeRelativePath::parse(&file.rel_path)?;
    if root == DownloadRoot::Downloads
        && relative
            .as_path()
            .components()
            .next()
            .map(|component| component.as_os_str().eq_ignore_ascii_case("staging"))
            .unwrap_or(false)
    {
        return Err(NexusError::invalid_path(
            "downloads/staging is reserved for unfinished transfers",
        ));
    }
    SafeRelativePath::parse(format!("{}/{}", root.prefix(), relative))
}

pub(crate) fn expected_size(file: &FileRow) -> Option<u64> {
    if file.size_bytes > 0 {
        return u64::try_from(file.size_bytes).ok();
    }
    file.content_length
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
}

async fn truncate_staged(
    ctx: &WorkerContext,
    task_id: &str,
    file_id: &str,
    len: u64,
) -> Result<(), NexusError> {
    let path = staging::absolute_part_path(&ctx.fs, task_id, file_id)?;
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        file.set_len(len)?;
        file.sync_all()?;
        Ok::<(), std::io::Error>(())
    })
    .await
    .map_err(|error| NexusError::internal(format!("truncate task failed: {error}")))?
    .map_err(|error| NexusError::internal(format!("cannot truncate staged file: {error}")))
}

async fn hash_async(path: PathBuf, max_size: u64) -> Result<FileDigest, NexusError> {
    tokio::task::spawn_blocking(move || hash_file(&path, max_size))
        .await
        .map_err(|error| NexusError::internal(format!("hash task failed: {error}")))?
}

async fn mark_failed(
    ctx: &WorkerContext,
    file: &FileRow,
    file_meter: &Arc<FileMeter>,
    failure: &DownloadFailure,
    bytes: u64,
    http_status: Option<u16>,
) {
    if let Err(error) = store::mark_file_failed(
        &ctx.pool,
        &file.id,
        &failure.code,
        bytes,
        http_status,
        ctx.clock.as_ref(),
    )
    .await
    {
        tracing::warn!(file_id = %file.id, code = %error.code(), "cannot persist failed file");
    }
    file_meter.set_done(bytes);
    file_meter.set_status(FileStatus::Failed);
}

async fn acquire_slot<'a>(
    ctx: &'a WorkerContext,
    control: &TaskControl,
) -> Result<super::limiter::ConcurrencyPermit<'a>, FileOutcome> {
    if let Some(stop) = stop_now(control, 0) {
        return Err(stop);
    }
    let mut receiver = control.subscribe();
    tokio::select! {
        permit = ctx.concurrency.acquire() => Ok(permit),
        changed = receiver.changed() => {
            let _ = changed;
            match control.command() {
                TaskCommand::Pause => Err(FileOutcome::Paused { bytes: 0 }),
                _ => Err(FileOutcome::Cancelled { bytes: 0 }),
            }
        }
    }
}

fn stop_now(control: &TaskControl, bytes: u64) -> Option<FileOutcome> {
    match control.command() {
        TaskCommand::Run => None,
        TaskCommand::Pause => Some(FileOutcome::Paused { bytes }),
        TaskCommand::Cancel => Some(FileOutcome::Cancelled { bytes }),
    }
}

fn stop_result(control: &TaskControl, bytes: u64) -> AttemptResult {
    AttemptResult {
        outcome: match control.command() {
            TaskCommand::Pause => AttemptOutcome::Paused { bytes },
            TaskCommand::Cancel => AttemptOutcome::Cancelled { bytes },
            TaskCommand::Run => AttemptOutcome::Retry {
                offset: bytes,
                reset: false,
                failure: DownloadFailure::new(
                    "DOWNLOAD_INTERRUPTED",
                    "Передача была прервана.",
                    true,
                ),
                retry_after: None,
            },
        },
    }
}

fn retry(offset: u64, reset: bool, failure: DownloadFailure) -> AttemptResult {
    AttemptResult {
        outcome: AttemptOutcome::Retry {
            offset,
            reset,
            failure,
            retry_after: None,
        },
    }
}

fn remote_changed(
    stored_etag: Option<&str>,
    stored_last_modified: Option<&str>,
    response_etag: Option<&str>,
    response_last_modified: Option<&str>,
    offset: u64,
) -> bool {
    if offset == 0 {
        return false;
    }
    if let Some(stored) = stored_etag {
        return match response_etag {
            Some(current) => stored != current,
            None => true,
        };
    }
    if let Some(stored) = stored_last_modified {
        return match response_last_modified {
            Some(current) => stored != current,
            None => true,
        };
    }
    false
}

fn retry_delay(base: Duration, max: Duration, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10);
    let multiplier = 1u32 << exponent;
    let base_ms = base.as_millis().max(1) as u64;
    let max_ms = max.as_millis().max(1) as u64;
    let raw = base_ms.saturating_mul(u64::from(multiplier)).min(max_ms);
    // Deterministic jitter of +/-10% avoids a synchronised retry storm without
    // introducing an RNG dependency.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = (nanos % 21) as i64 - 10;
    let adjusted = (raw as i64 + (raw as i64 * jitter / 100)).max(1) as u64;
    Duration::from_millis(adjusted.min(max_ms))
}

fn request_failure(error: &RequestError, retryable: bool) -> DownloadFailure {
    let code = match error.error.code() {
        nexus_core::ErrorCode::NetworkTimeout => "DOWNLOAD_TIMEOUT",
        nexus_core::ErrorCode::UrlInvalid
        | nexus_core::ErrorCode::UrlBlocked
        | nexus_core::ErrorCode::TlsRequired
        | nexus_core::ErrorCode::RedirectRejected => "DOWNLOAD_URL_REJECTED",
        _ if retryable => "DOWNLOAD_NETWORK",
        _ => "DOWNLOAD_FAILED",
    };
    DownloadFailure::new(
        code,
        if retryable {
            "Сетевая ошибка при загрузке файла. Повтор будет выполнен автоматически."
        } else {
            "Не удалось отправить запрос на загрузку файла."
        },
        retryable,
    )
}

fn http_failure(status: u16, retryable: bool, message: &str) -> DownloadFailure {
    DownloadFailure::new("DOWNLOAD_HTTP_STATUS", message, retryable).with_status(status)
}

fn size_failure(message: &str) -> DownloadFailure {
    DownloadFailure::new("DOWNLOAD_SIZE_MISMATCH", message, true)
}

fn disk_failure(error: &std::io::Error) -> DownloadFailure {
    let code = match error.raw_os_error() {
        Some(112) | Some(28) => "DOWNLOAD_DISK_FULL",
        Some(5) | Some(13) => "DOWNLOAD_PERMISSION_DENIED",
        _ => "DOWNLOAD_IO_ERROR",
    };
    DownloadFailure::new(code, format!("Ошибка записи на диск: {error}"), false)
}
