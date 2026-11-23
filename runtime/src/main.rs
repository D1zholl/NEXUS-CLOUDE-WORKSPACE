mod config;
mod filesystem;
mod patch;
mod process;
mod protocol;
mod provider;
mod runtime;
mod service;
mod tools;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use config::{DeviceCredentials, RuntimeConfig};
use serde_json::json;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "nexus-ai-runtime",
    version,
    about = "NEXUS AI Runtime для Windows"
)]
struct Cli {
    /// Путь к runtime.toml (или NEXUS_AI_RUNTIME_CONFIG).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Зарегистрировать устройство по одноразовому коду.
    Enroll {
        #[arg(long)]
        code: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Запустить Runtime.
    Run,
    /// Проверить конфигурацию, token, PolyStrata и программы.
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Сохранить новый device token после ротации в NEXUS.
    SetToken {
        #[arg(long)]
        node_id: uuid::Uuid,
        #[arg(long)]
        token: String,
    },
}

fn main() -> Result<()> {
    if service::requested() {
        return service::run();
    }

    let cli = Cli::parse();
    let path = resolve_config_path(cli.config.as_deref());
    let config = RuntimeConfig::load(&path).unwrap_or_else(|error| {
        eprintln!("не удалось загрузить конфигурацию: {error:#}");
        std::process::exit(2);
    });
    let _log_guard = init_logging(&config);

    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("не удалось создать async runtime")?;
    tokio_runtime.block_on(async move {
        match cli.command.unwrap_or(Command::Run) {
            Command::Enroll { code, name } => enroll(&config, &code, name.as_deref()).await,
            Command::Run => {
                let credentials = config.load_credentials()?;
                tracing::info!(
                    node_id = %credentials.node_id,
                    server = %config.server.url,
                    "NEXUS AI Runtime запускается"
                );
                runtime::run(config, credentials, None).await
            }
            Command::Check { json } => check(&config, json).await,
            Command::SetToken { node_id, token } => {
                let mut credentials = config.load_credentials().unwrap_or(DeviceCredentials {
                    node_id,
                    device_token: String::new(),
                    server_url: config.server.url.clone(),
                    enrolled_at: chrono::Utc::now(),
                    runtime_name: config.server.name.clone(),
                });
                credentials.node_id = node_id;
                credentials.device_token = token;
                credentials.server_url = config.server.url.clone();
                config.store_credentials(&credentials)?;
                println!("Device token сохранён для node {node_id}.");
                Ok(())
            }
        }
    })
}

pub(crate) fn resolve_config_path(explicit: Option<&std::path::Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Ok(path) = std::env::var("NEXUS_AI_RUNTIME_CONFIG") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("runtime.toml");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("runtime.toml")
}

pub(crate) fn init_logging(
    config: &RuntimeConfig,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "nexus_ai_runtime={},tungstenite=warn,reqwest=warn",
            config.logging.level
        ))
    });
    if config.storage.log_dir.trim().is_empty() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
        return None;
    }
    let dir = PathBuf::from(&config.storage.log_dir);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("не удалось создать каталог логов: {error}; вывод в stderr");
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
        return None;
    }
    let appender = match tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("nexus-ai-runtime")
        .max_log_files(config.logging.keep_files.max(1) as usize)
        .build(&dir)
    {
        Ok(appender) => appender,
        Err(error) => {
            eprintln!("не удалось открыть журнал: {error}; вывод в stderr");
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .compact()
                .init();
            return None;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .compact()
        .with_writer(writer)
        .init();
    Some(guard)
}

fn enroll_base(config: &RuntimeConfig) -> Result<String> {
    if !config.server.enroll_url.trim().is_empty() {
        return Ok(config.server.enroll_url.trim_end_matches('/').to_owned());
    }
    let url = url::Url::parse(&config.server.url).context("некорректный server.url")?;
    let scheme = if url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };
    let host = url.host_str().context("server.url без host")?;
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Ok(format!("{scheme}://{host}{port}"))
}

async fn enroll(config: &RuntimeConfig, code: &str, name: Option<&str>) -> Result<()> {
    let name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&config.server.name)
        .to_owned();
    if name.is_empty() {
        bail!("укажите имя устройства (--name или server.name)");
    }
    let base = enroll_base(config)?;
    let url = if base.ends_with("/api/v1") {
        format!("{base}/ai/runtime/enroll")
    } else {
        format!("{base}/api/v1/ai/runtime/enroll")
    };
    tracing::info!(%url, "регистрация Runtime");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .post(&url)
        .json(&json!({
            "code": code.trim(),
            "name": name,
            "platform": format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            "runtimeVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": protocol::PROTOCOL_VERSION,
        }))
        .send()
        .await
        .context("NEXUS недоступен для enrollment")?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .context("некорректный ответ enrollment")?;
    if !status.is_success() || body.get("success").and_then(|value| value.as_bool()) != Some(true) {
        let message = body
            .get("error")
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or("enrollment отклонён");
        bail!("{message}");
    }
    let data = body
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("ответ без data"))?;
    let node_id = data
        .get("nodeId")
        .and_then(|value| value.as_str())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| anyhow::anyhow!("ответ без nodeId"))?;
    let device_token = data
        .get("deviceToken")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("ответ без deviceToken"))?
        .to_owned();
    let credentials = DeviceCredentials {
        node_id,
        device_token,
        server_url: config.server.url.clone(),
        enrolled_at: chrono::Utc::now(),
        runtime_name: name,
    };
    config.store_credentials(&credentials)?;
    println!("Runtime зарегистрирован. node_id = {node_id}");
    println!(
        "Device token сохранён в {}. Код регистрации использован и больше недействителен.",
        config.token_path().display()
    );
    Ok(())
}

async fn check(config: &RuntimeConfig, json_output: bool) -> Result<()> {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let roots = match config.canonical_roots() {
        Ok(roots) => roots,
        Err(error) => {
            problems.push(error.to_string());
            Vec::new()
        }
    };
    let credentials = match config.load_credentials() {
        Ok(credentials) => Some(credentials),
        Err(error) => {
            problems.push(error.to_string());
            None
        }
    };
    let client = provider::ProviderClient::new(&config.provider)?;
    let (provider_ok, provider_data) = match client.status().await {
        Ok((reachable, data)) => (reachable, data),
        Err(error) => {
            problems.push(format!("PolyStrata: {error}"));
            (false, json!({}))
        }
    };
    let programs = process::ProgramRegistry::new(&config.programs, &config.limits);
    let program_status: Vec<_> = programs
        .descriptors()
        .into_iter()
        .map(|program| {
            if !program.available {
                warnings.push(format!("программа {} недоступна", program.program_id));
            }
            json!({
                "programId": program.program_id,
                "available": program.available,
                "version": program.version,
            })
        })
        .collect();
    let ok = problems.is_empty() && provider_ok;
    if json_output {
        println!(
            "{}",
            json!({
                "ok": ok,
                "version": env!("CARGO_PKG_VERSION"),
                "protocolVersion": protocol::PROTOCOL_VERSION,
                "nodeId": credentials.as_ref().map(|value| value.node_id),
                "serverUrl": config.server.url,
                "provider": {
                    "baseUrl": config.provider.base_url,
                    "reachable": provider_ok,
                    "data": provider_data,
                },
                "allowedRoots": roots.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                "programs": program_status,
                "problems": problems,
                "warnings": warnings,
            })
        );
    } else {
        println!(
            "NEXUS AI Runtime {} — диагностика",
            env!("CARGO_PKG_VERSION")
        );
        println!(
            "  устройство: {}",
            credentials
                .as_ref()
                .map(|value| value.node_id.to_string())
                .unwrap_or_else(|| "не зарегистрировано".to_owned())
        );
        println!("  сервер: {}", config.server.url);
        println!(
            "  PolyStrata: {}",
            if provider_ok {
                "доступна"
            } else {
                "недоступна"
            }
        );
        println!("  разрешённые roots: {}", roots.len());
        for program in &program_status {
            println!(
                "  программа {}: {}",
                program["programId"].as_str().unwrap_or(""),
                if program["available"].as_bool().unwrap_or(false) {
                    "доступна"
                } else {
                    "недоступна"
                }
            );
        }
        for warning in &warnings {
            println!("  внимание: {warning}");
        }
        for problem in &problems {
            eprintln!("  ошибка: {problem}");
        }
        if ok {
            println!("Проверка пройдена.");
        } else {
            println!("Проверка не пройдена.");
        }
    }
    if !ok {
        std::process::exit(2);
    }
    Ok(())
}
