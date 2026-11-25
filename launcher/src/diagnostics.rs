use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::paths::{InstanceConfig, Secrets, tail};
use crate::proc::{health_state, http_get, is_port_free, run_capture};
use crate::setup::{backend_env, database_process_running, verify_release};
use crate::state::{Ctx, read_state};

#[derive(Default)]
struct Diag {
    text: String,
    problems: Vec<String>,
    warnings: Vec<String>,
}

impl Diag {
    fn push(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push('\n');
    }
    fn ok(&mut self, text: &str) {
        self.push(&format!("[OK]   {text}"));
    }
    fn info(&mut self, text: &str) {
        self.push(&format!("[INFO] {text}"));
    }
    fn warn(&mut self, text: &str) {
        self.warnings.push(text.to_owned());
        self.push(&format!("[WARN] {text}"));
    }
    fn fail(&mut self, text: &str) {
        self.problems.push(text.to_owned());
        self.push(&format!("[FAIL] {text}"));
    }
}

pub fn run(ctx: &Ctx) -> Result<String> {
    let paths = &ctx.paths;
    let mut d = Diag::default();

    d.push("NEXUS WORKSPACE — ДИАГНОСТИКА STANDALONE");
    d.push(&format!(
        "Версия: {}\nВремя: {}\nКаталог: {}\nПлатформа: Windows ({}), exe: {}\n",
        crate::paths::VERSION,
        chrono::Local::now().to_rfc3339(),
        paths.root.display(),
        std::env::consts::ARCH,
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    ));

    d.push("== Структура релиза ==");
    match verify_release(paths) {
        Ok(()) => d.ok("обязательные файлы backend/frontend/PostgreSQL присутствуют"),
        Err(error) => {
            let message = format!("{error:#}");
            d.fail(&message);
        }
    }
    if paths.runtime_exe.exists() {
        d.ok("AI Runtime включён");
    } else {
        d.warn("AI Runtime не включён (AI-модуль будет недоступен)");
    }
    if paths.root.join("runtime").join("runtime.toml").exists() {
        d.ok("runtime/runtime.toml найден");
    }

    d.push("\n== Конфигурация ==");
    let instance = fs::read_to_string(&paths.instance)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstanceConfig>(&raw).ok());
    let secrets = fs::read_to_string(&paths.secrets)
        .ok()
        .and_then(|raw| serde_json::from_str::<Secrets>(&raw).ok());
    match (&instance, &secrets) {
        (Some(instance), Some(_)) => {
            d.ok(&format!(
                "config/instance.json: порты {} / {}, URL {}",
                instance.app_port, instance.db_port, instance.public_url
            ));
            d.ok("config/secrets.json присутствует (содержимое не выводится)");
        }
        (None, _) => d.warn("config/instance.json отсутствует — будет создан при запуске"),
        (_, None) => d.fail("config/secrets.json отсутствует или повреждён"),
    }
    if paths.nexus_toml.exists() {
        d.ok("config/nexus.toml присутствует");
    } else {
        d.warn("config/nexus.toml отсутствует — будет создан при запуске");
    }

    d.push("\n== Порты ==");
    let state = read_state(paths).unwrap_or_default();
    if let Some(instance) = &instance {
        if state.backend_running {
            if is_port_free(instance.app_port) {
                d.fail(&format!(
                    "backend числится запущенным, но порт {} свободен",
                    instance.app_port
                ));
            } else {
                d.ok(&format!(
                    "порт приложения {} занят ожидаемым backend",
                    instance.app_port
                ));
            }
        } else if is_port_free(instance.app_port) {
            d.ok(&format!("порт приложения {} свободен", instance.app_port));
        } else {
            d.warn(&format!(
                "порт приложения {} занят: при запуске будет выбран другой локальный порт",
                instance.app_port
            ));
        }
        if state.database_running {
            if is_port_free(instance.db_port) {
                d.fail(&format!(
                    "PostgreSQL числится запущенным, но порт {} свободен",
                    instance.db_port
                ));
            } else {
                d.ok(&format!(
                    "порт PostgreSQL {} занят ожидаемым процессом",
                    instance.db_port
                ));
            }
        } else if is_port_free(instance.db_port) {
            d.ok(&format!("порт PostgreSQL {} свободен", instance.db_port));
        } else {
            d.warn(&format!("порт PostgreSQL {} занят", instance.db_port));
        }
    }

    d.push("\n== Права на каталоги ==");
    for dir in [&paths.logs, &paths.cache, &paths.backups, &paths.data] {
        match write_probe(dir) {
            Ok(()) => d.ok(&format!("запись разрешена: {}", dir.display())),
            Err(error) => d.fail(&format!("нет записи в {}: {error}", dir.display())),
        }
    }

    d.push("\n== PostgreSQL ==");
    let pg_version = fs::read_to_string(paths.data_postgres.join("PG_VERSION")).unwrap_or_default();
    if pg_version.trim().is_empty() {
        d.warn("кластер ещё не инициализирован (первый запуск)");
    } else {
        d.ok(&format!(
            "каталог данных PostgreSQL версии {}",
            pg_version.trim()
        ));
    }
    if database_process_running(paths) {
        d.ok("pg_ctl сообщает, что PostgreSQL запущен");
    } else {
        d.warn("PostgreSQL не запущен");
    }
    if let (Some(instance), Some(secrets)) = (&instance, &secrets) {
        let env = vec![
            ("PGPASSWORD".to_string(), secrets.database_password.clone()),
            ("PGCONNECT_TIMEOUT".into(), "5".into()),
        ];
        let output = run_capture(
            &paths.database_bin.join("psql.exe"),
            &[
                "-h".into(),
                "127.0.0.1".into(),
                "-p".into(),
                instance.db_port.to_string(),
                "-U".into(),
                "nexus".into(),
                "-d".into(),
                "nexus".into(),
                "-tAc".into(),
                "select version()".into(),
            ],
            &paths.root,
            &env,
        );
        match output {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                d.ok(&format!(
                    "SQL-подключение: {}",
                    version.lines().next().unwrap_or("")
                ));
            }
            Ok(output) => d.fail(&format!(
                "SQL-подключение не удалось: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => d.fail(&format!("psql не запустился: {error:#}")),
        }
        let env = backend_env(paths, instance, secrets);
        if let Ok(output) = run_capture(
            &paths.backend_exe,
            &["check".into(), "--json".into()],
            &paths.root,
            &env,
        ) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout
                .lines()
                .rev()
                .find(|line| line.trim_start().starts_with('{'))
            {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    let admin = value
                        .get("admin_present")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if admin {
                        d.ok("администратор создан");
                    } else {
                        d.warn("администратор не создан (первый запуск не завершён)");
                    }
                    if let Some(info) = value.get("info") {
                        if let Some(migrations) = info.get("migrations").and_then(|v| v.as_str()) {
                            d.ok(&format!("миграции: {migrations}"));
                        }
                    }
                }
            }
        }
    }

    d.push("\n== Backend и frontend ==");
    if let Some(instance) = &instance {
        let (status, health) = health_state(instance.app_port);
        if status == 200 && health == "ok" {
            d.ok(&format!("API health: HTTP {status}, status {health}"));
            let (index_status, body) =
                http_get(instance.app_port, "/", std::time::Duration::from_secs(5))
                    .unwrap_or((0, String::new()));
            if index_status == 200 && body.contains("NEXUS") {
                d.ok("frontend index.html отдаётся backend");
            } else {
                d.fail(&format!("frontend не отдаётся (HTTP {index_status})"));
            }
            match http_get(
                instance.app_port,
                "/api/v1/openapi.json",
                std::time::Duration::from_secs(5),
            ) {
                Ok((200, _)) => d.ok("OpenAPI доступен"),
                Ok((status, _)) => d.warn(&format!("OpenAPI вернул HTTP {status}")),
                Err(error) => d.fail(&format!("OpenAPI недоступен: {error:#}")),
            }
            match http_get(
                instance.app_port,
                "/api/v1/ws",
                std::time::Duration::from_secs(5),
            ) {
                Ok((status, _)) if status == 401 || status == 400 || status == 426 => {
                    d.ok("WebSocket endpoint присутствует (требует авторизацию)")
                }
                Ok((status, _)) => d.warn(&format!("WebSocket endpoint вернул HTTP {status}")),
                Err(error) => d.fail(&format!("WebSocket endpoint недоступен: {error:#}")),
            }
        } else {
            d.fail(&format!(
                "API health недоступен: HTTP {status}, status {health} (backend не запущен?)"
            ));
        }
    }
    if !fs::read_to_string(&paths.backend_log())
        .unwrap_or_default()
        .is_empty()
    {
        d.ok("logs/backend.log присутствует");
        let errors = tail(&paths.backend_log(), 200)
            .lines()
            .filter(|line| line.contains("ERROR"))
            .count();
        if errors > 0 {
            d.warn(&format!(
                "в последних строках backend.log найдено ERROR: {errors}"
            ));
        }
    }

    d.push("\n== Свободное место ==");
    if let Some((free, total)) = crate::proc::disk_space(&paths.root) {
        let free_gb = free as f64 / 1024.0 / 1024.0 / 1024.0;
        let total_gb = total as f64 / 1024.0 / 1024.0 / 1024.0;
        d.info(&format!("{free_gb:.1} ГБ свободно из {total_gb:.1} ГБ"));
        if free < 2 * 1024 * 1024 * 1024 {
            d.warn("свободного места меньше 2 ГБ");
        }
    }

    d.push("\n== Целостность файлов ==");
    let manifest = paths.root.join("manifest.json");
    if let Ok(raw) = fs::read_to_string(&manifest) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(files) = value.get("files").and_then(|v| v.as_object()) {
                let mut checked = 0;
                let mut bad = 0;
                for (relative, expected) in files {
                    let path = paths.root.join(relative.replace('/', "\\"));
                    let Some(expected) = expected.as_str() else {
                        continue;
                    };
                    match sha256_file(&path) {
                        Ok(actual) if actual == expected => checked += 1,
                        Ok(_) => {
                            bad += 1;
                            d.fail(&format!("hash не совпадает: {relative}"));
                        }
                        Err(_) => {
                            bad += 1;
                            d.fail(&format!("файл отсутствует: {relative}"));
                        }
                    }
                }
                if bad == 0 {
                    d.ok(&format!("проверено файлов по manifest.json: {checked}"));
                }
            }
        }
    } else {
        d.warn("manifest.json отсутствует — проверка хэшей пропущена");
    }
    for forbidden in ["node_modules", ".git", "target", "src"] {
        if paths.root.join(forbidden).exists() {
            d.fail(&format!("в релизе найден development-каталог: {forbidden}"));
        }
    }

    d.push("\n== Ограничения ==");
    d.push(
        "AI Runtime: отдельный процесс WSS; требует PolyStrata и enrollment через AI Management.\n\
         Внешний PostgreSQL/сеть: не используются; приложение слушает только 127.0.0.1.",
    );

    d.push("\n== Итог ==");
    if d.problems.is_empty() {
        d.push(&format!("Проверки пройдены. Предупреждений: {}.", d.warnings.len()));
    } else {
        d.push(&format!(
            "Найдено проблем: {}. Предупреждений: {}.",
            d.problems.len(),
            d.warnings.len()
        ));
    }
    Ok(d.text)
}

fn write_probe(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(".nexus-write-probe");
    fs::write(&probe, b"ok")?;
    fs::remove_file(probe)?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("не удалось прочитать {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}


