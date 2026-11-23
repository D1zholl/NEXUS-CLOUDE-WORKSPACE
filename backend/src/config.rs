use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Технический максимум одного чанка и тела запроса для модулей загрузки.
pub const MAX_CHUNK_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub security: SecurityConfig,
    pub storage: StorageConfig,
    pub launcher: LauncherConfig,
    pub logging: LoggingConfig,
}

/// Лимиты Launcher API. Значения применяются при старте службы.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    /// Запросов каталога/манифестов в минуту на токен (всплеск равен лимиту).
    pub requests_per_minute: u32,
    /// Скачиваний артефактов в минуту на токен (всплеск равен лимиту).
    pub artifacts_per_minute: u32,
    /// Сколько артефактов один токен может скачивать одновременно.
    pub max_parallel_downloads: usize,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: crate::auth::rate_limit::LAUNCHER_REQUESTS_PER_MINUTE,
            artifacts_per_minute: crate::auth::rate_limit::LAUNCHER_ARTIFACTS_PER_MINUTE,
            max_parallel_downloads: crate::auth::rate_limit::LAUNCHER_MAX_PARALLEL_DOWNLOADS,
        }
    }
}

impl LauncherConfig {
    /// Переопределяет лимиты переменными окружения.
    ///
    /// Поиск вынесен в параметр, чтобы правило разбора проверялось тестом без
    /// изменения глобального окружения процесса.
    fn apply_env(&mut self, lookup: impl Fn(&str) -> Option<String>) -> Result<()> {
        if let Some(value) = lookup("NEXUS_LAUNCHER_RPM") {
            self.requests_per_minute = value
                .parse()
                .context("NEXUS_LAUNCHER_RPM должен быть числом")?;
        }
        if let Some(value) = lookup("NEXUS_LAUNCHER_ARTIFACTS_RPM") {
            self.artifacts_per_minute = value
                .parse()
                .context("NEXUS_LAUNCHER_ARTIFACTS_RPM должен быть числом")?;
        }
        Ok(())
    }
}

/// Настройки логирования в файл (для службы Windows).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Каталог для файлов журнала. Пусто — журнал только в stderr.
    pub dir: String,
    /// Имя файла журнала внутри каталога.
    pub file: String,
    /// Сколько последних файлов хранить при ротации.
    pub keep_files: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            dir: String::new(),
            file: "nexus.log".to_owned(),
            keep_files: 7,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub public_url: String,
    pub static_dir: String,
    pub trust_proxy: bool,
}

/// Настройки Storage Engine и загрузок Cloud Storage.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Максимальный размер одного файла.
    pub max_upload_bytes: u64,
    /// Максимальный размер одного чанка возобновляемой загрузки.
    pub chunk_max_bytes: u64,
    /// Через сколько часов незавершённая загрузка считается просроченной.
    pub upload_ttl_hours: i64,
    /// Срок действия ссылки общего доступа по умолчанию (часы).
    pub share_default_hours: i64,
    /// Через сколько часов удаляется готовый ZIP-архив массового скачивания.
    pub export_ttl_hours: i64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            max_upload_bytes: 10 * 1024 * 1024 * 1024,
            chunk_max_bytes: 64 * 1024 * 1024,
            upload_ttl_hours: 24,
            share_default_hours: 24 * 7,
            export_ttl_hours: 6,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            public_url: "http://localhost:8080".into(),
            static_dir: "../frontend/dist".into(),
            trust_proxy: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub session_ttl_hours: i64,
    /// Минимальная длина пароля для CLI. API читает значение из `system_settings`.
    pub password_min_length: usize,
    pub cookie_secure: bool,
    pub secret: String,
    pub allowed_origins: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            session_ttl_hours: 24,
            password_min_length: crate::auth::password::MIN_PASSWORD_LEN,
            cookie_secure: false,
            secret: String::new(),
            allowed_origins: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = env::var("NEXUS_CONFIG").unwrap_or_else(|_| "nexus.toml".to_string());

        let mut config = if Path::new(&path).exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("не удалось прочитать конфигурацию {path}"))?;
            toml::from_str(&raw).with_context(|| format!("некорректный TOML в {path}"))?
        } else {
            Config::default()
        };

        if let Ok(url) = env::var("DATABASE_URL") {
            config.database.url = url;
        }
        if let Ok(v) = env::var("NEXUS_HOST") {
            config.server.host = v;
        }
        if let Ok(v) = env::var("NEXUS_PORT") {
            config.server.port = v.parse().context("NEXUS_PORT должен быть числом")?;
        }
        if let Ok(v) = env::var("NEXUS_PUBLIC_URL") {
            config.server.public_url = v;
        }
        if let Ok(v) = env::var("NEXUS_STATIC_DIR") {
            config.server.static_dir = v;
        }
        if let Ok(v) = env::var("NEXUS_TRUST_PROXY") {
            config.server.trust_proxy = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = env::var("NEXUS_COOKIE_SECURE") {
            config.security.cookie_secure = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = env::var("NEXUS_SESSION_TTL_HOURS") {
            config.security.session_ttl_hours = v.parse().context("NEXUS_SESSION_TTL_HOURS")?;
        }
        if let Ok(v) = env::var("NEXUS_PASSWORD_MIN_LENGTH") {
            config.security.password_min_length = v
                .parse()
                .context("NEXUS_PASSWORD_MIN_LENGTH должен быть числом")?;
        }
        if let Ok(v) = env::var("NEXUS_SECRET") {
            config.security.secret = v;
        }
        if let Ok(v) = env::var("NEXUS_CORS_ORIGINS") {
            config.security.allowed_origins = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
        if let Ok(v) = env::var("NEXUS_MAX_UPLOAD_MB") {
            let mb: u64 = v
                .parse()
                .context("NEXUS_MAX_UPLOAD_MB должен быть числом")?;
            config.storage.max_upload_bytes = mb * 1024 * 1024;
        }
        if let Ok(v) = env::var("NEXUS_UPLOAD_TTL_HOURS") {
            config.storage.upload_ttl_hours = v.parse().context("NEXUS_UPLOAD_TTL_HOURS")?;
        }
        if let Ok(v) = env::var("NEXUS_EXPORT_TTL_HOURS") {
            config.storage.export_ttl_hours = v.parse().context("NEXUS_EXPORT_TTL_HOURS")?;
        }
        if let Ok(v) = env::var("NEXUS_LOG_DIR") {
            config.logging.dir = v;
        }
        if let Ok(v) = env::var("NEXUS_LOG_FILE") {
            config.logging.file = v;
        }
        if let Ok(v) = env::var("NEXUS_LOG_KEEP") {
            config.logging.keep_files = v.parse().context("NEXUS_LOG_KEEP")?;
        }
        // Служба Windows получает переменные среды уровня машины и .env из
        // каталога установки; переменные пользователя, заданные через `setx`
        // в интерактивной сессии, до службы не доходят.
        config.launcher.apply_env(|key| env::var(key).ok())?;

        if config.database.url.trim().is_empty() {
            return Err(anyhow!(
                "DATABASE_URL не задан. Укажите строку подключения PostgreSQL в .env или nexus.toml"
            ));
        }
        if config.storage.max_upload_bytes < 1024 * 1024 {
            return Err(anyhow!("max_upload_bytes должен быть не менее 1 МБ"));
        }
        if config.storage.chunk_max_bytes > MAX_CHUNK_BYTES {
            return Err(anyhow!(
                "chunk_max_bytes не может превышать {} МБ",
                MAX_CHUNK_BYTES / (1024 * 1024)
            ));
        }
        if config.storage.upload_ttl_hours < 1 || config.storage.upload_ttl_hours > 24 * 30 {
            return Err(anyhow!("upload_ttl_hours должен быть в диапазоне 1..=720"));
        }
        if !(1..=1_000_000).contains(&config.launcher.requests_per_minute) {
            return Err(anyhow!(
                "requests_per_minute должен быть в диапазоне 1..=1000000"
            ));
        }
        if !(1..=1_000_000).contains(&config.launcher.artifacts_per_minute) {
            return Err(anyhow!(
                "artifacts_per_minute должен быть в диапазоне 1..=1000000"
            ));
        }
        if !(1..=1024).contains(&config.launcher.max_parallel_downloads) {
            return Err(anyhow!(
                "max_parallel_downloads должен быть в диапазоне 1..=1024"
            ));
        }
        if config.security.session_ttl_hours < 1 || config.security.session_ttl_hours > 24 * 30 {
            return Err(anyhow!("session_ttl_hours должен быть в диапазоне 1..=720"));
        }
        if !(crate::auth::password::PASSWORD_MIN_ALLOWED
            ..=crate::auth::password::PASSWORD_MAX_ALLOWED)
            .contains(&config.security.password_min_length)
        {
            return Err(anyhow!(
                "password_min_length должен быть в диапазоне {}..={}",
                crate::auth::password::PASSWORD_MIN_ALLOWED,
                crate::auth::password::PASSWORD_MAX_ALLOWED
            ));
        }
        if config.security.secret.trim().is_empty() {
            let mut bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut bytes);
            config.security.secret = hex::encode(bytes);
            tracing::warn!(
                "NEXUS_SECRET не задан — сгенерирован временный секрет. После перезапуска \
                 CSRF-токены станут недействительны. Задайте NEXUS_SECRET для production."
            );
        }
        if config.security.secret.len() < 32 {
            return Err(anyhow!(
                "NEXUS_SECRET должен содержать не менее 32 символов"
            ));
        }

        Ok(config)
    }

    pub fn resolved_static_dir(&self) -> Option<PathBuf> {
        let configured = PathBuf::from(&self.server.static_dir);
        let candidates = if configured.is_absolute() {
            vec![configured]
        } else {
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            vec![cwd.join(&configured), cwd.join("..").join(&configured)]
        };
        candidates
            .into_iter()
            .find(|p| p.join("index.html").exists())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicConfig {
    pub public_url: String,
    pub cookie_secure: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_defaults_match_documented_limits() {
        let launcher = LauncherConfig::default();
        assert_eq!(launcher.requests_per_minute, 120);
        assert_eq!(launcher.artifacts_per_minute, 600);
        assert_eq!(launcher.max_parallel_downloads, 8);
    }

    #[test]
    fn launcher_env_overrides_are_parsed() {
        let mut launcher = LauncherConfig::default();
        launcher
            .apply_env(|key| match key {
                "NEXUS_LAUNCHER_RPM" => Some("42".to_owned()),
                "NEXUS_LAUNCHER_ARTIFACTS_RPM" => Some("84".to_owned()),
                _ => None,
            })
            .expect("значения разбираются");
        assert_eq!(launcher.requests_per_minute, 42);
        assert_eq!(launcher.artifacts_per_minute, 84);
        // Не заданная переменная не меняет значение по умолчанию.
        assert_eq!(launcher.max_parallel_downloads, 8);
    }

    #[test]
    fn invalid_launcher_env_is_rejected() {
        let mut launcher = LauncherConfig::default();
        let error = launcher
            .apply_env(|key| (key == "NEXUS_LAUNCHER_RPM").then(|| "fast".to_owned()))
            .expect_err("нечисловое значение отклоняется");
        assert!(error.to_string().contains("NEXUS_LAUNCHER_RPM"));
    }

    #[test]
    fn launcher_toml_section_is_deserialized() {
        let config: Config = toml::from_str(
            "[launcher]\nrequests_per_minute = 30\nartifacts_per_minute = 90\n\
             max_parallel_downloads = 4\n",
        )
        .expect("TOML разбирается");
        assert_eq!(config.launcher.requests_per_minute, 30);
        assert_eq!(config.launcher.artifacts_per_minute, 90);
        assert_eq!(config.launcher.max_parallel_downloads, 4);
        // Остальные секции заполняются значениями по умолчанию.
        assert_eq!(config.server.port, 8080);
    }
}
