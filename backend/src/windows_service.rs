//! Диспетчер службы Windows (SCM).
//!
//! Без этого модуля `nexus.exe` — обычная консольная программа: SCM запускает
//! её, не получает `StartServiceCtrlDispatcherW`, и `Start-Service` завершается
//! ошибкой 1053. Здесь реализован настоящий service wrapper в том же
//! бинарнике, поэтому отдельные NSSM/WinSW не нужны.
//!
//! `Stop-Service` и выключение системы приходят как `SERVICE_CONTROL_STOP` /
//! `SERVICE_CONTROL_SHUTDOWN` (не Ctrl+C). Контроллер переводит их в тот же
//! graceful shutdown, что и Ctrl+C в консоли: сервер перестаёт принимать
//! соединения, планировщик возвращает незавершённые задачи в очередь (C2),
//! пул БД закрывается. Если SCM всё же убьёт процесс по таймауту, следующий
//! старт вернёт `running` в `pending` (см. `system::scheduler`).

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::watch;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use nexus::config::Config;

use crate::{init_tracing, serve};

/// Имя по умолчанию. Установщик записывает фактическое имя в переменную
/// среды службы `NEXUS_SERVICE_NAME`, потому что SCM сопоставляет диспетчер
/// с зарегистрированным именем.
pub const DEFAULT_SERVICE_NAME: &str = "NexusWorkspace";

/// `WaitToKillServiceTimeout` на большинстве Windows 10/11 — 5000 мс.
/// Установщик передаёт фактическое значение в `NEXUS_SERVICE_STOP_TIMEOUT_MS`.
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Верхняя граница внутреннего ожидания задач (консольный режим — 30 с).
const MAX_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
/// Оставляем SCM запас на закрытие пула БД и завершение процесса.
const STOP_RESERVE: Duration = Duration::from_secs(1);

const SERVICE_TYPE_VALUE: ServiceType = ServiceType::OWN_PROCESS;

define_windows_service!(ffi_service_main, service_main);

/// Запущен ли процесс как служба. Установщик добавляет `--service` в binPath.
pub fn requested() -> bool {
    has_service_flag(std::env::args().skip(1))
}

fn has_service_flag(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|arg| arg == "--service")
}

/// Блокирует текущий поток в диспетчере SCM до остановки службы.
pub fn run() -> Result<()> {
    let name = service_name();
    service_dispatcher::start(&name, ffi_service_main)
        .with_context(|| format!("не удалось запустить диспетчер службы {name}"))
}

fn service_name() -> String {
    std::env::var("NEXUS_SERVICE_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned())
}

fn service_main(_arguments: Vec<OsString>) {
    // service_main вызывается SCM на отдельном потоке; паника здесь оставила
    // бы службу в состоянии StartPending до таймаута, поэтому ошибку только
    // журналируем и корректно сообщаем Stopped.
    if let Err(error) = run_service() {
        eprintln!("служба NEXUS WORKSPACE завершилась с ошибкой: {error:#}");
        write_service_error(&error);
    }
}

/// Аварийный журнал рядом с `.exe`: если конфигурация не загрузилась,
/// обычный tracing ещё не настроен, а stderr службы никуда не подключён.
fn write_service_error(error: &anyhow::Error) {
    use std::io::Write;

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let logs = dir.join("logs");
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("service-error.log"))
    else {
        return;
    };
    let _ = writeln!(
        file,
        "{} служебная ошибка: {error:#}",
        chrono::Local::now().to_rfc3339()
    );
}

/// Таймаут, после которого SCM завершает процесс принудительно.
fn scm_stop_timeout() -> Duration {
    std::env::var("NEXUS_SERVICE_STOP_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_STOP_TIMEOUT)
}

/// Внутренний graceful shutdown: не дольше таймаута SCM минус запас.
fn shutdown_grace(stop_timeout: Duration) -> Duration {
    stop_timeout
        .saturating_sub(STOP_RESERVE)
        .clamp(Duration::from_secs(1), MAX_SHUTDOWN_GRACE)
}

fn run_service() -> Result<()> {
    let name = service_name();
    let stop_timeout = scm_stop_timeout();
    let grace = shutdown_grace(stop_timeout);
    // SCM принимает wait hint в миллисекундах u32; ограничиваем разумным
    // максимумом, чтобы повреждённое значение реестра не вызвало панику.
    let stop_wait_hint = stop_timeout.min(Duration::from_secs(60));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let event_handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                // Повторные команды не мешают: watch::Sender::send перезаписывает
                // значение, а сервер завершается идемпотентно.
                let _ = shutdown_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(&name, event_handler)
        .with_context(|| format!("не удалось зарегистрировать обработчик службы {name}"))?;

    // SCM ждёт Running не дольше ~30 с. Миграции и подключение к PostgreSQL
    // могут занять больше, поэтому сообщаем Running сразу: если старт затем
    // провалится, служба завершится с ошибкой и сработает политика recovery.
    status_handle.set_service_status(service_status(
        ServiceState::Running,
        ServiceExitCode::NO_ERROR,
        0,
        Duration::ZERO,
    ))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("не удалось создать асинхронный рантайм службы")?;

    // Отдельная задача сообщает SCM о переходе в StopPending, чтобы
    // `Stop-Service` не считал службу зависшей во время graceful shutdown.
    let stop_status = status_handle;
    let mut stop_watch = shutdown_rx.clone();
    let stop_task = runtime.spawn(async move {
        while !*stop_watch.borrow() {
            if stop_watch.changed().await.is_err() {
                return;
            }
        }
        let _ = stop_status.set_service_status(service_status(
            ServiceState::StopPending,
            ServiceExitCode::NO_ERROR,
            0,
            stop_wait_hint,
        ));
    });

    let result = runtime.block_on(async move {
        let config = Config::load()?;
        // Держим guard до конца работы: он сбрасывает буфер журнала на диск.
        let _log_guard = init_tracing(&config);
        tracing::info!(
            scm_stop_timeout_ms = stop_timeout.as_millis() as u64,
            shutdown_grace_ms = grace.as_millis() as u64,
            "остановка службы: SERVICE_CONTROL_STOP → graceful shutdown"
        );
        serve(config, Some(shutdown_rx), grace).await
    });

    // Не даём задаче статуса выставить StopPending уже после Stopped.
    stop_task.abort();

    let exit_code = match &result {
        Ok(()) => ServiceExitCode::NO_ERROR,
        Err(error) => {
            eprintln!("служба NEXUS WORKSPACE: {error:#}");
            ServiceExitCode::ServiceSpecific(1)
        }
    };
    status_handle.set_service_status(service_status(
        ServiceState::Stopped,
        exit_code,
        0,
        Duration::ZERO,
    ))?;

    result
}

fn service_status(
    state: ServiceState,
    exit_code: ServiceExitCode,
    checkpoint: u32,
    wait_hint: Duration,
) -> ServiceStatus {
    let controls = if state == ServiceState::Running {
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
    } else {
        ServiceControlAccept::empty()
    };
    ServiceStatus {
        service_type: SERVICE_TYPE_VALUE,
        current_state: state,
        controls_accepted: controls,
        exit_code,
        checkpoint,
        wait_hint,
        process_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_flag_matches_installer_bin_path() {
        assert!(has_service_flag([
            "serve".to_owned(),
            "--service".to_owned()
        ]));
        assert!(!has_service_flag(["serve".to_owned()]));
    }

    #[test]
    fn running_service_accepts_stop_and_shutdown() {
        let status = service_status(
            ServiceState::Running,
            ServiceExitCode::NO_ERROR,
            0,
            Duration::ZERO,
        );
        assert!(
            status
                .controls_accepted
                .contains(ServiceControlAccept::STOP)
        );
        assert!(
            status
                .controls_accepted
                .contains(ServiceControlAccept::SHUTDOWN)
        );
        assert_eq!(status.current_state, ServiceState::Running);
    }

    #[test]
    fn stop_pending_reports_wait_hint() {
        let hint = Duration::from_secs(5);
        let status = service_status(
            ServiceState::StopPending,
            ServiceExitCode::NO_ERROR,
            0,
            hint,
        );
        assert_eq!(status.wait_hint, hint);
        assert!(status.controls_accepted.is_empty());
    }

    #[test]
    fn graceful_shutdown_stays_below_scm_timeout() {
        assert_eq!(
            shutdown_grace(Duration::from_secs(5)),
            Duration::from_secs(4)
        );
        assert_eq!(shutdown_grace(Duration::from_secs(60)), MAX_SHUTDOWN_GRACE);
        assert_eq!(
            shutdown_grace(Duration::from_millis(200)),
            Duration::from_secs(1)
        );
    }
}
