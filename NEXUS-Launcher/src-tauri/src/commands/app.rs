use nexus_core::ErrorDto;
use tauri::State;

use crate::dto::{AppInfoDto, BootstrapDto, DbHealthDto};
use crate::state::AppState;

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> Result<AppInfoDto, ErrorDto> {
    Ok(AppInfoDto::new(&state.init, state.paths.display_root()))
}

#[tauri::command]
pub async fn app_bootstrap(state: State<'_, AppState>) -> Result<BootstrapDto, ErrorDto> {
    let settings = state.settings.get().await.map_err(ErrorDto::from)?;
    Ok(BootstrapDto::new(
        &state.init,
        state.paths.display_root(),
        settings,
    ))
}

#[tauri::command]
pub async fn db_health(state: State<'_, AppState>) -> Result<DbHealthDto, ErrorDto> {
    let health = state.db.health().await.map_err(ErrorDto::from)?;
    Ok(DbHealthDto::from(&health))
}
