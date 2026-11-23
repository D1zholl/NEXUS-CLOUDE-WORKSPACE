use nexus_core::{EffectiveSettings, ErrorDto, SettingsGroup, SettingsPatch};
use tauri::State;

use crate::state::AppState;

/// Reads the effective settings (defaults merged with `app_settings`).
#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<EffectiveSettings, ErrorDto> {
    state.settings.get().await.map_err(ErrorDto::from)
}

/// Validates and persists a partial settings patch atomically.
///
/// The effective settings are returned so the UI can re-render without a second
/// call. A successful change also emits `app:settings-changed`.
#[tauri::command]
pub async fn settings_update(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<EffectiveSettings, ErrorDto> {
    let change = state.settings.apply(&patch).await.map_err(ErrorDto::from)?;
    state.downloads.set_limits(
        usize::from(change.settings.downloads.max_concurrent),
        u64::from(change.settings.downloads.max_speed_kbps) * 1024,
    );
    Ok(change.settings)
}

/// Deletes one typed group from `app_settings`; defaults apply on the next read.
/// Unknown/future keys are preserved.
#[tauri::command]
pub async fn settings_reset_group(
    state: State<'_, AppState>,
    group: SettingsGroup,
) -> Result<EffectiveSettings, ErrorDto> {
    let settings = state
        .settings
        .reset_group(group)
        .await
        .map_err(ErrorDto::from)?;
    state.downloads.set_limits(
        usize::from(settings.downloads.max_concurrent),
        u64::from(settings.downloads.max_speed_kbps) * 1024,
    );
    Ok(settings)
}
