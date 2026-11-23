//! Bootstrap configuration read from `config.toml`.
//!
//! Only settings that are required *before* the database is open live here:
//! network limits, download limits, logging and security flags. User-facing
//! settings (theme, language, paths, ...) are stored in SQLite.
//!
//! The file is versioned with `schema_version`. Unknown fields are preserved on
//! save, so an older launcher does not silently drop configuration written by a
//! newer one. A corrupt file is copied aside before defaults are written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::NexusError;
use crate::paths::{atomic_write, backup_file, quarantine_file};
use crate::settings::{DownloadSettings, EffectiveSettings, LogLevel, LoggingSettings};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub downloads: DownloadConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    /// Unknown/future keys. Preserved on save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

pub const fn default_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            network: NetworkConfig::default(),
            downloads: DownloadConfig::default(),
            logging: LoggingConfig::default(),
            security: SecurityConfig::default(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u8,
}

pub const fn default_connect_timeout_secs() -> u64 {
    30
}

pub const fn default_request_timeout_secs() -> u64 {
    300
}

pub const fn default_max_redirects() -> u8 {
    3
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout_secs(),
            request_timeout_secs: default_request_timeout_secs(),
            max_redirects: default_max_redirects(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadConfig {
    #[serde(default = "default_config_max_concurrent")]
    pub max_concurrent: u8,
    #[serde(default = "default_config_max_speed_kbps")]
    pub max_speed_kbps: u32,
    #[serde(default = "default_retries")]
    pub retries: u8,
}

pub const fn default_config_max_concurrent() -> u8 {
    4
}

pub const fn default_config_max_speed_kbps() -> u32 {
    0
}

pub const fn default_retries() -> u8 {
    3
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_concurrent: default_config_max_concurrent(),
            max_speed_kbps: default_config_max_speed_kbps(),
            retries: default_retries(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_logging_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default = "default_log_max_file_mb")]
    pub max_file_size_mb: u64,
    #[serde(default = "default_log_retention_days")]
    pub retention_days: u16,
}

pub const fn default_logging_enabled() -> bool {
    true
}

pub const fn default_log_max_file_mb() -> u64 {
    20
}

pub const fn default_log_retention_days() -> u16 {
    14
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: default_logging_enabled(),
            level: LogLevel::default(),
            max_file_size_mb: default_log_max_file_mb(),
            retention_days: default_log_retention_days(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_verify_tls")]
    pub verify_tls: bool,
    /// Explicit opt-in for private/LAN and loopback NEXUS Workspace sources.
    #[serde(default)]
    pub allow_local_nexus_sources: bool,
    /// Explicit opt-in for plain HTTP to a self-hosted NEXUS Workspace.
    /// HTTPS remains the default and must never be disabled globally.
    #[serde(default)]
    pub allow_insecure_http_nexus_sources: bool,
}

pub const fn default_verify_tls() -> bool {
    true
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            verify_tls: default_verify_tls(),
            allow_local_nexus_sources: false,
            allow_insecure_http_nexus_sources: false,
        }
    }
}

impl BootstrapConfig {
    pub fn validate(&self) -> Result<(), NexusError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(NexusError::config_invalid(format!(
                "schema_version {} is not supported (expected {})",
                self.schema_version, CONFIG_SCHEMA_VERSION
            )));
        }
        if !(1..=16).contains(&self.downloads.max_concurrent) {
            return Err(NexusError::config_invalid(
                "downloads.max_concurrent must be between 1 and 16",
            ));
        }
        if self.downloads.max_speed_kbps > 1_000_000 {
            return Err(NexusError::config_invalid(
                "downloads.max_speed_kbps is out of range",
            ));
        }
        if self.downloads.retries > 10 {
            return Err(NexusError::config_invalid(
                "downloads.retries must be between 0 and 10",
            ));
        }
        if self.network.connect_timeout_secs == 0 || self.network.connect_timeout_secs > 3_600 {
            return Err(NexusError::config_invalid(
                "network.connect_timeout_secs is out of range",
            ));
        }
        if self.network.request_timeout_secs == 0 || self.network.request_timeout_secs > 86_400 {
            return Err(NexusError::config_invalid(
                "network.request_timeout_secs is out of range",
            ));
        }
        if self.network.max_redirects > 10 {
            return Err(NexusError::config_invalid(
                "network.max_redirects must be between 0 and 10",
            ));
        }
        if self.logging.max_file_size_mb == 0 || self.logging.max_file_size_mb > 512 {
            return Err(NexusError::config_invalid(
                "logging.max_file_size_mb is out of range",
            ));
        }
        if self.logging.retention_days == 0 || self.logging.retention_days > 365 {
            return Err(NexusError::config_invalid(
                "logging.retention_days is out of range",
            ));
        }
        if !self.security.verify_tls {
            return Err(NexusError::config_invalid(
                "security.verify_tls=false is not allowed",
            ));
        }
        Ok(())
    }

    /// Bootstrap values used as the base layer for effective settings.
    ///
    /// Priority is `defaults -> config.toml -> app_settings`: values stored by
    /// the user in SQLite always win, but after `settings_reset_group` the
    /// bootstrap value becomes visible again.
    pub fn settings_base(&self) -> EffectiveSettings {
        EffectiveSettings {
            downloads: DownloadSettings {
                max_concurrent: self.downloads.max_concurrent,
                max_speed_kbps: self.downloads.max_speed_kbps,
            },
            logging: LoggingSettings {
                enabled: self.logging.enabled,
                level: self.logging.level,
            },
            ..EffectiveSettings::default()
        }
    }

    /// Atomically saves the config and keeps the previous revision as `.bak`.
    pub fn save_to(&self, path: &Path) -> Result<(), NexusError> {
        self.validate()?;
        let text = toml::to_string_pretty(self)
            .map_err(|error| NexusError::config_write(error.to_string()))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                NexusError::config_write(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        backup_file(path)
            .map_err(|error| NexusError::config_write(format!("cannot backup config: {error}")))?;
        atomic_write(path, text.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: BootstrapConfig,
    pub path: PathBuf,
    pub created: bool,
    /// Set when the previous file could not be parsed and was copied aside.
    pub quarantined: Option<PathBuf>,
}

/// Loads `config.toml`, creating it with defaults when it does not exist.
///
/// * missing file -> defaults are written;
/// * invalid TOML -> the file is quarantined and defaults are written;
/// * valid but unsupported `schema_version` -> error, file is left untouched.
pub fn load_or_init(path: &Path) -> Result<LoadedConfig, NexusError> {
    match fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<BootstrapConfig>(&text) {
            Ok(config) => {
                config.validate()?;
                Ok(LoadedConfig {
                    config,
                    path: path.to_path_buf(),
                    created: false,
                    quarantined: None,
                })
            }
            Err(error) => {
                let quarantined = quarantine_file(path, "corrupt").ok().flatten();
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    quarantined = ?quarantined,
                    "bootstrap config is invalid; writing defaults"
                );
                let config = BootstrapConfig::default();
                config.save_to(path)?;
                Ok(LoadedConfig {
                    config,
                    path: path.to_path_buf(),
                    created: true,
                    quarantined,
                })
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = BootstrapConfig::default();
            config.save_to(path)?;
            Ok(LoadedConfig {
                config,
                path: path.to_path_buf(),
                created: true,
                quarantined: None,
            })
        }
        Err(error) => Err(NexusError::config_invalid(format!(
            "cannot read {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_round_trips() {
        let config = BootstrapConfig::default();
        config.validate().unwrap();
        let text = toml::to_string_pretty(&config).unwrap();
        let back: BootstrapConfig = toml::from_str(&text).unwrap();
        assert_eq!(config, back);
        assert!(text.contains("schema_version = 1"));
        assert!(text.contains("verify_tls = true"));
    }

    #[test]
    fn unknown_fields_are_preserved() {
        let text = r#"
schema_version = 1

[future]
enabled = true
name = "feature"
"#;
        let config: BootstrapConfig = toml::from_str(text).unwrap();
        config.validate().unwrap();
        let saved = toml::to_string_pretty(&config).unwrap();
        assert!(saved.contains("[future]"));
        assert!(saved.contains("enabled = true"));
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let config = BootstrapConfig {
            schema_version: 99,
            ..BootstrapConfig::default()
        };
        let error = config.validate().unwrap_err();
        assert_eq!(error.code(), crate::ErrorCode::ConfigInvalid);
    }

    #[test]
    fn bootstrap_values_form_the_settings_base_layer() {
        let config = BootstrapConfig {
            downloads: DownloadConfig {
                max_concurrent: 9,
                max_speed_kbps: 512,
                retries: 3,
            },
            logging: LoggingConfig {
                enabled: false,
                level: LogLevel::Debug,
                max_file_size_mb: 20,
                retention_days: 14,
            },
            ..BootstrapConfig::default()
        };
        let base = config.settings_base();
        assert_eq!(base.downloads.max_concurrent, 9);
        assert_eq!(base.downloads.max_speed_kbps, 512);
        assert!(!base.logging.enabled);
        assert_eq!(base.logging.level, LogLevel::Debug);
        assert_eq!(base.ui.background_fps, 60);
    }

    #[test]
    fn tls_cannot_be_disabled() {
        let mut config = BootstrapConfig::default();
        config.security.verify_tls = false;
        assert!(config.validate().is_err());
    }

    #[test]
    fn nexus_self_hosted_exception_is_explicit() {
        let defaults = BootstrapConfig::default();
        assert!(!defaults.security.allow_local_nexus_sources);
        assert!(!defaults.security.allow_insecure_http_nexus_sources);

        let text = r#"
schema_version = 1

[security]
verify_tls = true
allow_local_nexus_sources = true
allow_insecure_http_nexus_sources = true
"#;
        let config: BootstrapConfig = toml::from_str(text).unwrap();
        assert!(config.security.allow_local_nexus_sources);
        assert!(config.security.allow_insecure_http_nexus_sources);
    }

    #[test]
    fn missing_file_is_created_and_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let loaded = load_or_init(&path).unwrap();
        assert!(loaded.created);
        assert!(path.is_file());
        let again = load_or_init(&path).unwrap();
        assert!(!again.created);
        assert_eq!(loaded.config, again.config);
    }

    #[test]
    fn corrupt_file_is_quarantined_and_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, b"not = [valid").unwrap();
        let loaded = load_or_init(&path).unwrap();
        assert!(loaded.created);
        assert!(loaded.quarantined.as_ref().is_some_and(|p| p.is_file()));
        assert!(path.is_file());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            toml::to_string_pretty(&BootstrapConfig::default()).unwrap()
        );
    }

    #[test]
    fn newer_schema_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let original = "schema_version = 42\n";
        fs::write(&path, original).unwrap();
        let error = load_or_init(&path).unwrap_err();
        assert_eq!(error.code(), crate::ErrorCode::ConfigInvalid);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn save_keeps_previous_revision_in_bak() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut first = BootstrapConfig::default();
        first.downloads.max_concurrent = 2;
        first.save_to(&path).unwrap();
        let mut second = BootstrapConfig::default();
        second.downloads.max_concurrent = 9;
        second.save_to(&path).unwrap();

        let backup = temp.path().join("config.toml.bak");
        assert!(backup.is_file());
        let previous: BootstrapConfig =
            toml::from_str(&fs::read_to_string(backup).unwrap()).unwrap();
        assert_eq!(previous.downloads.max_concurrent, 2);
        let current: BootstrapConfig = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(current.downloads.max_concurrent, 9);
    }
}
