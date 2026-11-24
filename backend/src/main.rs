use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sqlx::PgPool;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use nexus::auth::password::{hash_password, validate_password, validate_username};
use nexus::config::Config;
use nexus::database;
use nexus::state::AppState;
use nexus::system::monitoring::{MetricsService, spawn_collector};

#[cfg(windows)]
mod windows_service;

#[derive(Parser)]
#[command(name = "nexus", version, about = "NEXUS WORKSPACE server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Запустить HTTP-сервер.
    Serve,
    /// Применить миграции базы данных.
    Migrate,
    /// Создать первого администратора (пароль запрашивается интерактивно).
    CreateAdmin {
        #[arg(long)]
        username: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Пароль из аргумента (для автоматической установки; в истории команд не сохраняйте).
        #[arg(long)]
        password: Option<String>,
        /// Прочитать пароль из переменной окружения (безопаснее аргумента).
        #[arg(long, value_name = "VARIABLE")]
        password_env: Option<String>,
    },
    /// Update an existing superuser account.
    UpdateAdmin {
        #[arg(long)]
        username: Option<String>,
    },
    /// Проверить конфигурацию и доступность зависимостей без запуска сервера.
    Check {
        /// Вывести результат в JSON (для скриптов установки и мониторинга).
        #[arg(long)]
        json: bool,
    },
    /// Управление учётными записями (список, создание, пароли, роли, сессии).
    User {
        #[command(subcommand)]
        command: nexus::users::cli::UserCommand,
    },
    /// Проверка целостности объектов хранилища.
    Objects {
        #[command(subcommand)]
        command: nexus::storage::cli::ObjectsCommand,
    },
}

fn main() -> Result<()> {
    load_environment();

    // Служба Windows стартует с `serve --service`: процесс обязан вызвать
    // StartServiceCtrlDispatcherW, иначе SCM завершает Start-Service с 1053.
    #[cfg(windows)]
    if windows_service::requested() {
        return windows_service::run();
    }

    let cli = Cli::parse();
    let config = Config::load().unwrap_or_else(|error| {
        eprintln!("не удалось загрузить конфигурацию: {error}");
        std::process::exit(2);
    });
    // Guard держит поток записи журнала и сбрасывает буфер при выходе.
    let _log_guard = init_tracing(&config);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("не удалось создать асинхронный рантайм")?
        .block_on(run(config, cli))
}

async fn run(config: Config, cli: Cli) -> Result<()> {
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config, None, Duration::from_secs(30)).await,
        Command::Migrate => migrate(config).await,
        Command::CreateAdmin {
            username,
            display_name,
            password,
            password_env,
        } => create_admin(config, username, display_name, password, password_env).await,
        Command::UpdateAdmin { username } => update_admin(config, username).await,
        Command::Check { json } => check(config, json).await,
        Command::User { command } => match nexus::users::cli::run(config, command).await {
            Ok(()) => Ok(()),
            Err(failure) => {
                eprintln!("{}", failure.message);
                std::process::exit(failure.code);
            }
        },
        Command::Objects { command } => match nexus::storage::cli::run(config, command).await {
            Ok(()) => Ok(()),
            Err(failure) => {
                eprintln!("{}", failure.message);
                std::process::exit(failure.code);
            }
        },
    }
}

/// Загружает .env и nexus.toml рядом с исполняемым файлом, если в текущем
/// каталоге их нет. Служба Windows стартует из System32, поэтому без этого
/// поиска конфигурация установки не находилась бы.
fn load_environment() {
    if dotenvy::dotenv().is_ok() {
        return;
    }

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let env_path = dir.join(".env");
    if !env_path.exists() {
        return;
    }
    if dotenvy::from_path(&env_path).is_err() {
        return;
    }

    if std::env::var("NEXUS_CONFIG").is_err() {
        let toml_path = dir.join("nexus.toml");
        if toml_path.exists() {
            // SAFETY: load_environment вызывается до создания рантайма и потоков,
            // поэтому гонки за переменные окружения нет.
            unsafe { std::env::set_var("NEXUS_CONFIG", &toml_path) };
        }
    }
    // Относительные пути (статика, журналы) считаем от каталога установки.
    let _ = std::env::set_current_dir(dir);
}

/// Настраивает журнал: stderr и, если задан каталог, файл с суточной ротацией.
///
/// Возвращённый guard нужно держать до завершения процесса: он сбрасывает
/// буфер неблокирующего писателя. Для службы Windows журнал — единственный
/// способ увидеть диагностику, потому что stderr никуда не подключён.
fn init_tracing(config: &Config) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("nexus=info,tower_http=info,sqlx=warn"));

    if config.logging.dir.trim().is_empty() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
        return None;
    }

    match init_file_logging(config, filter.clone()) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("не удалось открыть файл журнала: {error}; вывод только в stderr");
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .compact()
                .init();
            None
        }
    }
}

/// Файл журнала на каждый день: `<каталог>/<имя>.<ГГГГ-ММ-ДД>`.
/// `NEXUS_LOG_KEEP` ограничивает число хранимых файлов (минимум один).
fn init_file_logging(
    config: &Config,
    filter: EnvFilter,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = std::path::PathBuf::from(&config.logging.dir);
    std::fs::create_dir_all(&dir).context("не удалось создать каталог журналов")?;

    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(config.logging.file.clone())
        .max_log_files(config.logging.keep_files.max(1) as usize)
        .build(&dir)
        .with_context(|| format!("не удалось открыть журнал в {}", dir.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .compact()
        .with_writer(writer)
        .init();
    Ok(guard)
}

async fn connect_and_migrate(config: &Config) -> Result<PgPool> {
    let pool = database::connect(config).await?;
    database::run_migrations(&pool).await?;
    database::sync_permission_catalog(&pool).await?;
    Ok(pool)
}

async fn migrate(config: Config) -> Result<()> {
    let pool = connect_and_migrate(&config).await?;
    tracing::info!("миграции применены");
    pool.close().await;
    Ok(())
}

/// Проверяет конфигурацию и зависимости, не занимая порт.
/// Код возврата 0 — всё готово, 1 — есть проблемы (для скриптов установки).
async fn check(config: Config, json: bool) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let mut info: Vec<(String, String)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut admin_present = false;

    // Секрет должен быть длинным, иначе сессии и CSRF небезопасны.
    if config.security.secret.trim().len() < 32 {
        problems.push("NEXUS_SECRET должен содержать не менее 32 символов".to_owned());
    }
    if config.security.secret.starts_with("change-me") {
        problems.push("NEXUS_SECRET оставлен значением по умолчанию".to_owned());
    }
    if config.server.public_url.starts_with("https://") && !config.security.cookie_secure {
        problems.push("public_url использует https, но NEXUS_COOKIE_SECURE выключен".to_owned());
    }

    match database::connect(&config).await {
        Ok(pool) => {
            let version: Option<String> = sqlx::query_scalar("select version()")
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
            match version {
                Some(version) => {
                    let short: String = version
                        .split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" ");
                    info.push(("database".to_owned(), short));
                }
                None => problems.push("база данных отвечает ошибкой".to_owned()),
            }

            let migrations: Option<String> =
                sqlx::query_scalar("select max(version)::text from _sqlx_migrations")
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
            match migrations {
                Some(version) => info.push(("migrations".to_owned(), version)),
                None => info.push(("migrations".to_owned(), "не применены".to_owned())),
            }

            let volumes: i64 = sqlx::query_scalar("select count(*) from storage_volumes")
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
            if volumes == 0 {
                warnings.push("тома хранения не зарегистрированы (/system/storage)".to_owned());
            } else {
                info.push(("volumes".to_owned(), volumes.to_string()));
            }

            let admins: i64 =
                sqlx::query_scalar("select count(*) from users where is_superadmin = true")
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0);
            admin_present = admins > 0;
            if !admin_present {
                problems.push(
                    "администратор не создан: выполните `nexus create-admin --username admin`"
                        .to_owned(),
                );
            } else {
                info.push(("admins".to_owned(), admins.to_string()));
            }

            pool.close().await;
        }
        Err(error) => problems.push(format!("не удалось подключиться к PostgreSQL: {error}")),
    }

    match config.resolved_static_dir() {
        Some(dir) => info.push(("static_dir".to_owned(), dir.display().to_string())),
        None => problems.push(format!(
            "каталог статики не найден (static_dir = {:?}); соберите frontend",
            config.server.static_dir
        )),
    }

    if !config.logging.dir.trim().is_empty() {
        let dir = std::path::Path::new(&config.logging.dir);
        if std::fs::create_dir_all(dir).is_err() {
            problems.push(format!(
                "не удалось создать каталог логов {}",
                dir.display()
            ));
        } else {
            info.push(("log_dir".to_owned(), dir.display().to_string()));
        }
    }

    // Эффективные лимиты лаунчера: полезно для проверки .env/переменных среды
    // службы Windows (переменные пользователя в службу не попадают).
    info.push((
        "launcher_limits".to_owned(),
        format!(
            "каталог {}/мин, артефакты {}/мин, параллельно {}",
            config.launcher.requests_per_minute,
            config.launcher.artifacts_per_minute,
            config.launcher.max_parallel_downloads
        ),
    ));

    let ok = problems.is_empty();

    if json {
        // Машиночитаемый вывод для скриптов: без локализованного текста.
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "ok": ok,
            "admin_present": admin_present,
            "info": info.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
            "warnings": warnings,
            "problems": problems,
        });
        println!("{payload}");
    } else {
        println!(
            "NEXUS WORKSPACE {} — проверка конфигурации",
            env!("CARGO_PKG_VERSION")
        );
        for (label, value) in &info {
            let human = match label.as_str() {
                "database" => "база данных".to_owned(),
                "migrations" => "миграции".to_owned(),
                "volumes" => "тома хранения".to_owned(),
                "admins" => "администраторы".to_owned(),
                "static_dir" => "статика".to_owned(),
                "log_dir" => "каталог логов".to_owned(),
                "launcher_limits" => "лимиты лаунчера".to_owned(),
                other => other.to_owned(),
            };
            if label == "database" {
                println!("  {human}: подключение успешно ({value})");
            } else {
                println!("  {human}: {value}");
            }
        }
        for warning in &warnings {
            println!("  внимание: {warning}");
        }
        for problem in &problems {
            eprintln!("  ошибка: {problem}");
        }
    }

    if ok {
        if !json {
            println!("Проверка пройдена: сервер готов к запуску.");
        }
        Ok(())
    } else {
        bail!("проверка не пройдена: проблем — {}", problems.len())
    }
}

async fn create_admin(
    config: Config,
    username: String,
    display_name: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
) -> Result<()> {
    let username = username.trim().to_owned();
    validate_username(&username).map_err(|e| anyhow::anyhow!(e.message))?;
    let display_name = display_name
        .unwrap_or_else(|| "Administrator".to_owned())
        .trim()
        .to_owned();
    if display_name.is_empty() || display_name.chars().count() > 80 {
        bail!("Отображаемое имя должно содержать от 1 до 80 символов");
    }

    // Пароль из переменной окружения удобен для скриптов установки: он не
    // остаётся в истории команд и не виден в списке процессов.
    let (password, confirmation) = match (password, password_env) {
        (Some(password), _) => (password.clone(), password),
        (None, Some(variable)) => {
            let value = std::env::var(&variable)
                .with_context(|| format!("переменная {variable} не задана"))?;
            (value.clone(), value)
        }
        (None, None) => {
            let password = rpassword::prompt_password("Administrator password: ")?;
            let confirmation = rpassword::prompt_password("Repeat password: ")?;
            (password, confirmation)
        }
    };
    validate_password(&password, config.security.password_min_length)
        .map_err(|e| anyhow::anyhow!(e.message))?;
    if password != confirmation {
        bail!("Пароли не совпадают");
    }

    let pool = connect_and_migrate(&config).await?;

    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from users where lower(username) = lower($1)")
            .bind(&username)
            .fetch_optional(&pool)
            .await
            .context("ошибка проверки пользователя")?;
    if existing.is_some() {
        bail!("Пользователь с таким именем уже существует");
    }

    let role_id: Uuid = sqlx::query_scalar("select id from roles where key = 'administrator'")
        .fetch_one(&pool)
        .await
        .context("системная роль administrator не найдена — примените миграции")?;

    let password_hash = hash_password(password)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;

    let mut tx = pool.begin().await?;
    let user_id: Uuid = sqlx::query_scalar(
        "insert into users (username, display_name, password_hash, status, is_superadmin)
         values ($1, $2, $3, 'active', true) returning id",
    )
    .bind(&username)
    .bind(&display_name)
    .bind(&password_hash)
    .fetch_one(&mut *tx)
    .await
    .context("не удалось создать пользователя")?;

    sqlx::query("insert into user_roles (user_id, role_id) values ($1, $2)")
        .bind(user_id)
        .bind(role_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "insert into audit_events
           (event_type, actor_id, actor_username, target_type, target_id, result, details)
         values ('user.created', $1, 'cli', 'user', $2, 'success', jsonb_build_object('superadmin', true))",
    )
    .bind(user_id)
    .bind(user_id.to_string())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    println!("Администратор {username} создан.");
    pool.close().await;
    Ok(())
}

async fn update_admin(config: Config, username: Option<String>) -> Result<()> {
    use std::io::{self, Write};

    let pool = connect_and_migrate(&config).await?;
    let superusers: Vec<(Uuid, String, String)> = sqlx::query_as(
        "select id, username, display_name from users where is_superadmin = true order by username",
    )
    .fetch_all(&pool)
    .await
    .context("failed to look up superusers")?;
    let (user_id, current_username, current_display_name) = match username {
        Some(username) => {
            let username = username.trim().to_owned();
            validate_username(&username).map_err(|e| anyhow::anyhow!(e.message))?;
            superusers
                .into_iter()
                .find(|(_, existing_username, _)| existing_username.eq_ignore_ascii_case(&username))
                .ok_or_else(|| anyhow::anyhow!("No superuser found with username '{username}'."))?
        }
        None => match superusers.as_slice() {
            [superuser] => superuser.clone(),
            [] => bail!("No superuser exists in the configured database."),
            _ => {
                println!("Multiple superusers found:");
                for (index, (_, username, _)) in superusers.iter().enumerate() {
                    println!("{}. {username}", index + 1);
                }
                print!("Choose superuser number: ");
                io::stdout().flush()?;
                let mut selection = String::new();
                io::stdin().read_line(&mut selection)?;
                let selection: usize = selection
                    .trim()
                    .parse()
                    .context("Enter the number of the superuser to update")?;
                if selection == 0 || selection > superusers.len() {
                    bail!(
                        "Superuser number must be between 1 and {}.",
                        superusers.len()
                    );
                }
                superusers[selection - 1].clone()
            }
        },
    };

    print!("New username [{current_username}]: ");
    io::stdout().flush()?;
    let mut new_username = String::new();
    io::stdin().read_line(&mut new_username)?;
    let new_username = if new_username.trim().is_empty() {
        current_username.clone()
    } else {
        new_username.trim().to_owned()
    };
    validate_username(&new_username).map_err(|e| anyhow::anyhow!(e.message))?;

    let duplicate: bool = sqlx::query_scalar(
        "select exists(select 1 from users where lower(username) = lower($1) and id <> $2)",
    )
    .bind(&new_username)
    .bind(user_id)
    .fetch_one(&pool)
    .await?;
    if duplicate {
        bail!("A user with username '{new_username}' already exists.");
    }

    print!("New display name [{current_display_name}]: ");
    io::stdout().flush()?;
    let mut display_name = String::new();
    io::stdin().read_line(&mut display_name)?;
    let display_name = if display_name.trim().is_empty() {
        current_display_name
    } else {
        display_name.trim().to_owned()
    };
    if display_name.is_empty() || display_name.chars().count() > 80 {
        bail!("Display name must contain 1 to 80 characters.");
    }

    let password = rpassword::prompt_password("New password: ")?;
    validate_password(&password, config.security.password_min_length)
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let password_hash = hash_password(password)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;

    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "update users set username = $2, display_name = $3, password_hash = $4,
                failed_login_attempts = 0, locked_until = null, updated_at = now()
         where id = $1 and is_superadmin = true",
    )
    .bind(user_id)
    .bind(&new_username)
    .bind(&display_name)
    .bind(&password_hash)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("The superuser account changed during the operation; no update was committed.");
    }
    sqlx::query(
        "update sessions set revoked_at = now()
         where user_id = $1 and revoked_at is null",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "insert into audit_events
           (event_type, actor_id, actor_username, target_type, target_id, result, details)
         values ('user.superadmin.updated', null, 'cli', 'user', $1, 'success',
                 jsonb_build_object('username', $2, 'passwordChanged', true))",
    )
    .bind(user_id.to_string())
    .bind(&new_username)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    println!("Superuser '{new_username}' was updated; existing sessions were revoked.");
    pool.close().await;
    Ok(())
}

/// Запускает HTTP-сервер.
///
/// `external_shutdown` — канал от диспетчера службы Windows: `true` означает
/// Stop/Shutdown. В консоли (`None`) сигналом служит Ctrl+C / SIGTERM.
/// `shutdown_grace` ограничивает ожидание фоновых задач: в службе оно меньше
/// `WaitToKillServiceTimeout`, чтобы SCM не пришлось убивать процесс.
async fn serve(
    mut config: Config,
    external_shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    shutdown_grace: Duration,
) -> Result<()> {
    let pool = connect_and_migrate(&config).await?;

    // Все лимитеры, кэши и планировщик — in-memory. Второй процесс на той же
    // БД дублировал бы задачи и ломал лимиты, поэтому берём advisory lock.
    // Блокировка живёт на выделенном соединении и снимается при остановке.
    let mut instance_conn = pool
        .acquire()
        .await
        .context("не удалось выделить соединение для блокировки экземпляра")?;
    let instance_lock: bool =
        sqlx::query_scalar("select pg_try_advisory_lock(hashtext('nexus.instance'))")
            .fetch_one(&mut *instance_conn)
            .await
            .context("не удалось проверить единственный экземпляр")?;
    if !instance_lock {
        bail!(
            "NEXUS уже запущен на этой базе данных. \
             Остановите другой экземпляр (или службу NexusWorkspace) и повторите запуск."
        );
    }
    tracing::info!("получена блокировка единственного экземпляра");

    // B5: network.* — restart-only. Значения из system_settings применяются
    // здесь, до создания AppState и сборки маршрутизатора, поэтому CORS и
    // trust proxy меняются только после перезапуска службы.
    nexus::system::settings::apply_startup_overrides(&pool, &mut config).await;
    tracing::info!(
        trust_proxy = config.server.trust_proxy,
        allowed_origins = config.security.allowed_origins.len(),
        "сетевые настройки применены из system_settings"
    );
    // Значения читаются из .env каталога установки или переменных среды
    // уровня машины: служба Windows не наследует переменные пользователя.
    tracing::info!(
        requests_per_minute = config.launcher.requests_per_minute,
        artifacts_per_minute = config.launcher.artifacts_per_minute,
        max_parallel_downloads = config.launcher.max_parallel_downloads,
        "лимиты Launcher API"
    );

    let metrics = MetricsService::new();
    spawn_collector(metrics.clone(), pool.clone());

    let static_dir = config.resolved_static_dir();
    let state = AppState::new(pool.clone(), config.clone(), metrics);
    let recovery = nexus::ai::orchestrator::spawn_recovery_tasks(state.clone()).await;
    if recovery.affected() > 0 {
        tracing::warn!(
            resume = recovery.resume.len(),
            cancel = recovery.cancel.len(),
            reconcile = recovery.reconcile.len(),
            failed_messages = recovery.failed_messages,
            failed_commands = recovery.failed_commands,
            "AI-задачи восстановлены после перезапуска"
        );
    }
    nexus::system::monitoring::spawn_realtime_broadcaster(state.clone());
    // C2: единственный потребитель очереди — планировщик фоновых задач.
    let scheduler = nexus::system::scheduler::spawn(state.clone());
    let app = nexus::build_router(state);

    let address = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .with_context(|| format!("не удалось занять адрес {address}"))?;

    tracing::info!(
        address = %address,
        public_url = %config.server.public_url,
        static_dir = ?static_dir,
        "NEXUS WORKSPACE запущен"
    );

    let shutdown_scheduler = scheduler.clone();
    let shutdown_pool = pool.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        match external_shutdown {
            Some(mut receiver) => {
                while !*receiver.borrow() {
                    if receiver.changed().await.is_err() {
                        break;
                    }
                }
                tracing::info!("получен сигнал остановки службы Windows");
            }
            None => shutdown_signal().await,
        }
        // Новые задачи не берутся, текущим выставляется cancel_requested.
        shutdown_scheduler.shutdown(&shutdown_pool).await;
    })
    .await
    .context("ошибка HTTP-сервера")?;

    // Даём обработчикам завершиться между единицами работы. В службе это
    // время меньше таймаута SCM; не успевшие задачи останутся running и будут
    // возвращены в pending при следующем старте (C2).
    if !scheduler.wait_idle(shutdown_grace).await {
        tracing::warn!(
            grace_secs = shutdown_grace.as_secs(),
            "не все фоновые задачи завершились за отведённое время; \
             при следующем запуске они вернутся в очередь"
        );
    }

    tracing::info!("остановка сервера, закрытие пула БД");
    let _ = sqlx::query("select pg_advisory_unlock(hashtext('nexus.instance'))")
        .execute(&mut *instance_conn)
        .await;
    drop(instance_conn);
    pool.close().await;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "не удалось подписаться на Ctrl+C");
    }
    tracing::info!("получен сигнал завершения");
}
