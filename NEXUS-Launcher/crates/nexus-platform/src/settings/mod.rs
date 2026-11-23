//! Persistence for typed application settings.
//!
//! Every known setting is stored as its own JSON value under a stable key.
//! Unknown keys are never deleted: they may have been written by a newer
//! launcher and must survive a downgrade.

use std::collections::HashMap;
use std::sync::Arc;

use nexus_core::settings::keys;
use nexus_core::{
    Clock, EffectiveSettings, NexusError, SettingsChange, SettingsGroup, SettingsPatch,
};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::db::DbError;
use crate::repositories::activity;

/// Optional adapter used by `src-tauri` to publish `app:settings-changed`.
pub trait SettingsEventSink: Send + Sync {
    fn settings_changed(&self, changed_keys: &[String]);
}

#[derive(Clone)]
pub struct SettingsService {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    base: EffectiveSettings,
    events: Option<Arc<dyn SettingsEventSink>>,
}

impl SettingsService {
    pub fn new(pool: SqlitePool, clock: Arc<dyn Clock>) -> Self {
        Self::with_base(pool, clock, EffectiveSettings::default())
    }

    /// Uses values from `config.toml` as the layer below `app_settings`.
    pub fn with_base(pool: SqlitePool, clock: Arc<dyn Clock>, base: EffectiveSettings) -> Self {
        Self {
            pool,
            clock,
            base,
            events: None,
        }
    }

    pub fn with_events(
        pool: SqlitePool,
        clock: Arc<dyn Clock>,
        events: Arc<dyn SettingsEventSink>,
    ) -> Self {
        Self::with_base_and_events(pool, clock, EffectiveSettings::default(), events)
    }

    pub fn with_base_and_events(
        pool: SqlitePool,
        clock: Arc<dyn Clock>,
        base: EffectiveSettings,
        events: Arc<dyn SettingsEventSink>,
    ) -> Self {
        Self {
            pool,
            clock,
            base,
            events: Some(events),
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Loads settings from SQLite. Damaged or unknown stored values are ignored
    /// with a warning instead of blocking startup.
    pub async fn get(&self) -> Result<EffectiveSettings, NexusError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value_json FROM app_settings ORDER BY key")
                .fetch_all(&self.pool)
                .await
                .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;

        let mut settings = self.base.clone();
        for (key, raw) in rows {
            let value: Value = match serde_json::from_str(&raw) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(%key, %error, "stored setting is not valid JSON; keeping default");
                    continue;
                }
            };
            if let Err(error) = settings.apply_stored(&key, &value) {
                tracing::warn!(
                    %key,
                    code = %error.code(),
                    "stored setting is invalid; keeping default"
                );
            }
        }
        Ok(settings)
    }

    /// Applies a partial typed patch in one transaction.
    ///
    /// Either every changed value is committed or nothing changes.
    pub async fn apply(&self, patch: &SettingsPatch) -> Result<SettingsChange, NexusError> {
        if patch.is_empty() {
            return Ok(SettingsChange {
                settings: self.get().await?,
                changed_keys: Vec::new(),
            });
        }

        let current = self.get().await?;
        let change = patch.apply(&current)?;
        if change.changed_keys.is_empty() {
            return Ok(change);
        }

        let all_values: HashMap<&'static str, Value> =
            change.settings.key_values().into_iter().collect();

        let now = self.clock.now_ms();
        let mut transaction =
            self.pool.begin().await.map_err(|error| {
                NexusError::settings_write(DbError::from_sqlx(error).to_string())
            })?;

        for key in &change.changed_keys {
            let value = all_values.get(key.as_str()).ok_or_else(|| {
                NexusError::settings_write(format!("value for known key {key} is missing"))
            })?;
            let json = serde_json::to_string(value)
                .map_err(|error| NexusError::settings_write(error.to_string()))?;
            sqlx::query(
                "INSERT INTO app_settings (key, value_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(key) DO UPDATE SET
                     value_json = excluded.value_json,
                     updated_at = excluded.updated_at",
            )
            .bind(key)
            .bind(&json)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| NexusError::settings_write(DbError::from_sqlx(error).to_string()))?;
        }

        transaction
            .commit()
            .await
            .map_err(|error| NexusError::settings_write(DbError::from_sqlx(error).to_string()))?;

        self.log_change("settings.updated", &change.changed_keys)
            .await;
        self.emit(&change.changed_keys);
        Ok(change)
    }

    /// Resets one typed group to defaults by deleting its keys. Keys belonging
    /// to other groups and unknown/future keys are untouched.
    pub async fn reset_group(&self, group: SettingsGroup) -> Result<EffectiveSettings, NexusError> {
        let keys = group.keys();
        let mut transaction =
            self.pool.begin().await.map_err(|error| {
                NexusError::settings_write(DbError::from_sqlx(error).to_string())
            })?;

        for key in keys {
            sqlx::query("DELETE FROM app_settings WHERE key = ?")
                .bind(key)
                .execute(&mut *transaction)
                .await
                .map_err(|error| {
                    NexusError::settings_write(DbError::from_sqlx(error).to_string())
                })?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| NexusError::settings_write(DbError::from_sqlx(error).to_string()))?;

        let changed: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        self.log_change("settings.reset", &changed).await;
        self.emit(&changed);
        self.get().await
    }

    /// Read-only access for diagnostics. Values are returned verbatim, so this
    /// must not be exposed to untrusted callers.
    pub async fn raw_value(&self, key: &str) -> Result<Option<Value>, NexusError> {
        if key.len() > 128 || key.chars().any(char::is_control) {
            return Err(NexusError::invalid_argument("setting key is invalid"));
        }
        let raw: Option<String> =
            sqlx::query_scalar("SELECT value_json FROM app_settings WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| NexusError::from(DbError::from_sqlx(error)))?;
        match raw {
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|error| NexusError::settings_invalid(error.to_string())),
            None => Ok(None),
        }
    }

    /// Number of persisted keys, including unknown/future keys.
    pub async fn stored_key_count(&self) -> Result<i64, NexusError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM app_settings")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| NexusError::from(DbError::from_sqlx(error)))
    }

    async fn log_change(&self, kind: &str, changed_keys: &[String]) {
        let details = serde_json::json!({
            "keys": changed_keys,
        });
        if let Err(error) = activity::log(
            &self.pool,
            self.clock.as_ref(),
            kind,
            Some("settings"),
            None,
            "Application settings changed.",
            Some(details),
        )
        .await
        {
            // Activity logging is diagnostic, it must not fail settings writes.
            tracing::warn!(code = %error.code(), "cannot write settings activity log");
        }
    }

    fn emit(&self, changed_keys: &[String]) {
        if let Some(events) = &self.events {
            events.settings_changed(changed_keys);
        }
    }
}

/// Ensures a key is part of the typed settings contract. Used by tests and by
/// future generic commands; arbitrary JSON keys are not accepted from the UI.
pub fn is_known_key(key: &str) -> bool {
    keys::ALL.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_known_keys_are_recognised() {
        assert!(is_known_key(keys::UI_THEME));
        assert!(is_known_key(keys::DOWNLOADS_MAX_CONCURRENT));
        assert!(!is_known_key("ui.theme.extra"));
        assert!(!is_known_key("secret.token"));
    }
}
