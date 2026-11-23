use std::io;

use nexus_core::config::LoggingConfig;
use nexus_core::{DataPaths, NexusError};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Keeps the non-blocking file writer alive for the process lifetime.
pub struct LoggingGuard {
    _guard: Option<WorkerGuard>,
}

impl LoggingGuard {
    pub fn disabled() -> Self {
        Self { _guard: None }
    }
}

/// Initialises file logging. A logging failure must never prevent the launcher
/// from starting, so an already-initialised subscriber is treated as success.
pub fn init(paths: &DataPaths, config: &LoggingConfig) -> Result<LoggingGuard, NexusError> {
    if !config.enabled {
        return Ok(LoggingGuard::disabled());
    }

    let filter =
        EnvFilter::try_new(config.level.as_str()).unwrap_or_else(|_| EnvFilter::new("info"));

    let file_appender = tracing_appender::rolling::daily(paths.logs_dir(), "launcher.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    let result = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(writer)
        .try_init();

    match result {
        Ok(()) => Ok(LoggingGuard {
            _guard: Some(guard),
        }),
        Err(error) => {
            // Keep the WorkerGuard alive even though the subscriber already
            // existed (for example in tests or when the app is embedded).
            tracing::debug!(%error, "tracing subscriber was already initialised");
            Ok(LoggingGuard {
                _guard: Some(guard),
            })
        }
    }
}

/// Convenience for tests and tools that only need a stderr subscriber.
pub fn init_stderr() {
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
}
