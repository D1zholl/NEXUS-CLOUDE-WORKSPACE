//! Typed IPC for the Download Engine.
//!
//! Every command is a thin adapter: validation and persistence live in
//! `nexus_platform::download`. Long-running work is always asynchronous; the UI
//! receives progress through the `download:*` events.

use nexus_core::{ErrorDto, NexusError, TaskId};
use nexus_platform::download::{
    DownloadErrorRecord, DownloadFileSnapshot, DownloadTaskDetail, DownloadTaskSnapshot,
    NewDownloadTask,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadLimitsDto {
    pub max_concurrent: usize,
    pub max_speed_bps: u64,
}

fn parse_task_id(raw: &str) -> Result<TaskId, ErrorDto> {
    TaskId::parse(raw).map_err(|_| {
        ErrorDto::from(NexusError::invalid_argument(
            "download task id is not a UUID",
        ))
    })
}

#[tauri::command]
pub async fn downloads_enqueue(
    state: State<'_, AppState>,
    request: NewDownloadTask,
) -> Result<DownloadTaskSnapshot, ErrorDto> {
    state
        .downloads
        .enqueue(request)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_list(
    state: State<'_, AppState>,
    status: Option<String>,
) -> Result<Vec<DownloadTaskSnapshot>, ErrorDto> {
    state
        .downloads
        .list_tasks(status.as_deref())
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_get(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTaskDetail, ErrorDto> {
    state
        .downloads
        .get_detail(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_files(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<DownloadFileSnapshot>, ErrorDto> {
    state
        .downloads
        .files(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_pause(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTaskSnapshot, ErrorDto> {
    state
        .downloads
        .pause(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_resume(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTaskSnapshot, ErrorDto> {
    state
        .downloads
        .resume(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_cancel(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTaskSnapshot, ErrorDto> {
    state
        .downloads
        .cancel(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_retry(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<DownloadTaskSnapshot, ErrorDto> {
    state
        .downloads
        .retry(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_remove(state: State<'_, AppState>, task_id: String) -> Result<(), ErrorDto> {
    state
        .downloads
        .remove(parse_task_id(&task_id)?)
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_clear_finished(state: State<'_, AppState>) -> Result<u64, ErrorDto> {
    state
        .downloads
        .clear_finished()
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub async fn downloads_errors(
    state: State<'_, AppState>,
    task_id: String,
    limit: Option<u32>,
) -> Result<Vec<DownloadErrorRecord>, ErrorDto> {
    state
        .downloads
        .errors(parse_task_id(&task_id)?, limit.unwrap_or(100))
        .await
        .map_err(ErrorDto::from)
}

#[tauri::command]
pub fn downloads_limits(state: State<'_, AppState>) -> DownloadLimitsDto {
    let (max_concurrent, max_speed_bps) = state.downloads.limits();
    DownloadLimitsDto {
        max_concurrent,
        max_speed_bps,
    }
}
