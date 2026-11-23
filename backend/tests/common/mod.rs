//! Общий каркас интеграционных тестов.
//!
//! Тесты работают с реальным PostgreSQL. Тестовая БД создаётся отдельно
//! (`nexus_test`) и заполняется миграциями; каждый запуск начинается с чистой схемы.
//! Если БД недоступна, тест падает с понятным сообщением, а не «проходит» ложно.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nexus::config::{Config, DatabaseConfig, SecurityConfig, ServerConfig, StorageConfig};
use nexus::state::AppState;
use nexus::system::monitoring::MetricsService;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

pub const TEST_DB: &str = "nexus_test";

/// Временный каталог, который на Windows удаляется с повторами.
///
/// `tempfile::TempDir` пытается удалить дерево один раз и молча оставляет его
/// при `ERROR_SHARING_VIOLATION`. Здесь тот же путь известен заранее, а
/// удаление повторяется, пока антивирус/индексатор не отпустят файл.
pub struct RetryTempDir {
    path: PathBuf,
}

impl RetryTempDir {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("nexus-test-")
            .tempdir()
            .expect("не удалось создать временный каталог");
        Self { path: dir.keep() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RetryTempDir {
    fn drop(&mut self) {
        for attempt in 0..20 {
            match std::fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if attempt == 19 => {
                    eprintln!(
                        "временный каталог не удалён после 20 попыток: {} ({error})",
                        self.path.display()
                    );
                    return;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(25)),
            }
        }
    }
}

/// Тесты делят одну БД, поэтому подготовка схемы сериализуется.
static DB_LOCK: Mutex<()> = Mutex::const_new(());

fn admin_url() -> String {
    std::env::var("NEXUS_TEST_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nexus:nexus@127.0.0.1:5432/nexus".to_owned())
}

fn test_url() -> String {
    std::env::var("NEXUS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| format!("postgres://nexus:nexus@127.0.0.1:5432/{TEST_DB}"))
}

async fn ensure_database() {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect(
            "тестовая PostgreSQL недоступна: задайте NEXUS_TEST_ADMIN_URL или запустите локальный PostgreSQL",
        );
    let exists: Option<String> =
        sqlx::query_scalar("select datname from pg_database where datname = $1")
            .bind(TEST_DB)
            .fetch_optional(&admin)
            .await
            .expect("не удалось проверить наличие тестовой БД");
    if exists.is_none() {
        // Имя БД — фиксированная константа, пользовательский ввод не подставляется.
        sqlx::query(&format!("create database {TEST_DB}"))
            .execute(&admin)
            .await
            .expect("не удалось создать тестовую БД");
    }
    admin.close().await;
}

/// Чистая тестовая БД с применёнными миграциями.
pub struct TestEnv {
    pub pool: PgPool,
    pub state: Arc<AppState>,
    pub root: RetryTempDir,
    _guard: MutexGuard<'static, ()>,
}

impl TestEnv {
    pub async fn new() -> Self {
        let guard = DB_LOCK.lock().await;
        ensure_database().await;
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&test_url())
            .await
            .expect("не удалось подключиться к тестовой БД");

        sqlx::query("drop schema public cascade")
            .execute(&pool)
            .await
            .expect("не удалось очистить схему");
        sqlx::query("create schema public")
            .execute(&pool)
            .await
            .expect("не удалось создать схему");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("не удалось применить миграции");
        nexus::database::sync_permission_catalog(&pool)
            .await
            .expect("не удалось синхронизировать права");

        let root = RetryTempDir::new();
        let config = test_config(&test_url());
        let state = AppState::new(pool.clone(), config, MetricsService::new());
        Self {
            pool,
            state,
            root,
            _guard: guard,
        }
    }

    /// Регистрирует временный том хранилища внутри временного каталога теста.
    pub async fn add_volume(&self, name: &str) -> Uuid {
        let path = self.root.path().join(name);
        std::fs::create_dir_all(&path).expect("не удалось создать каталог тома");
        nexus::storage::engine::probe_volume(&path)
            .await
            .expect("том должен быть доступен для записи");
        // Свободное место берём с реального диска, иначе авто-выбор тома откажет.
        let (total, available) = nexus::storage::engine::disk_space(&path);
        let id: Uuid = sqlx::query_scalar(
            "insert into storage_volumes
               (name, root_path, mode, priority, state, total_bytes, available_bytes)
             values ($1, $2, 'auto', 100, 'online', $3, $4)
             returning id",
        )
        .bind(name)
        .bind(path.to_string_lossy().to_string())
        .bind(total as i64)
        .bind(available as i64)
        .fetch_one(&self.pool)
        .await
        .expect("не удалось зарегистрировать том");
        id
    }

    pub async fn create_user(&self, username: &str, password: &str) -> Uuid {
        let hash = nexus::auth::password::hash_password(password.to_owned())
            .await
            .expect("не удалось захешировать пароль");
        let id: Uuid = sqlx::query_scalar(
            "insert into users (username, display_name, password_hash, status)
             values ($1, $1, $2, 'active') returning id",
        )
        .bind(username)
        .bind(hash)
        .fetch_one(&self.pool)
        .await
        .expect("не удалось создать пользователя");
        sqlx::query(
            "insert into user_roles (user_id, role_id)
             select $1, id from roles where key = 'administrator'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .expect("не удалось выдать роль");
        id
    }
}

fn test_config(url: &str) -> Config {
    Config {
        database: DatabaseConfig {
            url: url.to_owned(),
            max_connections: 5,
        },
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            public_url: "http://localhost".into(),
            static_dir: String::new(),
            trust_proxy: false,
        },
        security: SecurityConfig {
            session_ttl_hours: 24,
            password_min_length: nexus::auth::password::MIN_PASSWORD_LEN,
            cookie_secure: false,
            secret: "test-secret-test-secret-test-secret-32".to_owned(),
            allowed_origins: Vec::new(),
        },
        storage: StorageConfig {
            max_upload_bytes: 64 * 1024 * 1024,
            chunk_max_bytes: 1024 * 1024,
            upload_ttl_hours: 24,
            share_default_hours: 24,
            export_ttl_hours: 1,
        },
        launcher: Default::default(),
        logging: Default::default(),
    }
}
