mod diagnostics;
mod paths;
mod proc;
mod setup;
mod state;
mod ui;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::paths::{AppPaths, InstanceConfig, append_log, write_private};
use crate::state::{Ctx, Phase, read_state};

const MUTEX_NAME: &str = "Local\\NEXUS-Workspace-Standalone-Launcher";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
    let code = match run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            #[cfg(windows)]
            if args.is_empty() || args.iter().any(|a| a == "--gui") {
                ui::error_dialog("NEXUS", &format!("{error:#}"));
            }
            1
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<i32> {
    let gui = args.is_empty() || args.iter().any(|a| a == "--gui" || a == "--first-run");
    if gui {
        hide_console();
        let paths = AppPaths::detect()?;
        paths.ensure_directories()?;
        if !acquire_single_instance() {
            ui::bring_to_front();
            return Ok(0);
        }
        append_log(
            &paths.launcher_log,
            &format!("запуск лаунчера {} из {}", paths::VERSION, paths.root.display()),
        );
        check_version_file(&paths)?;
        let ctx = Ctx::new(paths);
        return Ok(ui::run_gui(ctx));
    }

    match args[0].as_str() {
        "--help" | "-h" | "/?" => {
            print_help();
            Ok(0)
        }
        "--version" | "-V" => {
            println!("NEXUS WORKSPACE Standalone {} (launcher {})", paths::VERSION, paths::VERSION);
            Ok(0)
        }
        "--status" => status_command(args),
        "--stop" => stop_command(),
        "--restart" => restart_command(),
        "--open" => open_command(),
        "--diagnostics" => diagnostics_command(),
        "--provision" => provision_command(),
        "--start" => start_command(args),
        "--backup" => backup_command(),
        "--restore" => restore_command(args),
        "--backend" => backend_command(&args[1..]),
        "--version-file" => {
            println!("{}", paths::VERSION);
            Ok(0)
        }
        other => {
            print_help();
            bail!("неизвестный параметр: {other}");
        }
    }
}

fn print_help() {
    println!(
        "NEXUS WORKSPACE Standalone {}\n\
         \n\
         Использование:\n\
           NEXUS.exe                 графический лаунчер (обычный запуск)\n\
           NEXUS.exe --first-run     первоначальная настройка\n\
           NEXUS.exe --start         запуск без GUI (для сценариев)\n\
           NEXUS.exe --stop          корректная остановка\n\
           NEXUS.exe --restart       перезапуск\n\
           NEXUS.exe --status        статус компонентов\n\
           NEXUS.exe --diagnostics   диагностика (отчёт в logs)\n\
           NEXUS.exe --provision     подготовить БД и миграции без запуска\n\
           NEXUS.exe --backup        резервная копия БД и storage\n\
           NEXUS.exe --restore DIR   восстановить из резервной копии\n\
           NEXUS.exe --backend ARGS  выполнить CLI backend (user list, objects ...)\n\
         \n\
         Секреты не передаются через командную строку.",
        paths::VERSION
    );
}

fn hide_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleWindow};
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
        let console = GetConsoleWindow();
        if !console.is_null() {
            ShowWindow(console, SW_HIDE);
            FreeConsole();
        }
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    unsafe {
        let name = wide(MUTEX_NAME);
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return false;
        }
        // Дескриптор живёт до завершения процесса; закрывается ОС.
        true
    }
}

#[cfg(not(windows))]
fn acquire_single_instance() -> bool {
    true
}

#[cfg(windows)]
fn launcher_running() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    };
    unsafe {
        let name = wide(MUTEX_NAME);
        let handle = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
        if handle.is_null() {
            false
        } else {
            CloseHandle(handle);
            true
        }
    }
}

#[cfg(not(windows))]
fn launcher_running() -> bool {
    false
}

fn check_version_file(paths: &AppPaths) -> Result<()> {
    let version_file = paths.root.join("VERSION");
    if let Ok(text) = fs::read_to_string(&version_file) {
        let declared = text.trim();
        if !declared.is_empty() && declared != paths::VERSION {
            append_log(
                &paths.launcher_log,
                &format!(
                    "внимание: VERSION={declared}, лаунчер {}",
                    paths::VERSION
                ),
            );
        }
    }
    Ok(())
}

fn status_command(args: &[String]) -> Result<i32> {
    let paths = AppPaths::detect()?;
    let state = read_state(&paths).unwrap_or_default();
    let instance = fs::read_to_string(&paths.instance)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstanceConfig>(&raw).ok());
    let app_port = instance
        .as_ref()
        .map(|i| i.app_port)
        .unwrap_or(state.app_port);
    let db_port = instance
        .as_ref()
        .map(|i| i.db_port)
        .unwrap_or(state.db_port);
    let (http_status, health) = proc::health_state(app_port);
    let mut backend_pid = state.backend_pid;
    if !setup::backend_process_alive(&paths, backend_pid) {
        backend_pid = setup::live_backend_pid(&paths);
    }
    let backend_running = backend_pid != 0;
    let db_running = setup::database_process_running(&paths);
    let launcher = launcher_running();
    let json = args.iter().any(|a| a == "--json");
    if json {
        let payload = serde_json::json!({
            "version": paths::VERSION,
            "root": paths.root,
            "launcherRunning": launcher,
            "backendRunning": backend_running,
            "backendPid": backend_pid,
            "databaseRunning": db_running,
            "phase": state.phase,
            "appPort": app_port,
            "dbPort": db_port,
            "address": format!("http://127.0.0.1:{app_port}"),
            "httpStatus": http_status,
            "health": health,
            "adminPresent": state.admin_present,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("NEXUS WORKSPACE Standalone {}", paths::VERSION);
        println!("Каталог:            {}", paths.root.display());
        println!("Лаунчер:            {}", if launcher { "запущен" } else { "не запущен" });
        println!(
            "NEXUS:              {}",
            if state.phase == Phase::Ready && backend_running {
                "Running"
            } else {
                state.phase.label()
            }
        );
        println!(
            "Backend:            {}",
            if backend_running {
                format!("PID {} (HTTP {http_status}, {health})", backend_pid)
            } else {
                "Stopped".into()
            }
        );
        println!(
            "PostgreSQL:         {}",
            if db_running {
                format!("Running (127.0.0.1:{db_port})")
            } else {
                "Stopped".into()
            }
        );
        println!("Адрес:              http://127.0.0.1:{app_port}");
        if !state.error.is_empty() {
            println!("Последняя ошибка:   {}", state.error);
        }
    }
    Ok(if health == "ok" && backend_running { 0 } else { 1 })
}

fn wait_launcher_stop(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !launcher_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn stop_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    paths.ensure_directories()?;
    if launcher_running() {
        write_private(&paths.stop_request(), b"stop")?;
        println!("Запрошена корректная остановка NEXUS...");
        if wait_launcher_stop(Duration::from_secs(150)) {
            println!("NEXUS остановлен.");
            return Ok(0);
        }
        println!("Лаунчер не завершился за отведённое время; выполняю остановку напрямую.");
    }
    let ctx = Ctx::new(paths);
    match setup::stop_all(&ctx) {
        Ok(()) => {
            println!("NEXUS остановлен.");
            Ok(0)
        }
        Err(error) => {
            eprintln!("Остановка завершилась с ошибкой: {error:#}");
            Ok(1)
        }
    }
}

fn restart_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    paths.ensure_directories()?;
    if launcher_running() {
        write_private(&paths.restart_request(), b"restart")?;
        println!("Запрошен перезапуск NEXUS.");
        return Ok(0);
    }
    let ctx = Ctx::new(paths.clone());
    let _ = setup::stop_all(&ctx);
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(exe)
            .arg("--gui")
            .current_dir(&paths.root)
            .creation_flags(0x0000_0008)
            .spawn()?;
    }
    #[cfg(not(windows))]
    std::process::Command::new(exe).arg("--gui").spawn()?;
    println!("NEXUS запускается заново.");
    Ok(0)
}

fn open_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    paths.ensure_directories()?;
    let state = read_state(&paths).unwrap_or_default();
    let instance = fs::read_to_string(&paths.instance)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstanceConfig>(&raw).ok());
    let port = instance.map(|i| i.app_port).unwrap_or(state.app_port);
    if launcher_running() {
        write_private(&paths.open_request(), b"open")?;
        return Ok(0);
    }
    if !proc::open_external(&format!("http://127.0.0.1:{port}")) {
        println!("Откройте вручную: http://127.0.0.1:{port}");
        return Ok(1);
    }
    Ok(0)
}

fn diagnostics_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    paths.ensure_directories()?;
    let ctx = Ctx::new_cli(paths.clone());
    let report = diagnostics::run(&ctx)?;
    let target = paths.logs.join("diagnostics-latest.txt");
    write_private(&target, report.as_bytes())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let copy = paths.logs.join(format!("diagnostics-{stamp}.txt"));
    let _ = fs::copy(&target, &copy);
    println!("{report}");
    println!("Отчёт сохранён: {}", target.display());
    Ok(if report.contains("[FAIL]") { 1 } else { 0 })
}

fn provision_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    let ctx = Ctx::new_cli(paths);
    let prepared = setup::provision_only(&ctx)?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "appPort": prepared.instance.app_port,
            "dbPort": prepared.instance.db_port,
            "publicUrl": prepared.instance.public_url,
            "database": "portable PostgreSQL 17",
        })
    );
    Ok(0)
}

fn start_command(args: &[String]) -> Result<i32> {
    let paths = AppPaths::detect()?;
    let ctx = Ctx::new(paths.clone());
    let open_browser = !args.iter().any(|a| a == "--no-browser");
    setup::start_all(&ctx, false, open_browser)?;
    let state = ctx.state();
    println!(
        "NEXUS запущен: http://127.0.0.1:{} (backend PID {})",
        state.app_port, state.backend_pid
    );
    Ok(0)
}

fn backup_command() -> Result<i32> {
    let paths = AppPaths::detect()?;
    let ctx = Ctx::new(paths.clone());
    let state = read_state(&paths).unwrap_or_default();
    let was_running = setup::backend_process_alive(&paths, state.backend_pid)
        || setup::live_backend_pid(&paths) != 0;
    if was_running {
        println!("Остановка backend для согласованной копии...");
        let prepared = setup::load_config(&ctx)?;
        setup::stop_backend(&ctx)?;
        let result = setup::database_backup(&ctx);
        setup::start_backend(&ctx, &prepared.instance, &prepared.secrets)?;
        let target = result?;
        println!("Резервная копия создана: {}", target.display());
    } else {
        let prepared = setup::load_config(&ctx)?;
        setup::prepare_postgres(&ctx, &prepared.instance, &prepared.secrets)?;
        setup::start_postgres(&ctx, &prepared.instance)?;
        let target = setup::database_backup(&ctx)?;
        println!("Резервная копия создана: {}", target.display());
    }
    Ok(0)
}

fn restore_command(args: &[String]) -> Result<i32> {
    let paths = AppPaths::detect()?;
    let source = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("укажите каталог резервной копии: NEXUS.exe --restore backups\\TIMESTAMP"))?;
    let yes = args.iter().any(|a| a == "--yes");
    if !yes {
        println!("Восстановление заменит базу данных NEXUS.");
        print!("Введите YES для продолжения: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "YES" {
            println!("Отменено.");
            return Ok(1);
        }
    }
    let ctx = Ctx::new(paths.clone());
    if setup::live_backend_pid(&paths) != 0
        || read_state(&paths)
            .map(|state| setup::backend_process_alive(&paths, state.backend_pid))
            .unwrap_or(false)
    {
        setup::stop_backend(&ctx)?;
    }
    setup::database_restore(&ctx, &source)?;
    println!("Восстановление завершено из {}", source.display());
    Ok(0)
}

fn backend_command(args: &[String]) -> Result<i32> {
    if args.is_empty() {
        bail!("использование: NEXUS.exe --backend user list");
    }
    let paths = AppPaths::detect()?;
    let ctx = Ctx::new_cli(paths);
    setup::run_backend_cli(&ctx, args)
}
