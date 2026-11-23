//! Windows Service wrapper для NEXUS AI Runtime.

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

pub const DEFAULT_SERVICE_NAME: &str = "NexusAIRuntime";

pub fn requested() -> bool {
    std::env::args().skip(1).any(|arg| arg == "--service")
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use tokio::sync::watch;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    define_windows_service!(ffi_service_main, service_main);

    pub fn run() -> Result<()> {
        let name = service_name();
        service_dispatcher::start(&name, ffi_service_main)
            .with_context(|| format!("не удалось запустить диспетчер службы {name}"))
    }

    fn service_name() -> String {
        let mut args = std::env::args().skip(1);
        let mut name = None;
        while let Some(arg) = args.next() {
            if arg == "--service-name" {
                name = args.next();
                break;
            }
            if let Some(value) = arg.strip_prefix("--service-name=") {
                name = Some(value.to_owned());
                break;
            }
        }
        name.or_else(|| std::env::var("NEXUS_AI_RUNTIME_SERVICE_NAME").ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned())
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(error) = run_service() {
            eprintln!("служба NEXUS AI Runtime завершилась с ошибкой: {error:#}");
        }
    }

    fn run_service() -> Result<()> {
        let name = service_name();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let event_handler = move |control| -> ServiceControlHandlerResult {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                    let _ = shutdown_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };
        let status_handle = service_control_handler::register(&name, event_handler)
            .with_context(|| format!("не удалось зарегистрировать обработчик службы {name}"))?;
        status_handle.set_service_status(status(
            ServiceState::Running,
            ServiceExitCode::NO_ERROR,
            Duration::ZERO,
        ))?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("не удалось создать async runtime службы")?;
        let result = runtime.block_on(async move {
            let path = crate::resolve_config_path(config_arg().as_deref());
            let config = crate::config::RuntimeConfig::load(&path)?;
            let _guard = crate::init_logging(&config);
            let credentials = config.load_credentials()?;
            tracing::info!(
                node_id = %credentials.node_id,
                config = %path.display(),
                "служба NEXUS AI Runtime запущена"
            );
            crate::runtime::run(config, credentials, Some(shutdown_rx)).await
        });

        let exit_code = if result.is_ok() {
            ServiceExitCode::NO_ERROR
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };
        status_handle.set_service_status(status(
            ServiceState::Stopped,
            exit_code,
            Duration::ZERO,
        ))?;
        result
    }

    fn config_arg() -> Option<PathBuf> {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--config" {
                return args.next().map(PathBuf::from);
            }
            if let Some(value) = arg.strip_prefix("--config=") {
                return Some(PathBuf::from(value));
            }
        }
        None
    }

    fn status(
        state: ServiceState,
        exit_code: ServiceExitCode,
        wait_hint: Duration,
    ) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code,
            checkpoint: 0,
            wait_hint,
            process_id: None,
        }
    }
}

#[cfg(windows)]
pub use windows_impl::run;

#[cfg(not(windows))]
pub fn run() -> Result<()> {
    anyhow::bail!("Windows Service поддерживается только на Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_default_is_stable() {
        assert_eq!(DEFAULT_SERVICE_NAME, "NexusAIRuntime");
    }
}
