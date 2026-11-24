//! Конфигурация NEXUS AI Runtime и защищённое хранение device token.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub provider: ProviderConfig,
    pub limits: LimitsConfig,
    pub programs: Vec<ProgramConfig>,
    pub logging: LoggingConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// WSS endpoint NEXUS: wss://host/api/v1/ai/runtime/connect
    pub url: String,
    /// HTTP(S) база NEXUS для enrollment: https://host
    pub enroll_url: String,
    pub name: String,
    pub allow_insecure_ws: bool,
    pub heartbeat_seconds: u64,
    pub reconnect_min_seconds: u64,
    pub reconnect_max_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub state_dir: String,
    pub token_file: String,
    pub allowed_roots: Vec<String>,
    pub log_dir: String,
    pub receipts_file: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub timeout_seconds: u64,
    pub connect_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    pub max_parallel_tasks: usize,
    pub max_tool_output_bytes: usize,
    pub max_file_bytes: u64,
    pub max_search_files: usize,
    pub max_search_results: usize,
    pub max_directory_entries: usize,
    pub default_tool_timeout_seconds: u64,
    pub env_allowlist: Vec<String>,
    pub skip_directories: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ProgramConfig {
    pub program_id: String,
    pub name: String,
    pub executable: String,
    pub version: String,
    pub risk: String,
    /// Если пусто и allow_any_args=false — программа запускается без аргументов.
    pub arg_patterns: Vec<String>,
    pub allow_any_args: bool,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
    /// deny (по умолчанию) — временный outbound block через Windows Firewall.
    pub network_policy: String,
    /// Лимит памяти процесса в байтах (0 — без лимита).
    pub max_memory_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub keep_files: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: "ws://127.0.0.1:8080/api/v1/ai/runtime/connect".to_owned(),
            enroll_url: "http://127.0.0.1:8080".to_owned(),
            name: "".to_owned(),
            allow_insecure_ws: false,
            heartbeat_seconds: 20,
            reconnect_min_seconds: 2,
            reconnect_max_seconds: 60,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080/v1".to_owned(),
            api_key: String::new(),
            timeout_seconds: 180,
            connect_timeout_seconds: 10,
        }
    }
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_parallel_tasks: 4,
            max_tool_output_bytes: 262_144,
            max_file_bytes: 16 * 1024 * 1024,
            max_search_files: 2_000,
            max_search_results: 200,
            max_directory_entries: 1_000,
            default_tool_timeout_seconds: 120,
            env_allowlist: vec![
                "PATH".into(),
                "SystemRoot".into(),
                "SystemDrive".into(),
                "TEMP".into(),
                "TMP".into(),
                "USERPROFILE".into(),
                "HOMEDRIVE".into(),
                "HOMEPATH".into(),
                "PATHEXT".into(),
                "NUMBER_OF_PROCESSORS".into(),
                "PROCESSOR_ARCHITECTURE".into(),
            ],
            skip_directories: vec![
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "dist".into(),
                ".venv".into(),
                "__pycache__".into(),
            ],
        }
    }
}

impl Default for ProgramConfig {
    fn default() -> Self {
        Self {
            program_id: String::new(),
            name: String::new(),
            executable: String::new(),
            version: String::new(),
            risk: "high".to_owned(),
            arg_patterns: Vec::new(),
            allow_any_args: false,
            timeout_seconds: 300,
            max_output_bytes: 262_144,
            network_policy: "deny".to_owned(),
            max_memory_bytes: 0,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            keep_files: 14,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCredentials {
    pub node_id: Uuid,
    pub device_token: String,
    pub server_url: String,
    pub enrolled_at: chrono::DateTime<chrono::Utc>,
    pub runtime_name: String,
}

impl RuntimeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("не удалось прочитать конфигурацию {}", path.display()))?;
        let config: RuntimeConfig = toml::from_str(&raw)
            .with_context(|| format!("некорректный TOML в {}", path.display()))?;
        let mut config = config;
        if let Ok(value) = std::env::var("NEXUS_AI_PROVIDER_API_KEY") {
            if !value.trim().is_empty() {
                config.provider.api_key = value;
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.server.name.trim().is_empty() {
            bail!("server.name не задан: укажите имя компьютера Runtime");
        }
        let url =
            Url::parse(self.server.url.trim()).context("server.url должен быть корректным URL")?;
        match url.scheme() {
            "wss" => {}
            "ws" if self.server.allow_insecure_ws => {
                tracing::warn!(
                    "server.url использует незашифрованный ws:// (только для локальной отладки)"
                );
            }
            "ws" => bail!(
                "server.url использует ws://. Для production требуется wss://; \
                 для локальной отладки явно задайте allow_insecure_ws = true"
            ),
            other => bail!("server.url: неподдерживаемая схема {other}"),
        }
        if !self.server.enroll_url.trim().is_empty() {
            let enroll = Url::parse(self.server.enroll_url.trim())
                .context("server.enroll_url должен быть корректным URL")?;
            if !matches!(enroll.scheme(), "http" | "https") {
                bail!("server.enroll_url должен использовать http или https");
            }
        }
        let provider = Url::parse(self.provider.base_url.trim())
            .context("provider.base_url должен быть корректным URL")?;
        if !matches!(provider.scheme(), "http" | "https") {
            bail!("provider.base_url должен использовать http или https");
        }
        if self.storage.allowed_roots.is_empty() {
            bail!(
                "storage.allowed_roots не задан: workspace обязан находиться в разрешённом корне"
            );
        }
        for root in &self.storage.allowed_roots {
            let path = Path::new(root);
            if !path.is_absolute() {
                bail!("allowed_roots: путь {root} должен быть абсолютным");
            }
        }
        if !(1..=16).contains(&self.limits.max_parallel_tasks) {
            bail!("limits.max_parallel_tasks должен быть 1..=16");
        }
        if !(5..=600).contains(&self.server.heartbeat_seconds) {
            bail!("server.heartbeat_seconds должен быть 5..=600");
        }
        if self.provider.timeout_seconds < 5 || self.provider.timeout_seconds > 1800 {
            bail!("provider.timeout_seconds должен быть 5..=1800");
        }
        let mut ids = std::collections::HashSet::new();
        for program in &self.programs {
            if program.program_id.trim().is_empty() {
                bail!("programs: program_id обязателен");
            }
            if !ids.insert(program.program_id.clone()) {
                bail!("programs: повторный program_id {}", program.program_id);
            }
            if program.executable.trim().is_empty() {
                bail!("programs: executable обязателен для {}", program.program_id);
            }
            if !matches!(program.network_policy.as_str(), "allow" | "deny") {
                bail!(
                    "programs.{}: network_policy должен быть allow или deny",
                    program.program_id
                );
            }
        }
        Ok(())
    }

    pub fn state_dir(&self) -> PathBuf {
        PathBuf::from(if self.storage.state_dir.trim().is_empty() {
            "state"
        } else {
            &self.storage.state_dir
        })
    }

    pub fn token_path(&self) -> PathBuf {
        self.state_dir()
            .join(if self.storage.token_file.trim().is_empty() {
                "device.json"
            } else {
                &self.storage.token_file
            })
    }

    pub fn receipts_path(&self) -> PathBuf {
        self.state_dir()
            .join(if self.storage.receipts_file.trim().is_empty() {
                "receipts.jsonl"
            } else {
                &self.storage.receipts_file
            })
    }

    /// Канонические разрешённые корни. Несуществующий корень — ошибка старта.
    pub fn canonical_roots(&self) -> Result<Vec<PathBuf>> {
        let mut roots = Vec::new();
        for root in &self.storage.allowed_roots {
            let canonical = std::fs::canonicalize(root)
                .with_context(|| format!("allowed_roots: каталог {root} недоступен"))?;
            if !canonical.is_dir() {
                bail!("allowed_roots: {root} не является каталогом");
            }
            roots.push(canonical);
        }
        Ok(roots)
    }

    pub fn load_credentials(&self) -> Result<DeviceCredentials> {
        let path = self.token_path();
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "device token не найден ({}). Выполните: nexus-ai-runtime enroll --code <code>",
                path.display()
            )
        })?;
        let credentials: DeviceCredentials = serde_json::from_str(&raw)
            .with_context(|| format!("некорректный файл device token {}", path.display()))?;
        if credentials.device_token.trim().is_empty() || credentials.node_id.is_nil() {
            bail!("device token повреждён: повторно выполните enroll");
        }
        Ok(credentials)
    }

    pub fn store_credentials(&self, credentials: &DeviceCredentials) -> Result<()> {
        let dir = self.state_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("не удалось создать {}", dir.display()))?;
        let path = self.token_path();
        let temp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(credentials)?;
        write_private_file(&temp, &json)?;
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::rename(&temp, &path)
            .with_context(|| format!("не удалось сохранить {}", path.display()))?;
        restrict_permissions(&path)?;
        Ok(())
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("не удалось открыть {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// На Windows выставляем ACL каталога состояния в установочном скрипте.
/// Здесь дополнительно снимаем наследование с файла токена, если доступен icacls.
fn restrict_permissions(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_default();
        if user.is_empty() {
            return Ok(());
        }
        let status = std::process::Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:R,W"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Ok(status) = status {
            if !status.success() {
                tracing::warn!(path = %path.display(), "не удалось ограничить ACL файла device token");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RuntimeConfig {
        RuntimeConfig {
            server: ServerConfig {
                name: "TEST-PC".into(),
                url: "wss://nexus.example/api/v1/ai/runtime/connect".into(),
                enroll_url: "https://nexus.example".into(),
                ..Default::default()
            },
            storage: StorageConfig {
                allowed_roots: vec![std::env::temp_dir().to_string_lossy().to_string()],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn rejects_ws_without_explicit_opt_in() {
        let mut config = base();
        config.server.url = "ws://127.0.0.1:8080/api/v1/ai/runtime/connect".into();
        assert!(config.validate().is_err());
        config.server.allow_insecure_ws = true;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_programs() {
        let mut config = base();
        config.programs = vec![
            ProgramConfig {
                program_id: "git".into(),
                executable: "C:\\git.exe".into(),
                ..Default::default()
            },
            ProgramConfig {
                program_id: "git".into(),
                executable: "C:\\git.exe".into(),
                ..Default::default()
            },
        ];
        assert!(config.validate().is_err());
    }

    #[test]
    fn requires_workspace_roots() {
        let mut config = base();
        config.storage.allowed_roots.clear();
        assert!(config.validate().is_err());
    }
}
