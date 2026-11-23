use nexus_platform::SettingsEventSink;
use nexus_platform::download::{DownloadEvent, DownloadEventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChangedPayload {
    pub changed_keys: Vec<String>,
}

/// Tauri adapter for settings notifications.
///
/// Events are notifications only: the frontend must re-read settings through
/// `settings_get` after receiving `app:settings-changed`.
#[derive(Clone)]
pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl SettingsEventSink for TauriEventSink {
    fn settings_changed(&self, changed_keys: &[String]) {
        let payload = SettingsChangedPayload {
            changed_keys: changed_keys.to_vec(),
        };
        if let Err(error) = self.app.emit("app:settings-changed", payload) {
            tracing::warn!(%error, "cannot emit app:settings-changed");
        }
    }
}

impl DownloadEventSink for TauriEventSink {
    fn emit(&self, event: DownloadEvent) {
        let name = event.name();
        if let Err(error) = self.app.emit(name, event) {
            tracing::warn!(%error, event = name, "cannot emit download event");
        }
    }
}
