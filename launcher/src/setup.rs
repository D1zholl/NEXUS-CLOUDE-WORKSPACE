use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::paths::{
    AppPaths, CommandNoWindow, InstanceConfig, Secrets, append_log, render_nexus_toml, tail,
    write_private, write_text,
};
use crate::proc::{
    find_free_port, health_state, http_get, is_port_free, process_alive, process_image_path,
    run_capture, terminate, wait_health, wait_process_exit, wait_tcp,
};
use crate::state::{Ctx, Phase, read_state};

const PG_READY_TIMEOUT: Duration = Duration::from_secs(60);
const BACKEND_READY_TIMEOUT: Duration = Duration::from_secs(180);

pub struct Prepared {
    pub instance: InstanceConfig,
    pub secrets: Secrets,
}

pub fn verify_release(paths: &AppPaths) -> Result<()> {
    let required: [(&str, PathBuf); 9] = [
        ("backend/nexus.exe", paths.backend_exe.clone()),
        ("frontend/index.html", paths.frontend.join("index.html")),
        (
            "database/bin/postgres.exe",
            paths.database_bin.join("postgres.exe"),
        ),
        (
            "database/bin/initdb.exe",
            paths.database_bin.join("initdb.exe"),
        ),
        (
            "database/bin/pg_ctl.exe",
            paths.database_bin.join("pg_ctl.exe"),
        ),
        (
            "database/bin/psql.exe",
            paths.database_bin.join("psql.exe"),
        ),
        (
            "database/bin/pg_isready.exe",
            paths.database_bin.join("pg_isready.exe"),
        ),
        (
            "database/share/postgresql.conf.sample",
            paths
                .root
                .join("database")
                .join("share")
                .join("postgresql.conf.sample"),
        ),
        (
            "database/bin/pg_dump.exe",
            paths.database_bin.join("pg_dump.exe"),
        ),
    ];
    let mut missing = Vec::new();
    for (label, path) in required {
        if !path.exists() {
            missing.push(format!("{label} ({})", path.display()));
        }
    }
    if !missing.is_empty() {
        bail!(
            "Файлы релиза повреждены или отсутствуют:\n  {}\n\
             Переустановите Standalone из исходного архива; не удаляйте файлы приложения.",
            missing.join("\n  ")
        );
    }
    Ok(())
}

pub fn load_config(ctx: &Ctx) -> Result<Prepared> {
    let paths = &ctx.paths;
    let (mut instance, warning) = InstanceConfig::load_or_create(paths);
    let secrets = Secrets::load_or_create(paths)?;
    if let Some(warning) = warning {
        append_log(&paths.launcher_log, &warning);
        ctx.update(|state| state.warning = warning);
    }

    // Порт приложения: не захватываем чужой процесс, а предлагаем свободный
    // локальный порт и сохраняем выбор. Если backend этого же экземпляра уже
    // работает (CLI migrate/backup при запущенном NEXUS), порт не меняем.
    let own_backend_running = {
        let state_pid = read_state(paths).map(|s| s.backend_pid).unwrap_or(0);
        let pid = if backend_process_alive(paths, state_pid) {
            state_pid
        } else {
            live_backend_pid(paths)
        };
        pid != 0 && health_state(instance.app_port).0 == 200
    };
    if !is_port_free(instance.app_port) && !own_backend_running {
        match find_free_port(instance.app_port.saturating_add(1)) {
            Some(port) => {
                let warning = format!(
                    "Порт {} занят другим процессом. NEXUS будет запущен на свободном порту {}.",
                    instance.app_port, port
                );
                append_log(&paths.launcher_log, &warning);
                ctx.update(|state| state.warning = warning);
                instance.app_port = port;
                instance.public_url = format!("http://127.0.0.1:{port}");
            }
            None => bail!(
                "Порт {} занят, свободный локальный порт не найден. \
                 Закройте программу, занимающую порт, или измените app_port в config/instance.json.",
                instance.app_port
            ),
        }
    }
    if !is_port_free(instance.db_port) {
        if !database_process_running(paths) {
            match find_free_port(instance.db_port.saturating_add(1)) {
                Some(port) => {
                    let warning = format!(
                        "Порт PostgreSQL {} занят. Встроенная база будет запущена на порту {}.",
                        instance.db_port, port
                    );
                    append_log(&paths.launcher_log, &warning);
                    ctx.update(|state| state.warning = warning);
                    instance.db_port = port;
                }
                None => bail!("Не найден свободный локальный порт для PostgreSQL."),
            }
        }
    }
    if instance.public_url.trim().is_empty() {
        instance.public_url = format!("http://127.0.0.1:{}", instance.app_port);
    }
    instance.version = crate::paths::VERSION.into();

    instance.save(paths)?;
    sync_nexus_toml(paths, &instance)?;
    ctx.update(|state| {
        state.app_port = instance.app_port;
        state.db_port = instance.db_port;
    });
    Ok(Prepared { instance, secrets })
}

/// Сохраняет пользовательские правки nexus.toml, меняя только строки
/// `port`/`public_url` в секции `[server]`. Полная перезапись — только если
/// файла нет.
fn sync_nexus_toml(paths: &AppPaths, instance: &InstanceConfig) -> Result<()> {
    if !paths.nexus_toml.exists() {
        return write_text(&paths.nexus_toml, &render_nexus_toml(instance));
    }
    let raw = fs::read_to_string(&paths.nexus_toml)
        .with_context(|| format!("не удалось прочитать {}", paths.nexus_toml.display()))?;
    let mut in_server = false;
    let mut saw_port = false;
    let mut saw_url = false;
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_server = trimmed == "[server]";
        }
        if in_server && trimmed.starts_with("port") && trimmed.contains('=') {
            lines.push(format!("port = {}", instance.app_port));
            saw_port = true;
            continue;
        }
        if in_server && trimmed.starts_with("public_url") && trimmed.contains('=') {
            lines.push(format!("public_url = \"{}\"", instance.public_url));
            saw_url = true;
            continue;
        }
        lines.push(line.to_owned());
    }
    if !saw_port || !saw_url {
        // Секция повреждена или отсутствует — добавляем управляемые значения.
        lines.push(String::new());
        lines.push("[server]".into());
        if !saw_port {
            lines.push(format!("port = {}", instance.app_port));
            lines.push("host = \"127.0.0.1\"".into());
        }
        if !saw_url {
            lines.push(format!("public_url = \"{}\"", instance.public_url));
        }
        if !raw.contains("static_dir") {
            lines.push("static_dir = \"frontend\"".into());
        }
    }
    write_text(&paths.nexus_toml, &(lines.join("\r\n") + "\r\n"))
}

pub fn backend_env(paths: &AppPaths, instance: &InstanceConfig, secrets: &Secrets) -> Vec<(String, String)> {
    vec![
        ("DATABASE_URL".into(), instance.database_url(&secrets.database_password)),
        ("NEXUS_SECRET".into(), secrets.nexus_secret.clone()),
        (
            "NEXUS_CONFIG".into(),
            paths.nexus_toml.to_string_lossy().to_string(),
        ),
        ("NEXUS_HOST".into(), "127.0.0.1".into()),
        ("NEXUS_PORT".into(), instance.app_port.to_string()),
        ("NEXUS_PUBLIC_URL".into(), instance.public_url.clone()),
        (
            "NEXUS_STATIC_DIR".into(),
            paths.frontend.to_string_lossy().to_string(),
        ),
        (
            "NEXUS_LOG_DIR".into(),
            paths.logs.to_string_lossy().to_string(),
        ),
        ("NEXUS_LOG_FILE".into(), "backend.log".into()),
        ("NEXUS_LOG_KEEP".into(), "14".into()),
        ("NEXUS_COOKIE_SECURE".into(), "false".into()),
        (
            "NEXUS_SHUTDOWN_FILE".into(),
            paths.shutdown_file().to_string_lossy().to_string(),
        ),
        (
            "RUST_LOG".into(),
            "nexus=info,tower_http=info,sqlx=warn".into(),
        ),
    ]
}

fn postgres_env() -> Vec<(String, String)> {
    vec![
        ("PGCLIENTENCODING".into(), "UTF8".into()),
        ("PGCONNECT_TIMEOUT".into(), "5".into()),
    ]
}

fn psql_env(secrets: &Secrets) -> Vec<(String, String)> {
    let mut env = postgres_env();
    env.push(("PGPASSWORD".into(), secrets.database_password.clone()));
    env
}

pub fn database_process_running(paths: &AppPaths) -> bool {
    let marker = paths.data_postgres.join("postmaster.pid");
    if !marker.exists() {
        return false;
    }
    // pg_ctl status — штатная проверка; код 3 означает «не запущен».
    Command::new(paths.database_bin.join("pg_ctl.exe"))
        .arg("-D")
        .arg(&paths.data_postgres)
        .arg("status")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_no_window()
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn prepare_postgres(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<()> {
    let paths = &ctx.paths;
    let data = &paths.data_postgres;
    let version_file = data.join("PG_VERSION");
    ctx.set_phase(Phase::PreparingDatabase, "Проверка встроенной PostgreSQL");

    if !version_file.exists() {
        if data.exists() && fs::read_dir(data).map(|mut d| d.next().is_some()).unwrap_or(false) {
            bail!(
                "Каталог базы данных {} не содержит PG_VERSION. \
                 Он повреждён или создан другой версией. Не удаляйте его автоматически: \
                 восстановите data из резервной копии или переименуйте каталог вручную.",
                data.display()
            );
        }
        fs::create_dir_all(data)?;
        let pwfile = paths.cache.join("initdb-password.txt");
        write_private(&pwfile, secrets.database_password.as_bytes())?;
        append_log(&paths.launcher_log, "initdb: инициализация кластера PostgreSQL");
        let args = vec![
            "-D".to_string(),
            data.to_string_lossy().to_string(),
            "-U".to_string(),
            "nexus".to_string(),
            "--pwfile".to_string(),
            pwfile.to_string_lossy().to_string(),
            "--auth".to_string(),
            "scram-sha-256".to_string(),
            "--encoding".to_string(),
            "UTF8".to_string(),
            "--locale".to_string(),
            "C".to_string(),
            "--no-instructions".to_string(),
        ];
        let result = run_capture(
            &paths.database_bin.join("initdb.exe"),
            &args,
            &paths.root,
            &postgres_env(),
        );
        let _ = fs::remove_file(&pwfile);
        let output = result?;
        if !output.status.success() {
            bail!(
                "initdb завершился с ошибкой: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        append_log(&paths.launcher_log, "initdb: кластер создан");
    }

    let version = fs::read_to_string(&version_file).unwrap_or_default();
    if !version.trim().starts_with("17") {
        ctx.update(|state| {
            state.warning = format!(
                "Каталог данных PostgreSQL имеет версию {}; в релиз включена 17.x. \
                 Запуск продолжится, но проверьте совместимость.",
                version.trim()
            );
        });
    }

    ensure_postgres_conf(paths, instance)?;
    ensure_pg_hba(paths)?;
    Ok(())
}

fn ensure_postgres_conf(paths: &AppPaths, instance: &InstanceConfig) -> Result<()> {
    let conf = paths.data_postgres.join("postgresql.conf");
    let raw = fs::read_to_string(&conf)
        .with_context(|| format!("не удалось прочитать {}", conf.display()))?;
    let mut kept: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("port")
            && trimmed.contains('=')
            && !trimmed.starts_with('#')
        {
            continue;
        }
        if trimmed.starts_with("listen_addresses")
            && trimmed.contains('=')
            && !trimmed.starts_with('#')
        {
            continue;
        }
        kept.push(line.to_owned());
    }
    kept.push(String::new());
    kept.push("# --- NEXUS portable managed block ---".into());
    kept.push("listen_addresses = '127.0.0.1'".into());
    kept.push(format!("port = {}", instance.db_port));
    kept.push("max_connections = 50".into());
    kept.push("shared_buffers = 128MB".into());
    kept.push("password_encryption = scram-sha-256".into());
    kept.push("ssl = off".into());
    kept.push("logging_collector = off".into());
    kept.push("log_min_messages = warning".into());
    kept.push("log_line_prefix = '%m [%p] '".into());
    let mut file = fs::File::create(&conf)?;
    file.write_all((kept.join("\n") + "\n").as_bytes())?;
    Ok(())
}

fn ensure_pg_hba(paths: &AppPaths) -> Result<()> {
    let hba = paths.data_postgres.join("pg_hba.conf");
    let content = "\
# NEXUS portable PostgreSQL: только локальные подключения по SCRAM.
# Не добавляйте строки host all all 0.0.0.0/0 без явной необходимости.
local   all             all                                     scram-sha-256
host    all             all             127.0.0.1/32            scram-sha-256
host    all             all             ::1/128                 scram-sha-256
";
    write_text(&hba, content)
}

/// pg_ctl запускает postgres как отдельный долгоживущий процесс. Если захватить
/// его stdout/stderr через pipe, `Command::output()` будет ждать EOF, пока
/// работает сервер, и запуск зависнет. Поэтому вывод pg_ctl направляется в
/// файл, а ожидание ограничивается кодом возврата самой утилиты.
fn run_pg_ctl(paths: &AppPaths, args: &[String]) -> Result<std::process::ExitStatus> {
    fs::create_dir_all(&paths.logs)?;
    // Не открываем logs\database.log: этот файл передаётся pg_ctl через -l, и
    // второй открытый дескриптор мешает PostgreSQL открыть журнал самому.
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.pg_ctl_log())?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(paths.database_bin.join("pg_ctl.exe"));
    command
        .args(args)
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    for (key, value) in postgres_env() {
        command.env(key, value);
    }
    command.creation_flags_no_window();
    command.status().context("не удалось запустить pg_ctl.exe")
}

pub fn start_postgres(ctx: &Ctx, instance: &InstanceConfig) -> Result<()> {
    let paths = &ctx.paths;
    if database_process_running(paths) {
        if wait_tcp(instance.db_port, Duration::from_secs(15)) {
            ctx.update(|state| {
                state.database_running = true;
                state.phase = Phase::DatabaseReady;
                state.detail = format!("PostgreSQL уже запущен на порту {}", instance.db_port);
            });
            return Ok(());
        }
        bail!(
            "PostgreSQL числится запущенным, но порт {} не отвечает. \
             Проверьте logs/database.log.",
            instance.db_port
        );
    }
    ctx.set_phase(
        Phase::StartingPostgres,
        format!("Запуск PostgreSQL на 127.0.0.1:{}", instance.db_port),
    );
    fs::create_dir_all(&paths.logs)?;
    let args = vec![
        "-D".to_string(),
        paths.data_postgres.to_string_lossy().to_string(),
        "-l".to_string(),
        paths.postgres_log().to_string_lossy().to_string(),
        "start".to_string(),
        "-w".to_string(),
        "-t".to_string(),
        "60".to_string(),
    ];
    let status = run_pg_ctl(paths, &args)?;
    if !status.success() {
        let log = format!(
            "--- pg_ctl ---\n{}\n--- PostgreSQL ---\n{}",
            tail(&paths.pg_ctl_log(), 20),
            tail(&paths.postgres_log(), 40)
        );
        bail!(
            "Не удалось запустить PostgreSQL (код {}).\n{}",
            status.code().unwrap_or(-1),
            log
        );
    }
    if !wait_tcp(instance.db_port, PG_READY_TIMEOUT) {
        bail!(
            "PostgreSQL не открыл порт {} за {} с. Журнал: {}",
            instance.db_port,
            PG_READY_TIMEOUT.as_secs(),
            paths.postgres_log().display()
        );
    }
    // pg_isready проверяет не только TCP, но и готовность принимать запросы.
    let mut ready = false;
    for _ in 0..60 {
        let output = run_capture(
            &paths.database_bin.join("pg_isready.exe"),
            &[
                "-h".into(),
                "127.0.0.1".into(),
                "-p".into(),
                instance.db_port.to_string(),
                "-t".into(),
                "2".into(),
            ],
            &paths.root,
            &postgres_env(),
        );
        if output.map(|o| o.status.success()).unwrap_or(false) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !ready {
        bail!(
            "PostgreSQL принял TCP-соединение, но не готов принимать запросы. \
             Журнал: {}",
            paths.postgres_log().display()
        );
    }
    ctx.update(|state| {
        state.database_running = true;
        state.phase = Phase::DatabaseReady;
        state.detail = format!("PostgreSQL готов на порту {}", instance.db_port);
        state.started_at = chrono::Local::now().to_rfc3339();
    });
    Ok(())
}

pub fn stop_postgres(ctx: &Ctx) -> Result<()> {
    let paths = &ctx.paths;
    if !database_process_running(paths) {
        ctx.update(|state| state.database_running = false);
        return Ok(());
    }
    for mode in ["fast", "immediate"] {
        let status = run_pg_ctl(
            paths,
            &[
                "-D".into(),
                paths.data_postgres.to_string_lossy().to_string(),
                "stop".into(),
                "-m".into(),
                mode.into(),
                "-w".into(),
                "-t".into(),
                "60".into(),
            ],
        )?;
        if status.success() {
            ctx.update(|state| state.database_running = false);
            append_log(&paths.launcher_log, &format!("PostgreSQL остановлен (режим {mode})"));
            return Ok(());
        }
        append_log(
            &paths.launcher_log,
            &format!(
                "pg_ctl stop -m {mode}: код {}; {}",
                status.code().unwrap_or(-1),
                tail(&paths.pg_ctl_log(), 10)
            ),
        );
    }
    if database_process_running(paths) {
        bail!(
            "PostgreSQL не завершился корректно. Проверьте logs/database.log и процессы postgres.exe."
        );
    }
    Ok(())
}

pub fn ensure_nexus_database(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<()> {
    let paths = &ctx.paths;
    let psql = paths.database_bin.join("psql.exe");
    let port = instance.db_port.to_string();
    let exists = run_capture(
        &psql,
        &[
            "-h".into(),
            "127.0.0.1".into(),
            "-p".into(),
            port.clone(),
            "-U".into(),
            "nexus".into(),
            "-d".into(),
            "postgres".into(),
            "-tAc".into(),
            "select 1 from pg_database where datname = 'nexus'".into(),
        ],
        &paths.root,
        &psql_env(secrets),
    )?;
    if !exists.status.success() {
        bail!(
            "Не удалось проверить базу nexus: {}",
            String::from_utf8_lossy(&exists.stderr).trim()
        );
    }
    if String::from_utf8_lossy(&exists.stdout).trim() == "1" {
        return Ok(());
    }
    append_log(&paths.launcher_log, "создание базы данных nexus");
    let output = run_capture(
        &paths.database_bin.join("createdb.exe"),
        &[
            "-h".into(),
            "127.0.0.1".into(),
            "-p".into(),
            port,
            "-U".into(),
            "nexus".into(),
            "nexus".into(),
        ],
        &paths.root,
        &psql_env(secrets),
    )?;
    if !output.status.success() {
        bail!(
            "Не удалось создать базу nexus: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn run_migrations(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<()> {
    ctx.set_phase(Phase::RunningMigrations, "Применение миграций NEXUS");
    let paths = &ctx.paths;
    let env = backend_env(paths, instance, secrets);
    let output = run_capture(
        &paths.backend_exe,
        &["migrate".into()],
        &paths.root,
        &env,
    )?;
    if !output.status.success() {
        bail!(
            "Миграции не применились (код {}): {}\n{}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim(),
            tail(&paths.backend_log(), 20)
        );
    }
    append_log(&paths.launcher_log, "миграции применены");
    Ok(())
}

pub fn has_admin(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<bool> {
    let paths = &ctx.paths;
    let env = backend_env(paths, instance, secrets);
    let output = run_capture(
        &paths.backend_exe,
        &["check".into(), "--json".into()],
        &paths.root,
        &env,
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let admin = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|value| value.get("admin_present").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    ctx.update(|state| state.admin_present = admin);
    Ok(admin)
}

pub fn ensure_default_volume(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<()> {
    let paths = &ctx.paths;
    let sql_path = paths.cache.join("bootstrap-volume.sql");
    let storage = storage_path_string(&paths.storage);
    let drive = drive_root(&paths.storage);
    let (free, total) = crate::proc::disk_space(&paths.storage).unwrap_or((0, 0));
    let sql = format!(
        "SET client_encoding = 'UTF8';\n\
         INSERT INTO storage_volumes\n\
           (name, root_path, disk_mount, mode, priority, reserved_bytes, max_bytes,\n\
            enabled, state, state_message, total_bytes, available_bytes,\n\
            last_checked_at, created_by, scope, owner_user_id)\n\
         SELECT 'NEXUS Storage', '{storage}', '{drive}', 'auto', 100, 0, NULL,\n\
                true, 'online', 'Portable local storage.', {total}, {free}, now(), NULL, 'system', NULL\n\
         WHERE NOT EXISTS (SELECT 1 FROM storage_volumes);\n\
         UPDATE storage_volumes\n\
            SET root_path = '{storage}', disk_mount = '{drive}',\n\
                total_bytes = {total}, available_bytes = {free},\n\
                state = 'online', state_message = 'Portable local storage.',\n\
                last_checked_at = now(), updated_at = now()\n\
          WHERE name = 'NEXUS Storage' AND scope = 'system';\n"
    );
    write_private(&sql_path, sql.as_bytes())?;
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
            "-v".into(),
            "ON_ERROR_STOP=1".into(),
            "-f".into(),
            sql_path.to_string_lossy().to_string(),
        ],
        &paths.root,
        &psql_env(secrets),
    )?;
    let _ = fs::remove_file(&sql_path);
    if !output.status.success() {
        // Том — удобство первого запуска, а не условие старта NEXUS.
        append_log(
            &paths.launcher_log,
            &format!(
                "не удалось зарегистрировать локальное хранилище: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        );
    }
    Ok(())
}

fn storage_path_string(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
    text.replace('/', "\\").replace('\'', "''")
}

fn drive_root(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
    if text.len() >= 2 && text.as_bytes()[1] == b':' {
        format!("{}\\", text[..2].to_ascii_uppercase())
    } else {
        String::new()
    }
}

pub fn create_admin_interactive(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<()> {
    ctx.set_phase(
        Phase::CreatingAdministrator,
        "В открывшемся окне задайте пароль администратора",
    );
    let paths = &ctx.paths;
    append_log(&paths.launcher_log, "открытие консольного мастера администратора");
    let env = backend_env(paths, instance, secrets);
    // `cmd /C start "" /WAIT` создаёт отдельное консольное окно. Это работает и
    // тогда, когда лаунчер уже отсоединён от консоли: rpassword читает пароль
    // из CONIN$ нового окна, без эха и без передачи в argv.
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let mut command = Command::new(shell);
    command
        .arg("/D")
        .arg("/C")
        .arg("start")
        .arg("")
        .arg("/WAIT")
        .arg(&paths.backend_exe)
        .arg("create-admin")
        .arg("--username")
        .arg("admin")
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in &env {
        command.env(key, value);
    }
    command.creation_flags_no_window();
    let status = command.status().context("не удалось открыть мастер администратора")?;
    if !status.success() && !has_admin(ctx, instance, secrets)? {
        bail!("Администратор не создан. Запустите NEXUS.exe и повторите первоначальную настройку.");
    }
    if !has_admin(ctx, instance, secrets)? {
        bail!("Администратор не создан.");
    }
    Ok(())
}

fn read_backend_pid(paths: &AppPaths) -> u32 {
    fs::read_to_string(paths.backend_pid_file())
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// PID считается нашим backend только если путь процесса совпадает с
/// backend\nexus.exe этого релиза. Это защищает от переиспользования PID.
pub fn backend_process_alive(paths: &AppPaths, pid: u32) -> bool {
    if pid == 0 || !process_alive(pid) {
        return false;
    }
    match process_image_path(pid) {
        Some(image) => image
            .to_string_lossy()
            .eq_ignore_ascii_case(&paths.backend_exe.to_string_lossy()),
        None => false,
    }
}

pub fn live_backend_pid(paths: &AppPaths) -> u32 {
    let pid = read_backend_pid(paths);
    if backend_process_alive(paths, pid) {
        pid
    } else {
        0
    }
}

pub fn start_backend(ctx: &Ctx, instance: &InstanceConfig, secrets: &Secrets) -> Result<u32> {
    let paths = &ctx.paths;
    ctx.set_phase(
        Phase::StartingBackend,
        format!("Запуск backend на 127.0.0.1:{}", instance.app_port),
    );
    let _ = fs::remove_file(paths.shutdown_file());
    fs::create_dir_all(&paths.logs)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.backend_log())?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(&paths.backend_exe);
    command
        .arg("serve")
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    for (key, value) in backend_env(paths, instance, secrets) {
        command.env(key, value);
    }
    command.creation_flags_no_window();
    let child = command.spawn().context("не удалось запустить backend")?;
    let pid = child.id();
    let _ = write_private(&paths.backend_pid_file(), pid.to_string().as_bytes());
    append_log(&paths.launcher_log, &format!("backend запущен, PID {pid}"));

    let alive = || process_alive(pid);
    match wait_health(instance.app_port, BACKEND_READY_TIMEOUT, alive) {
        Ok(_) => {
            ctx.update(|state| {
                state.backend_pid = pid;
                state.backend_running = true;
                state.phase = Phase::Ready;
                state.detail = format!("NEXUS доступен на {}", instance.public_url);
                state.started_at = chrono::Local::now().to_rfc3339();
                state.database_running = true;
            });
            let _ = http_get(instance.app_port, "/api/v1/health", Duration::from_secs(3));
            Ok(pid)
        }
        Err(error) => {
            let log_tail = tail(&paths.backend_log(), 60);
            terminate(pid, false);
            if !wait_process_exit(pid, Duration::from_secs(5)) {
                terminate(pid, true);
            }
            bail!(
                "Backend не стал готовым: {error}\n\nПоследние строки logs/backend.log:\n{log_tail}"
            )
        }
    }
}

pub fn stop_backend(ctx: &Ctx) -> Result<()> {
    let paths = &ctx.paths;
    let state = read_state(paths).unwrap_or_default();
    let mut pid = ctx.state().backend_pid;
    if pid == 0 {
        pid = state.backend_pid;
    }
    if !backend_process_alive(paths, pid) {
        pid = live_backend_pid(paths);
    }
    if !backend_process_alive(paths, pid) {
        let _ = fs::remove_file(paths.backend_pid_file());
        ctx.update(|s| {
            s.backend_pid = 0;
            s.backend_running = false;
        });
        return Ok(());
    }
    append_log(&paths.launcher_log, &format!("graceful stop backend PID {pid}"));
    let _ = fs::write(paths.shutdown_file(), b"stop");
    let mut exited = wait_process_exit(pid, Duration::from_secs(45));
    if !exited {
        append_log(&paths.launcher_log, "backend не завершился за 45 с, отправка taskkill");
        terminate(pid, false);
        exited = wait_process_exit(pid, Duration::from_secs(10));
    }
    if !exited {
        append_log(&paths.launcher_log, "backend не завершился, принудительное завершение");
        terminate(pid, true);
        exited = wait_process_exit(pid, Duration::from_secs(10));
    }
    let _ = fs::remove_file(paths.shutdown_file());
    let _ = fs::remove_file(paths.backend_pid_file());
    ctx.update(|s| {
        s.backend_pid = 0;
        s.backend_running = false;
    });
    if !exited {
        bail!("Backend не завершился. Проверьте процессы nexus.exe и logs/backend.log.");
    }
    Ok(())
}

/// Полный запуск. `interactive` разрешает открыть консольный мастер admin.
pub fn start_all(ctx: &Ctx, interactive: bool, open_browser: bool) -> Result<()> {
    ctx.set_phase(Phase::CheckingEnvironment, "Проверка файлов релиза");
    ctx.paths.ensure_directories()?;
    verify_release(&ctx.paths)?;
    let runtime_included = ctx.paths.runtime_exe.exists();
    let runtime_configured = ctx.paths.root.join("runtime").join("runtime.toml").exists()
        || ctx.paths.config.join("runtime.toml").exists();
    ctx.update(|state| {
        state.runtime_included = runtime_included;
        state.runtime_configured = runtime_configured;
        state.launcher_pid = std::process::id();
    });

    // После падения лаунчера backend мог остаться живым: не запускаем второй
    // экземпляр, а подхватываем существующий (advisory lock в БД это подтвердит).
    // PID хранится и в state.json, и в отдельном backend.pid — второй файл
    // переживает служебные CLI-команды и повреждение state.json.
    let existing = read_state(&ctx.paths).unwrap_or_default();
    let configured = fs::read_to_string(&ctx.paths.instance)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstanceConfig>(&raw).ok());
    let port = configured
        .as_ref()
        .map(|config| config.app_port)
        .unwrap_or(existing.app_port);
    let mut candidate = existing.backend_pid;
    if !backend_process_alive(&ctx.paths, candidate) {
        candidate = live_backend_pid(&ctx.paths);
    }
    if candidate != 0 && health_state(port).0 == 200 {
        let _ = write_private(&ctx.paths.backend_pid_file(), candidate.to_string().as_bytes());
        ctx.update(|state| {
            state.backend_pid = candidate;
            state.backend_running = true;
            state.phase = Phase::Ready;
            state.detail = format!("NEXUS уже запущен: http://127.0.0.1:{port}");
            state.app_port = port;
            state.db_port = configured
                .as_ref()
                .map(|config| config.db_port)
                .unwrap_or(existing.db_port);
            state.database_running = database_process_running(&ctx.paths);
            state.started_at = existing.started_at;
            state.admin_present = existing.admin_present;
        });
        if open_browser {
            crate::proc::open_external(&format!("http://127.0.0.1:{port}"));
        }
        return Ok(());
    }

    let prepared = load_config(ctx)?;
    prepare_postgres(ctx, &prepared.instance, &prepared.secrets)?;
    start_postgres(ctx, &prepared.instance)?;
    ensure_nexus_database(ctx, &prepared.instance, &prepared.secrets)?;
    run_migrations(ctx, &prepared.instance, &prepared.secrets)?;
    ensure_default_volume(ctx, &prepared.instance, &prepared.secrets)?;

    let admin = has_admin(ctx, &prepared.instance, &prepared.secrets)?;
    if !admin {
        if !interactive {
            bail!(
                "Администратор ещё не создан. Запустите NEXUS.exe с графическим интерфейсом \
                 (или First-Run.bat) и задайте пароль администратора."
            );
        }
        crate::ui::info_dialog(
            "Первый запуск NEXUS",
            "Сейчас откроется отдельное окно консоли для создания администратора.\n\n\
             Логин: admin\n\
             Введите пароль не короче 12 символов дважды. Пароль не отображается,\n\
             не передаётся в командной строке и не записывается в журналы.",
        );
        create_admin_interactive(ctx, &prepared.instance, &prepared.secrets)?;
    }

    start_backend(ctx, &prepared.instance, &prepared.secrets)?;
    if open_browser {
        let opened = crate::proc::open_external(&prepared.instance.public_url);
        if !opened {
            ctx.update(|state| {
                state.warning = format!(
                    "Браузер не открылся автоматически. Откройте вручную: {}",
                    prepared.instance.public_url
                );
            });
        }
    }
    ctx.update(|state| {
        state.app_port = prepared.instance.app_port;
        state.db_port = prepared.instance.db_port;
        state.admin_present = true;
        state.phase = Phase::Ready;
        state.detail = format!("NEXUS доступен на {}", prepared.instance.public_url);
    });
    Ok(())
}

pub fn stop_all(ctx: &Ctx) -> Result<()> {
    ctx.set_phase(Phase::Stopping, "Остановка backend");
    let backend_result = stop_backend(ctx);
    ctx.set_phase(Phase::Stopping, "Остановка PostgreSQL");
    let db_result = stop_postgres(ctx);
    let _ = fs::remove_file(ctx.paths.stop_request());
    let _ = fs::remove_file(ctx.paths.restart_request());
    match (backend_result, db_result) {
        (Ok(()), Ok(())) => {
            ctx.update(|state| {
                state.phase = Phase::Stopped;
                state.detail = "NEXUS остановлен".into();
                state.backend_running = false;
                state.database_running = false;
            });
            Ok(())
        }
        (backend, db) => {
            let mut parts = Vec::new();
            if let Err(error) = backend {
                parts.push(format!("backend: {error}"));
            }
            if let Err(error) = db {
                parts.push(format!("PostgreSQL: {error}"));
            }
            bail!(parts.join("; "))
        }
    }
}

pub fn provision_only(ctx: &Ctx) -> Result<Prepared> {
    ctx.set_phase(Phase::CheckingEnvironment, "Проверка файлов релиза");
    ctx.paths.ensure_directories()?;
    verify_release(&ctx.paths)?;
    let prepared = load_config(ctx)?;
    prepare_postgres(ctx, &prepared.instance, &prepared.secrets)?;
    start_postgres(ctx, &prepared.instance)?;
    ensure_nexus_database(ctx, &prepared.instance, &prepared.secrets)?;
    run_migrations(ctx, &prepared.instance, &prepared.secrets)?;
    ensure_default_volume(ctx, &prepared.instance, &prepared.secrets)?;
    ctx.set_phase(Phase::DatabaseReady, "База данных и миграции готовы");
    Ok(prepared)
}

pub fn run_backend_cli(ctx: &Ctx, args: &[String]) -> Result<i32> {
    if args.is_empty() {
        bail!("не указаны аргументы backend CLI");
    }
    ctx.paths.ensure_directories()?;
    verify_release(&ctx.paths)?;
    let prepared = load_config(ctx)?;
    prepare_postgres(ctx, &prepared.instance, &prepared.secrets)?;
    start_postgres(ctx, &prepared.instance)?;
    ensure_nexus_database(ctx, &prepared.instance, &prepared.secrets)?;
    let mut command = Command::new(&ctx.paths.backend_exe);
    command
        .args(args)
        .current_dir(&ctx.paths.root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in backend_env(&ctx.paths, &prepared.instance, &prepared.secrets) {
        command.env(key, value);
    }
    let status = command.status()?;
    Ok(status.code().unwrap_or(1))
}

pub fn database_backup(ctx: &Ctx) -> Result<PathBuf> {
    let paths = &ctx.paths;
    let prepared = load_config(ctx)?;
    if !database_process_running(paths) {
        bail!("PostgreSQL не запущен. Запустите NEXUS перед созданием резервной копии.");
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let target = paths.backups.join(&stamp);
    fs::create_dir_all(&target)?;
    ctx.set_phase(Phase::Stopping, "Создание резервной копии базы данных");
    let dump = target.join("database.dump");
    let output = run_capture(
        &paths.database_bin.join("pg_dump.exe"),
        &[
            "-h".into(),
            "127.0.0.1".into(),
            "-p".into(),
            prepared.instance.db_port.to_string(),
            "-U".into(),
            "nexus".into(),
            "-d".into(),
            "nexus".into(),
            "--format=custom".into(),
            "--compress=9".into(),
            "--no-owner".into(),
            "--no-privileges".into(),
            format!("--file={}", dump.display()),
        ],
        &paths.root,
        &psql_env(&prepared.secrets),
    )?;
    if !output.status.success() {
        bail!(
            "pg_dump завершился с ошибкой: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if fs::metadata(&dump)?.len() == 0 {
        bail!("pg_dump создал пустой файл");
    }

    // Пользовательские файлы и конфигурация копируются отдельно; секреты не
    // попадают в отчёт, но остаются в защищённом config/secrets.json.
    copy_tree(&paths.storage, &target.join("storage"))?;
    copy_tree(&paths.config, &target.join("config"))?;
    let manifest = serde_json::json!({
        "version": crate::paths::VERSION,
        "createdAt": chrono::Local::now().to_rfc3339(),
        "databaseDump": "database.dump",
        "databaseBytes": fs::metadata(&dump)?.len(),
        "appPort": prepared.instance.app_port,
        "dbPort": prepared.instance.db_port,
        "storageIncluded": paths.storage.exists(),
    });
    write_text(
        &target.join("manifest.json"),
        &serde_json::to_string_pretty(&manifest)?,
    )?;
    write_text(
        &target.join("README-RESTORE.txt"),
        "Восстановление NEXUS:\r\n\
         1. Остановите NEXUS (Stop.bat).\r\n\
         2. Запустите Restore.bat и укажите этот каталог.\r\n\
         3. Скрипт восстановит базу и storage, не перезаписывая config/secrets.json.\r\n\
         4. Запустите NEXUS.exe.\r\n",
    )?;
    append_log(paths.launcher_log.as_path(), &format!("backup создан: {}", target.display()));
    ctx.update(|state| {
        state.phase = Phase::Ready;
        state.detail = format!("Резервная копия: {}", target.display());
    });
    Ok(target)
}

pub fn database_restore(ctx: &Ctx, source: &Path) -> Result<()> {
    let paths = &ctx.paths;
    let dump = if source.is_file() {
        source.to_path_buf()
    } else {
        source.join("database.dump")
    };
    if !dump.exists() {
        bail!("Не найден файл database.dump в {}", source.display());
    }
    let prepared = load_config(ctx)?;
    prepare_postgres(ctx, &prepared.instance, &prepared.secrets)?;
    start_postgres(ctx, &prepared.instance)?;
    ensure_nexus_database(ctx, &prepared.instance, &prepared.secrets)?;
    ctx.set_phase(Phase::Stopping, "Восстановление базы данных");
    let output = run_capture(
        &paths.database_bin.join("pg_restore.exe"),
        &[
            "-h".into(),
            "127.0.0.1".into(),
            "-p".into(),
            prepared.instance.db_port.to_string(),
            "-U".into(),
            "nexus".into(),
            "-d".into(),
            "nexus".into(),
            "--clean".into(),
            "--if-exists".into(),
            "--no-owner".into(),
            "--no-privileges".into(),
            "--exit-on-error".into(),
            dump.to_string_lossy().to_string(),
        ],
        &paths.root,
        &psql_env(&prepared.secrets),
    )?;
    if !output.status.success() {
        bail!(
            "pg_restore завершился с ошибкой: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let storage_source = source.join("storage");
    if storage_source.exists() {
        copy_tree(&storage_source, &paths.storage)?;
    }
    append_log(
        paths.launcher_log.as_path(),
        &format!("backup восстановлен из {}", source.display()),
    );
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
