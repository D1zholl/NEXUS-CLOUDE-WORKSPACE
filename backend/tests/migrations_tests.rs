//! K3: все миграции применяются на **пустой** базе.
//!
//! Остальные интеграционные тесты работают в общей `nexus_test` и очищают
//! схему `public`. Этот тест создаёт отдельную одноразовую базу, применяет
//! полный набор миграций и проверяет результат, а затем удаляет базу. Так
//! проверяется именно сценарий «новый хост → `nexus migrate`», а не повторное
//! применение поверх существующей схемы.

use std::path::Path;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("NEXUS_TEST_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nexus:nexus@127.0.0.1:5432/nexus".to_owned())
}

/// Заменяет имя базы в URL, сохраняя параметры подключения.
fn with_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let prefix = base
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .expect("DATABASE_URL должен содержать имя базы");
    match query {
        Some(query) => format!("{prefix}/{database}?{query}"),
        None => format!("{prefix}/{database}"),
    }
}

async fn connect(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "PostgreSQL недоступна по NEXUS_TEST_ADMIN_URL ({error}). \
                 Задайте переменную или запустите локальный PostgreSQL."
            )
        })
}

#[tokio::test]
async fn migrations_apply_to_empty_database() {
    let admin = connect(&admin_url()).await;
    let database = format!("nexus_mig_{}", Uuid::new_v4().simple());

    // Имя состоит только из [a-z0-9_], поэтому подстановка в DDL безопасна.
    sqlx::query(&format!("create database {database}"))
        .execute(&admin)
        .await
        .expect("не удалось создать одноразовую базу");

    let result = verify_clean_database(&with_database(&admin_url(), &database)).await;

    // Удаляем базу даже при ошибке проверки, чтобы не копить мусор.
    let _ = sqlx::query(&format!("drop database if exists {database} with (force)"))
        .execute(&admin)
        .await;
    admin.close().await;

    if let Err(message) = result {
        panic!("{message}");
    }
}

async fn verify_clean_database(url: &str) -> Result<(), String> {
    let pool = connect(url).await;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|error| format!("миграции не применились: {error}"))?;

    let result = check_schema(&pool).await;
    pool.close().await;
    result
}

async fn check_schema(pool: &PgPool) -> Result<(), String> {
    // 1. Все файлы миграций применены ровно один раз и успешно.
    let files = migration_files();
    if files.is_empty() {
        return Err("каталог migrations пуст".to_owned());
    }

    let rows =
        sqlx::query("select version, description, success from _sqlx_migrations order by version")
            .fetch_all(pool)
            .await
            .map_err(|error| format!("не удалось прочитать _sqlx_migrations: {error}"))?;
    if rows.len() != files.len() {
        return Err(format!(
            "применено {} миграций, файлов {}",
            rows.len(),
            files.len()
        ));
    }
    let mut applied = Vec::with_capacity(rows.len());
    for row in &rows {
        let version: i64 = row.get("version");
        let success: bool = row.get("success");
        if !success {
            return Err(format!("миграция {version} помечена как неуспешная"));
        }
        applied.push(version);
    }
    let expected: Vec<i64> = files.iter().map(|(version, _)| *version).collect();
    if applied != expected {
        return Err(format!(
            "версии _sqlx_migrations {applied:?} не совпадают с файлами {expected:?}"
        ));
    }

    // 2. Последняя миграция — синхронизация статуса сборки и релизов.
    if applied.last().copied() != Some(20) {
        return Err(format!(
            "последняя миграция: {:?}, ожидалась 20",
            applied.last()
        ));
    }

    // AI-модуль: ключевые таблицы и права.
    let ai_table: Option<String> =
        sqlx::query_scalar("select to_regclass('public.ai_tasks')::text")
            .fetch_one(pool)
            .await
            .map_err(|error| format!("не удалось проверить ai_tasks: {error}"))?;
    if ai_table.is_none() {
        return Err("таблица ai_tasks не создана".to_owned());
    }
    let ai_permissions: i64 =
        sqlx::query_scalar("select count(*) from permissions where module = 'ai'")
            .fetch_one(pool)
            .await
            .map_err(|error| format!("не удалось проверить ai permissions: {error}"))?;
    if ai_permissions < 10 {
        return Err(format!(
            "права AI синхронизированы не полностью: {ai_permissions}"
        ));
    }

    // 3. Ключевые объекты созданы, а `cloud_trash` — именно view.
    let table: Option<String> =
        sqlx::query_scalar("select to_regclass('public.storage_volumes')::text")
            .fetch_one(pool)
            .await
            .map_err(|error| format!("не удалось проверить storage_volumes: {error}"))?;
    if table.is_none() {
        return Err("таблица storage_volumes не создана".to_owned());
    }

    let trash_kind: Option<String> = sqlx::query_scalar(
        "select c.relkind::text
           from pg_class c
           join pg_namespace n on n.oid = c.relnamespace
          where n.nspname = 'public' and c.relname = 'cloud_trash'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!("не удалось проверить cloud_trash: {error}"))?;
    if trash_kind.as_deref() != Some("v") {
        return Err(format!(
            "cloud_trash должен быть view (relkind = 'v'), получено {trash_kind:?}"
        ));
    }

    let scope_column: Option<String> = sqlx::query_scalar(
        "select column_name
           from information_schema.columns
          where table_schema = 'public'
            and table_name = 'storage_volumes'
            and column_name = 'scope'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!("не удалось проверить storage_volumes.scope: {error}"))?;
    if scope_column.as_deref() != Some("scope") {
        return Err("колонка storage_volumes.scope не создана".to_owned());
    }

    Ok(())
}

/// Пары (номер, имя файла) в порядке применения.
fn migration_files() -> Vec<(i64, String)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut result: Vec<(i64, String)> = std::fs::read_dir(&directory)
        .expect("каталог migrations недоступен")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".sql") {
                return None;
            }
            let version = name.split('_').next()?.parse::<i64>().ok()?;
            Some((version, name))
        })
        .collect();
    result.sort_by_key(|(version, _)| *version);
    result
}
