use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VERSION: &str = "1.0.0";

/// Корень Standalone определяется по расположению NEXUS.exe, а не по текущему
/// каталогу процесса: ярлык, BAT-файл или консоль могут стартовать из любого места.
pub fn application_root() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("не удалось определить путь NEXUS.exe")?;
    let dir = exe
        .parent()
        .context("NEXUS.exe не имеет родительского каталога")?;
    Ok(dir.to_path_buf())
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub data_postgres: PathBuf,
    pub storage: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
    pub backups: PathBuf,
    pub run: PathBuf,
    pub database_bin: PathBuf,
    pub backend_exe: PathBuf,
    pub frontend: PathBuf,
    pub runtime_exe: PathBuf,
    pub secrets: PathBuf,
    pub instance: PathBuf,
    pub nexus_toml: PathBuf,
    pub state: PathBuf,
    pub launcher_log: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Result<Self> {
        let root = application_root()?;
        let config = root.join("config");
        let data = root.join("data");
        let logs = root.join("logs");
        let run = data.join("run");
        let database_bin = resolve_postgres_bin(&root)?;
        let paths = Self {
            config: config.clone(),
            data: data.clone(),
            data_postgres: data.join("postgres"),
            storage: root.join("storage"),
            logs: logs.clone(),
            cache: root.join("cache"),
            backups: root.join("backups"),
            run: run.clone(),
            database_bin,
            backend_exe: root.join("backend").join("nexus.exe"),
            frontend: root.join("frontend"),
            runtime_exe: root.join("runtime").join("nexus-ai-runtime.exe"),
            secrets: config.join("secrets.json"),
            instance: config.join("instance.json"),
            nexus_toml: config.join("nexus.toml"),
            state: run.join("state.json"),
            launcher_log: logs.join("launcher.log"),
            root,
        };
        Ok(paths)
    }

    pub fn ensure_directories(&self) -> Result<()> {
        for dir in [
            &self.config,
            &self.data,
            &self.data_postgres,
            &self.storage,
            &self.logs,
            &self.cache,
            &self.backups,
            &self.run,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("не удалось создать каталог {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn shutdown_file(&self) -> PathBuf {
        self.run.join("shutdown.request")
    }
    pub fn stop_request(&self) -> PathBuf {
        self.run.join("stop.request")
    }
    pub fn restart_request(&self) -> PathBuf {
        self.run.join("restart.request")
    }
    pub fn open_request(&self) -> PathBuf {
        self.run.join("open.request")
    }
    pub fn backup_request(&self) -> PathBuf {
        self.run.join("backup.request")
    }
    pub fn postgres_log(&self) -> PathBuf {
        self.logs.join("database.log")
    }
    pub fn pg_ctl_log(&self) -> PathBuf {
        self.logs.join("pg_ctl.log")
    }
    pub fn backend_log(&self) -> PathBuf {
        self.logs.join("backend.log")
    }
    pub fn backend_pid_file(&self) -> PathBuf {
        self.run.join("backend.pid")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceConfig {
    pub config_version: u32,
    pub app_port: u16,
    pub db_port: u16,
    pub public_url: String,
    pub version: String,
    pub created_at: String,
    pub last_started_at: String,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            app_port: 8080,
            db_port: 55432,
            public_url: "http://127.0.0.1:8080".into(),
            version: VERSION.into(),
            created_at: String::new(),
            last_started_at: String::new(),
        }
    }
}

impl InstanceConfig {
    /// Повреждённый instance.json не удаляет данные: файл переименовывается,
    /// создаётся новый с безопасными значениями по умолчанию.
    pub fn load_or_create(paths: &AppPaths) -> (Self, Option<String>) {
        match fs::read_to_string(&paths.instance) {
            Ok(raw) => match serde_json::from_str::<Self>(&raw) {
                Ok(mut cfg) => {
                    if cfg.app_port == 0 {
                        cfg.app_port = 8080;
                    }
                    if cfg.db_port == 0 {
                        cfg.db_port = 55432;
                    }
                    if cfg.public_url.trim().is_empty() {
                        cfg.public_url = format!("http://127.0.0.1:{}", cfg.app_port);
                    }
                    (cfg, None)
                }
                Err(error) => {
                    let backup = paths.instance.with_extension(format!(
                        "broken-{}",
                        chrono::Local::now().format("%Y%m%d-%H%M%S")
                    ));
                    let _ = fs::rename(&paths.instance, &backup);
                    let mut cfg = Self::default();
                    cfg.created_at = chrono::Local::now().to_rfc3339();
                    let _ = cfg.save(paths);
                    (
                        cfg,
                        Some(format!(
                            "config/instance.json повреждён ({error}); создан новый, старый сохранён как {}",
                            backup.file_name().unwrap_or_default().to_string_lossy()
                        )),
                    )
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut cfg = Self::default();
                cfg.created_at = chrono::Local::now().to_rfc3339();
                let _ = cfg.save(paths);
                (cfg, None)
            }
            Err(error) => (
                Self::default(),
                Some(format!("не удалось прочитать config/instance.json: {error}")),
            ),
        }
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        write_private(&paths.instance, json.as_bytes())
    }

    pub fn database_url(&self, password: &str) -> String {
        format!(
            "postgres://nexus:{password}@127.0.0.1:{}/nexus",
            self.db_port
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Secrets {
    pub database_password: String,
    pub nexus_secret: String,
}

impl Secrets {
    pub fn load_or_create(paths: &AppPaths) -> Result<Self> {
        if paths.secrets.exists() {
            // После переноса папки на другой компьютер ACL может ссылаться на
            // старый SID. Пытаемся вернуть доступ текущему пользователю до чтения.
            tighten_acl(&paths.secrets);
            let raw = fs::read_to_string(&paths.secrets)
                .with_context(|| format!("не удалось прочитать {}", paths.secrets.display()))?;
            let secrets: Self = serde_json::from_str(&raw).with_context(|| {
                format!(
                    "файл секретов повреждён: {}. Восстановите его из резервной копии.",
                    paths.secrets.display()
                )
            })?;
            if secrets.database_password.len() < 16 || secrets.nexus_secret.len() < 32 {
                bail!(
                    "файл секретов содержит слишком короткие значения. \
                     Не редактируйте config/secrets.json вручную; восстановите резервную копию."
                );
            }
            return Ok(secrets);
        }

        let secrets = Self {
            database_password: random_hex(24),
            nexus_secret: random_hex(32),
        };
        let json = serde_json::to_string_pretty(&secrets)?;
        write_private(&paths.secrets, json.as_bytes())?;
        Ok(secrets)
    }
}

pub fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buffer);
    let mut out = String::with_capacity(bytes * 2);
    for byte in buffer {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Записывает файл и сразу ограничивает ACL текущим пользователем.
/// Содержимое (пароль БД, NEXUS_SECRET) не попадает в журналы и командную строку.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)
        .with_context(|| format!("не удалось создать {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    tighten_acl(path);
    Ok(())
}

/// PostgreSQL на Windows не умеет запускать свои утилиты, если путь к
/// `database\bin` содержит не-ASCII символы: initdb передаёт путь в bootstrap-
/// backend как UTF-8, хотя получает его в ANSI. Данные и сама папка остаются на
/// месте; для запуска создаётся NTFS junction с ASCII-путём на `database`.
fn resolve_postgres_bin(root: &Path) -> Result<PathBuf> {
    let real_bin = root.join("database").join("bin");
    let root_text = root.to_string_lossy();
    if root_text.is_ascii() {
        return Ok(real_bin);
    }
    let database = root.join("database");
    if !database.exists() {
        return Ok(real_bin);
    }

    let mut hasher = Sha256::new();
    hasher.update(database.to_string_lossy().to_lowercase().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let hash = &hash[..12];

    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(public) = std::env::var("PUBLIC") {
        bases.push(PathBuf::from(public));
    }
    bases.push(PathBuf::from(r"C:\Users\Public"));
    if let Ok(program_data) = std::env::var("ProgramData") {
        bases.push(PathBuf::from(program_data));
    }

    for base in bases {
        if !base.to_string_lossy().is_ascii() {
            continue;
        }
        let link_root = base.join("NEXUS");
        let link = link_root.join(format!("pgbin-{hash}"));
        if link.join("bin").join("initdb.exe").exists() {
            let same = matches!(
                (fs::canonicalize(&link), fs::canonicalize(&database)),
                (Ok(a), Ok(b)) if a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
            );
            if same {
                return Ok(link.join("bin"));
            }
            let _ = fs::remove_dir(&link);
        } else if link.exists() {
            let _ = fs::remove_dir(&link);
        }
        if fs::create_dir_all(&link_root).is_err() {
            continue;
        }
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let created = std::process::Command::new(shell)
            .arg("/D")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&database)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags_no_window()
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if created && link.join("bin").join("initdb.exe").exists() {
            return Ok(link.join("bin"));
        }
    }

    bail!(
        "Каталог приложения содержит не-ASCII символы ({}), а PostgreSQL на Windows \
         не поддерживает такой путь к своим утилитам. Не удалось создать ASCII-junction \
         в C:\\Users\\Public или %ProgramData%. Переместите папку NEXUS в путь только \
         из латинских букв, цифр, пробелов и дефисов (например, D:\\NEXUS).",
        root.display()
    )
}

pub fn tighten_acl(path: &Path) {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) if !domain.trim().is_empty() => format!("{domain}\\{name}"),
        (_, Ok(name)) if !name.trim().is_empty() => name,
        _ => return,
    };
    let _ = std::process::Command::new("icacls.exe")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .creation_flags_no_window()
        .output();
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text).with_context(|| format!("не удалось записать {}", path.display()))
}

/// TOML-строка с экранированием. Пути внутри nexus.toml относительные,
/// поэтому перенос папки ничего не ломает.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub fn render_nexus_toml(config: &InstanceConfig) -> String {
    format!(
        "# NEXUS WORKSPACE — конфигурация портативного запуска.\n\
         # Секреты (DATABASE_URL, NEXUS_SECRET) передаются лаунчером через окружение\n\
         # и хранятся в config/secrets.json с ограниченным ACL.\n\
         # Пути относительны корня Standalone: папку можно переносить.\n\
         \n\
         [server]\n\
         host = \"127.0.0.1\"\n\
         port = {port}\n\
         public_url = {public_url}\n\
         static_dir = \"frontend\"\n\
         trust_proxy = false\n\
         \n\
         [database]\n\
         max_connections = 20\n\
         \n\
         [security]\n\
         session_ttl_hours = 24\n\
         password_min_length = 12\n\
         cookie_secure = false\n\
         secret = \"\"\n\
         allowed_origins = []\n\
         \n\
         [storage]\n\
         max_upload_bytes = 10737418240\n\
         chunk_max_bytes = 67108864\n\
         upload_ttl_hours = 24\n\
         share_default_hours = 168\n\
         export_ttl_hours = 6\n\
         \n\
         [launcher]\n\
         requests_per_minute = 120\n\
         artifacts_per_minute = 600\n\
         max_parallel_downloads = 8\n\
         \n\
         [logging]\n\
         dir = \"logs\"\n\
         file = \"backend.log\"\n\
         keep_files = 14\n",
        port = config.app_port,
        public_url = toml_string(&config.public_url)
    )
}

/// Добавляет к файлу строку/блок с ограничением размера и ротацией.
pub fn append_log(path: &Path, line: &str) {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() > 8 * 1024 * 1024 {
            let rotated = path.with_extension(format!(
                "log.{}",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ));
            let _ = fs::rename(path, rotated);
        }
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{stamp}] {line}");
    }
}

/// Хвост файла журнала для сообщения об ошибке (без секретов: журналы
/// backend не печатают DATABASE_URL и NEXUS_SECRET).
pub fn tail(path: &Path, lines: usize) -> String {
    let Ok(raw) = fs::read_to_string(path) else {
        return String::new();
    };
    let collected: Vec<&str> = raw.lines().rev().take(lines).collect();
    collected.into_iter().rev().collect::<Vec<_>>().join("\n")
}

pub trait CommandNoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl CommandNoWindow for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000);
        }
        self
    }
}
