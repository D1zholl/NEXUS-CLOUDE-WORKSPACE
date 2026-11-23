//! Typed application settings stored in SQLite (`app_settings`).
//!
//! Settings are intentionally modelled as typed groups. The frontend can apply
//! a partial patch, but it cannot write arbitrary JSON keys. Unknown keys that
//! already exist in the database are preserved so that an older launcher does
//! not destroy values written by a newer one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::NexusError;
use crate::paths::validate_directory;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

pub mod keys {
    pub const PATHS_GAME_DIRECTORY: &str = "paths.game_directory";
    pub const PATHS_JAVA_DIRECTORY: &str = "paths.java_directory";

    pub const UI_LANGUAGE: &str = "ui.language";
    pub const UI_THEME: &str = "ui.theme";
    pub const UI_BACKGROUND_QUALITY: &str = "ui.background_quality";
    pub const UI_BACKGROUND_FPS: &str = "ui.background_fps";

    pub const DOWNLOADS_MAX_CONCURRENT: &str = "downloads.max_concurrent";
    pub const DOWNLOADS_MAX_SPEED_KBPS: &str = "downloads.max_speed_kbps";

    pub const LOGGING_ENABLED: &str = "logging.enabled";
    pub const LOGGING_LEVEL: &str = "logging.level";

    pub const PATHS: &[&str] = &[PATHS_GAME_DIRECTORY, PATHS_JAVA_DIRECTORY];
    pub const UI: &[&str] = &[
        UI_LANGUAGE,
        UI_THEME,
        UI_BACKGROUND_QUALITY,
        UI_BACKGROUND_FPS,
    ];
    pub const DOWNLOADS: &[&str] = &[DOWNLOADS_MAX_CONCURRENT, DOWNLOADS_MAX_SPEED_KBPS];
    pub const LOGGING: &[&str] = &[LOGGING_ENABLED, LOGGING_LEVEL];

    pub const ALL: &[&str] = &[
        PATHS_GAME_DIRECTORY,
        PATHS_JAVA_DIRECTORY,
        UI_LANGUAGE,
        UI_THEME,
        UI_BACKGROUND_QUALITY,
        UI_BACKGROUND_FPS,
        DOWNLOADS_MAX_CONCURRENT,
        DOWNLOADS_MAX_SPEED_KBPS,
        LOGGING_ENABLED,
        LOGGING_LEVEL,
    ];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Ru,
    En,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundQuality {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl BackgroundQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathSettings {
    /// `None` means "use the default path inside the NEXUS data directory".
    #[serde(default)]
    pub game_directory: Option<String>,
    /// `None` means "use NEXUS-managed Java runtimes or auto-detection".
    #[serde(default)]
    pub java_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub background_quality: BackgroundQuality,
    /// Target frame rate for the animated background. `0` means unlimited.
    #[serde(default = "default_background_fps")]
    pub background_fps: u16,
}

pub const fn default_background_fps() -> u16 {
    60
}

pub const fn default_max_concurrent_downloads() -> u8 {
    4
}

pub const fn default_max_speed_kbps() -> u32 {
    0
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            background_quality: BackgroundQuality::default(),
            background_fps: default_background_fps(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent: u8,
    /// `0` means unlimited.
    #[serde(default = "default_max_speed_kbps")]
    pub max_speed_kbps: u32,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            max_concurrent: default_max_concurrent_downloads(),
            max_speed_kbps: default_max_speed_kbps(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingSettings {
    #[serde(default = "default_logging_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub level: LogLevel,
}

pub const fn default_logging_enabled() -> bool {
    true
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            enabled: default_logging_enabled(),
            level: LogLevel::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSettings {
    #[serde(default)]
    pub paths: PathSettings,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub downloads: DownloadSettings,
    #[serde(default)]
    pub logging: LoggingSettings,
}

impl EffectiveSettings {
    /// Returns `(key, JSON value)` pairs for every known setting.
    pub fn key_values(&self) -> Vec<(&'static str, Value)> {
        vec![
            (
                keys::PATHS_GAME_DIRECTORY,
                option_string_to_json(&self.paths.game_directory),
            ),
            (
                keys::PATHS_JAVA_DIRECTORY,
                option_string_to_json(&self.paths.java_directory),
            ),
            (keys::UI_LANGUAGE, json!(self.ui.language)),
            (keys::UI_THEME, json!(self.ui.theme)),
            (
                keys::UI_BACKGROUND_QUALITY,
                json!(self.ui.background_quality),
            ),
            (keys::UI_BACKGROUND_FPS, json!(self.ui.background_fps)),
            (
                keys::DOWNLOADS_MAX_CONCURRENT,
                json!(self.downloads.max_concurrent),
            ),
            (
                keys::DOWNLOADS_MAX_SPEED_KBPS,
                json!(self.downloads.max_speed_kbps),
            ),
            (keys::LOGGING_ENABLED, json!(self.logging.enabled)),
            (keys::LOGGING_LEVEL, json!(self.logging.level)),
        ]
    }

    /// Applies one stored JSON value. Unknown keys are ignored (forward
    /// compatibility). Invalid values return a `SETTINGS_INVALID` error; the
    /// service treats them as a soft failure and keeps the default.
    pub fn apply_stored(&mut self, key: &str, value: &Value) -> Result<bool, NexusError> {
        match key {
            keys::PATHS_GAME_DIRECTORY => {
                self.paths.game_directory = stored_optional_path(key, value)?;
            }
            keys::PATHS_JAVA_DIRECTORY => {
                self.paths.java_directory = stored_optional_path(key, value)?;
            }
            keys::UI_LANGUAGE => self.ui.language = from_json(key, value)?,
            keys::UI_THEME => self.ui.theme = from_json(key, value)?,
            keys::UI_BACKGROUND_QUALITY => self.ui.background_quality = from_json(key, value)?,
            keys::UI_BACKGROUND_FPS => {
                let fps: u16 = from_json(key, value)?;
                validate_background_fps(fps)?;
                self.ui.background_fps = fps;
            }
            keys::DOWNLOADS_MAX_CONCURRENT => {
                let value: u8 = from_json(key, value)?;
                validate_max_concurrent(value)?;
                self.downloads.max_concurrent = value;
            }
            keys::DOWNLOADS_MAX_SPEED_KBPS => {
                let value: u32 = from_json(key, value)?;
                validate_max_speed(value)?;
                self.downloads.max_speed_kbps = value;
            }
            keys::LOGGING_ENABLED => self.logging.enabled = from_json(key, value)?,
            keys::LOGGING_LEVEL => self.logging.level = from_json(key, value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), NexusError> {
        if let Some(path) = &self.paths.game_directory {
            validate_directory(path)?;
        }
        if let Some(path) = &self.paths.java_directory {
            validate_directory(path)?;
        }
        validate_background_fps(self.ui.background_fps)?;
        validate_max_concurrent(self.downloads.max_concurrent)?;
        validate_max_speed(self.downloads.max_speed_kbps)?;
        Ok(())
    }

    /// Default values for a single group, used by `settings_reset_group`.
    pub fn group_defaults(group: SettingsGroup) -> Self {
        let mut settings = Self::default();
        match group {
            SettingsGroup::Paths => settings.paths = PathSettings::default(),
            SettingsGroup::Ui => settings.ui = UiSettings::default(),
            SettingsGroup::Downloads => settings.downloads = DownloadSettings::default(),
            SettingsGroup::Logging => settings.logging = LoggingSettings::default(),
        }
        settings
    }
}

fn option_string_to_json(value: &Option<String>) -> Value {
    match value {
        Some(value) => json!(value),
        None => Value::Null,
    }
}

fn stored_optional_path(key: &str, value: &Value) -> Result<Option<String>, NexusError> {
    match value {
        Value::Null => Ok(None),
        Value::String(raw) if raw.is_empty() => Ok(None),
        Value::String(raw) => {
            validate_directory(raw)
                .map_err(|error| NexusError::settings_invalid(format!("{key}: {error}")))?;
            Ok(Some(raw.clone()))
        }
        _ => Err(NexusError::settings_invalid(format!(
            "{key}: expected a string or null"
        ))),
    }
}

fn from_json<T>(key: &str, value: &Value) -> Result<T, NexusError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value.clone())
        .map_err(|error| NexusError::settings_invalid(format!("{key}: {error}")))
}

fn validate_background_fps(value: u16) -> Result<(), NexusError> {
    if value > 240 {
        return Err(NexusError::settings_invalid(
            "background_fps must be between 0 and 240",
        ));
    }
    Ok(())
}

fn validate_max_concurrent(value: u8) -> Result<(), NexusError> {
    if !(1..=16).contains(&value) {
        return Err(NexusError::settings_invalid(
            "max_concurrent must be between 1 and 16",
        ));
    }
    Ok(())
}

fn validate_max_speed(value: u32) -> Result<(), NexusError> {
    if value > 1_000_000 {
        return Err(NexusError::settings_invalid(
            "max_speed_kbps must be between 0 and 1000000",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsGroup {
    Paths,
    Ui,
    Downloads,
    Logging,
}

impl SettingsGroup {
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Paths => keys::PATHS,
            Self::Ui => keys::UI,
            Self::Downloads => keys::DOWNLOADS,
            Self::Logging => keys::LOGGING,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathSettingsPatch {
    #[serde(default)]
    pub game_directory: Option<String>,
    #[serde(default)]
    pub java_directory: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiSettingsPatch {
    #[serde(default)]
    pub language: Option<Language>,
    #[serde(default)]
    pub theme: Option<Theme>,
    #[serde(default)]
    pub background_quality: Option<BackgroundQuality>,
    #[serde(default)]
    pub background_fps: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadSettingsPatch {
    #[serde(default)]
    pub max_concurrent: Option<u8>,
    #[serde(default)]
    pub max_speed_kbps: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoggingSettingsPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub level: Option<LogLevel>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default)]
    pub paths: Option<PathSettingsPatch>,
    #[serde(default)]
    pub ui: Option<UiSettingsPatch>,
    #[serde(default)]
    pub downloads: Option<DownloadSettingsPatch>,
    #[serde(default)]
    pub logging: Option<LoggingSettingsPatch>,
}

impl SettingsPatch {
    pub fn is_empty(&self) -> bool {
        self.paths.is_none()
            && self.ui.is_none()
            && self.downloads.is_none()
            && self.logging.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsChange {
    pub settings: EffectiveSettings,
    pub changed_keys: Vec<String>,
}

impl SettingsPatch {
    /// Applies the patch to a copy of `current`.
    ///
    /// Either the whole patch is valid and applied, or nothing changes.
    pub fn apply(&self, current: &EffectiveSettings) -> Result<SettingsChange, NexusError> {
        let mut next = current.clone();
        let mut changed = Vec::new();

        if let Some(paths) = &self.paths {
            if let Some(raw) = &paths.game_directory {
                let value = normalize_path(&next.paths.game_directory, raw)?;
                if next.paths.game_directory != value {
                    next.paths.game_directory = value;
                    changed.push(keys::PATHS_GAME_DIRECTORY.to_owned());
                }
            }
            if let Some(raw) = &paths.java_directory {
                let value = normalize_path(&next.paths.java_directory, raw)?;
                if next.paths.java_directory != value {
                    next.paths.java_directory = value;
                    changed.push(keys::PATHS_JAVA_DIRECTORY.to_owned());
                }
            }
        }

        if let Some(ui) = &self.ui {
            if let Some(value) = ui.language
                && next.ui.language != value
            {
                next.ui.language = value;
                changed.push(keys::UI_LANGUAGE.to_owned());
            }
            if let Some(value) = ui.theme
                && next.ui.theme != value
            {
                next.ui.theme = value;
                changed.push(keys::UI_THEME.to_owned());
            }
            if let Some(value) = ui.background_quality
                && next.ui.background_quality != value
            {
                next.ui.background_quality = value;
                changed.push(keys::UI_BACKGROUND_QUALITY.to_owned());
            }
            if let Some(value) = ui.background_fps {
                validate_background_fps(value)?;
                if next.ui.background_fps != value {
                    next.ui.background_fps = value;
                    changed.push(keys::UI_BACKGROUND_FPS.to_owned());
                }
            }
        }

        if let Some(downloads) = &self.downloads {
            if let Some(value) = downloads.max_concurrent {
                validate_max_concurrent(value)?;
                if next.downloads.max_concurrent != value {
                    next.downloads.max_concurrent = value;
                    changed.push(keys::DOWNLOADS_MAX_CONCURRENT.to_owned());
                }
            }
            if let Some(value) = downloads.max_speed_kbps {
                validate_max_speed(value)?;
                if next.downloads.max_speed_kbps != value {
                    next.downloads.max_speed_kbps = value;
                    changed.push(keys::DOWNLOADS_MAX_SPEED_KBPS.to_owned());
                }
            }
        }

        if let Some(logging) = &self.logging {
            if let Some(value) = logging.enabled
                && next.logging.enabled != value
            {
                next.logging.enabled = value;
                changed.push(keys::LOGGING_ENABLED.to_owned());
            }
            if let Some(value) = logging.level
                && next.logging.level != value
            {
                next.logging.level = value;
                changed.push(keys::LOGGING_LEVEL.to_owned());
            }
        }

        next.validate()?;
        Ok(SettingsChange {
            settings: next,
            changed_keys: changed,
        })
    }
}

fn normalize_path(old: &Option<String>, raw: &str) -> Result<Option<String>, NexusError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let path =
        validate_directory(raw).map_err(|error| NexusError::settings_invalid(error.to_string()))?;
    let normalized = path.to_string_lossy().into_owned();
    if old.as_deref() == Some(normalized.as_str()) {
        Ok(old.clone())
    } else {
        Ok(Some(normalized))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_complete() {
        let settings = EffectiveSettings::default();
        settings.validate().unwrap();
        assert_eq!(settings.ui.language, Language::Ru);
        assert_eq!(settings.ui.background_fps, 60);
        assert_eq!(settings.downloads.max_concurrent, 4);
        assert_eq!(settings.key_values().len(), keys::ALL.len());
    }

    #[test]
    fn patch_changes_only_provided_fields() {
        let current = EffectiveSettings::default();
        let patch: SettingsPatch = serde_json::from_value(json!({
            "ui": { "theme": "light", "backgroundFps": 30 },
            "downloads": { "maxConcurrent": 8 }
        }))
        .unwrap();
        let change = patch.apply(&current).unwrap();
        assert_eq!(change.settings.ui.theme, Theme::Light);
        assert_eq!(change.settings.ui.background_fps, 30);
        assert_eq!(change.settings.downloads.max_concurrent, 8);
        assert_eq!(change.settings.ui.language, Language::Ru);
        assert_eq!(
            change.changed_keys,
            vec!["ui.theme", "ui.background_fps", "downloads.max_concurrent"]
        );
    }

    #[test]
    fn invalid_patch_is_rejected_without_mutating_current() {
        let current = EffectiveSettings::default();
        let patch: SettingsPatch = serde_json::from_value(json!({
            "ui": { "backgroundFps": 999 }
        }))
        .unwrap();
        assert!(patch.apply(&current).is_err());
        assert_eq!(current.ui.background_fps, 60);
    }

    #[test]
    fn paths_must_be_absolute_and_empty_clears() {
        let current = EffectiveSettings::default();
        let bad: SettingsPatch = serde_json::from_value(json!({
            "paths": { "gameDirectory": "relative\\dir" }
        }))
        .unwrap();
        assert!(bad.apply(&current).is_err());

        let set: SettingsPatch = serde_json::from_value(json!({
            "paths": { "gameDirectory": "C:\\Games\\Minecraft" }
        }))
        .unwrap();
        let changed = set.apply(&current).unwrap();
        assert_eq!(
            changed.settings.paths.game_directory.as_deref(),
            Some("C:\\Games\\Minecraft")
        );

        let clear: SettingsPatch = serde_json::from_value(json!({
            "paths": { "gameDirectory": "" }
        }))
        .unwrap();
        let cleared = clear.apply(&changed.settings).unwrap();
        assert_eq!(cleared.settings.paths.game_directory, None);
    }

    #[test]
    fn stored_values_round_trip_and_unknown_keys_are_ignored() {
        let mut settings = EffectiveSettings::default();
        for (key, value) in settings.clone().key_values() {
            assert!(settings.apply_stored(key, &value).unwrap());
        }
        assert!(!settings.apply_stored("future.unknown", &json!(1)).unwrap());

        let bad = NexusError::settings_invalid("x");
        let error = settings
            .apply_stored(keys::UI_THEME, &json!("neon"))
            .unwrap_err();
        assert_eq!(error.code(), bad.code());
    }

    #[test]
    fn stored_damaged_value_is_detected() {
        let mut settings = EffectiveSettings::default();
        assert!(
            settings
                .apply_stored(keys::DOWNLOADS_MAX_CONCURRENT, &json!(0))
                .is_err()
        );
        assert!(
            settings
                .apply_stored(keys::UI_BACKGROUND_FPS, &json!("sixty"))
                .is_err()
        );
    }

    #[test]
    fn group_defaults_reset_only_one_group() {
        let current = EffectiveSettings {
            ui: UiSettings {
                theme: Theme::Light,
                ..UiSettings::default()
            },
            downloads: DownloadSettings {
                max_concurrent: 12,
                ..DownloadSettings::default()
            },
            ..EffectiveSettings::default()
        };
        let defaults = EffectiveSettings::group_defaults(SettingsGroup::Ui);
        assert_eq!(defaults.ui.theme, Theme::Dark);
        assert_eq!(defaults.downloads.max_concurrent, 4);
        assert_eq!(current.downloads.max_concurrent, 12);
    }

    #[test]
    fn settings_serialize_to_camel_case_for_ipc() {
        let value = serde_json::to_value(EffectiveSettings::default()).unwrap();
        assert!(value.get("downloads").is_some());
        assert!(value["downloads"].get("maxConcurrent").is_some());
        assert!(value["ui"].get("backgroundFps").is_some());
    }

    #[test]
    fn patch_rejects_unknown_command_arguments() {
        let unknown_top = serde_json::json!({ "ui": { "theme": "dark" }, "shell": "cmd" });
        assert!(serde_json::from_value::<SettingsPatch>(unknown_top).is_err());

        let unknown_nested = serde_json::json!({ "ui": { "theme": "dark", "exec": "calc.exe" } });
        assert!(serde_json::from_value::<SettingsPatch>(unknown_nested).is_err());

        let valid = serde_json::json!({ "ui": { "theme": "dark" } });
        assert!(serde_json::from_value::<SettingsPatch>(valid).is_ok());
    }
}
