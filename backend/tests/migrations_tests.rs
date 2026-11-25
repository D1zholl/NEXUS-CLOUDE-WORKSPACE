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

    // 2. Последняя миграция соответствует последнему файлу каталога.
    if applied.last() != expected.last() {
        return Err(format!(
            "последняя миграция: {:?}, ожидалась {:?}",
            applied.last(),
            expected.last()
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

    // Agent Mode delta (0021): индексы и колонки.
    let active_index: Option<String> =
        sqlx::query_scalar("select to_regclass('public.ai_tasks_active_conversation_uq')::text")
            .fetch_one(pool)
            .await
            .map_err(|error| {
                format!("не удалось проверить ai_tasks_active_conversation_uq: {error}")
            })?;
    if active_index.is_none() {
        return Err("уникальный индекс активного запуска не создан".to_owned());
    }
    let skills_column: Option<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
          where table_schema = 'public' and table_name = 'ai_tasks'
            and column_name = 'skill_ids'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!("не удалось проверить ai_tasks.skill_ids: {error}"))?;
    if skills_column.as_deref() != Some("skill_ids") {
        return Err("колонка ai_tasks.skill_ids не создана".to_owned());
    }
    let iterations_column: Option<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
          where table_schema = 'public' and table_name = 'ai_user_quotas'
            and column_name = 'max_iterations'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!("не удалось проверить ai_user_quotas.max_iterations: {error}"))?;
    if iterations_column.as_deref() != Some("max_iterations") {
        return Err("колонка ai_user_quotas.max_iterations не создана".to_owned());
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

fn migration_sql(name: &str) -> String {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    std::fs::read_to_string(directory.join(name))
        .unwrap_or_else(|error| panic!("не удалось прочитать {name}: {error}"))
}

async fn apply_migrations_upto(pool: &PgPool, max_version: i64) -> Result<(), String> {
    for (version, name) in migration_files() {
        if version > max_version {
            continue;
        }
        let sql = migration_sql(&name);
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(())
}

/// 0021 проверяется на отдельной тестовой базе:
/// 1. на дублях `root_path` миграция обязана упасть, ничего не удаляя;
/// 2. на двух активных задачах одного диалога — тоже упасть;
/// 3. после ручного устранения дублей — примениться;
/// 4. повторный запуск идемпотентен.
#[tokio::test]
async fn migration_0021_preconditions_and_repeatability() {
    let admin = connect(&admin_url()).await;
    let database = format!("nexus_mig21_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("create database {database}"))
        .execute(&admin)
        .await
        .expect("не удалось создать одноразовую базу");

    let url = with_database(&admin_url(), &database);
    let result = check_migration_0021(&url).await;

    let _ = sqlx::query(&format!("drop database if exists {database} with (force)"))
        .execute(&admin)
        .await;
    admin.close().await;

    if let Err(message) = result {
        panic!("{message}");
    }
}

async fn check_migration_0021(url: &str) -> Result<(), String> {
    let pool = connect(url).await;
    apply_migrations_upto(&pool, 20).await?;

    let user: Uuid = sqlx::query_scalar(
        "insert into users (username, display_name, password_hash)
         values ('mig021', 'Migration', 'hash') returning id",
    )
    .fetch_one(&pool)
    .await
    .map_err(|error| format!("не удалось создать пользователя: {error}"))?;
    let node: Uuid = sqlx::query_scalar(
        "insert into ai_runtime_nodes (name, token_hash)
         values ('MIG-NODE', 'mig-token-hash') returning id",
    )
    .fetch_one(&pool)
    .await
    .map_err(|error| format!("не удалось создать Runtime: {error}"))?;

    // Дубли root_path с точностью до регистра.
    for (name, root) in [
        ("alpha", r"C:\NexusWorkspaces\Alpha"),
        ("alpha-dup", r"c:\nexusworkspaces\alpha"),
    ] {
        sqlx::query(
            "insert into ai_workspaces (owner_id, runtime_node_id, name, root_path)
             values ($1, $2, $3, $4)",
        )
        .bind(user)
        .bind(node)
        .bind(name)
        .bind(root)
        .execute(&pool)
        .await
        .map_err(|error| format!("не удалось создать workspace {name}: {error}"))?;
    }

    let sql_0021 = migration_sql("0021_agent_mode_delta.sql");
    let error = sqlx::raw_sql(&sql_0021)
        .execute(&pool)
        .await
        .expect_err("миграция должна отказать при дублях root_path");
    let message = error.to_string();
    if !message.contains("ai_workspaces_node_root_uq") {
        return Err(format!(
            "ожидалась ошибка уникального индекса root_path, получено: {message}"
        ));
    }
    // Данные не удалены автоматически.
    let workspaces: i64 = sqlx::query_scalar("select count(*) from ai_workspaces")
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("не удалось посчитать workspaces: {error}"))?;
    if workspaces != 2 {
        return Err(format!(
            "миграция не должна удалять/сливать данные: workspaces = {workspaces}"
        ));
    }

    sqlx::query("delete from ai_workspaces where name = 'alpha-dup'")
        .execute(&pool)
        .await
        .map_err(|error| format!("не удалось удалить дубль вручную: {error}"))?;

    // Две активные задачи в одном диалоге.
    let conversation: Uuid = sqlx::query_scalar(
        "insert into ai_conversations (owner_id, mode) values ($1, 'agent') returning id",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .map_err(|error| format!("не удалось создать диалог: {error}"))?;
    for status in ["running", "waiting_approval"] {
        sqlx::query(
            "insert into ai_tasks (conversation_id, owner_id, runtime_node_id, status)
             values ($1, $2, $3, $4)",
        )
        .bind(conversation)
        .bind(user)
        .bind(node)
        .bind(status)
        .execute(&pool)
        .await
        .map_err(|error| format!("не удалось создать задачу {status}: {error}"))?;
    }
    let error = sqlx::raw_sql(&sql_0021)
        .execute(&pool)
        .await
        .expect_err("миграция должна отказать при двух активных задачах");
    let message = error.to_string();
    if !message.contains("ai_tasks_active_conversation_uq") {
        return Err(format!(
            "ожидалась ошибка уникального индекса активной задачи, получено: {message}"
        ));
    }

    sqlx::query("update ai_tasks set status = 'completed' where status = 'waiting_approval'")
        .execute(&pool)
        .await
        .map_err(|error| format!("не удалось завершить вторую задачу: {error}"))?;

    // Предусловия устранены — миграция применяется и повторяется без ошибок.
    for attempt in 1..=2 {
        sqlx::raw_sql(&sql_0021)
            .execute(&pool)
            .await
            .map_err(|error| format!("0021 не применилась на попытке {attempt}: {error}"))?;
    }

    for (table, column) in [
        ("ai_messages", "task_id"),
        ("ai_messages", "iteration"),
        ("ai_tasks", "skill_ids"),
        ("ai_user_quotas", "max_iterations"),
        ("ai_tool_executions", "output_tail"),
        ("ai_tool_executions", "process_id"),
    ] {
        let exists: Option<String> = sqlx::query_scalar(
            "select column_name from information_schema.columns
              where table_schema = 'public' and table_name = $1 and column_name = $2",
        )
        .bind(table)
        .bind(column)
        .fetch_optional(&pool)
        .await
        .map_err(|error| format!("не удалось проверить {table}.{column}: {error}"))?;
        if exists.is_none() {
            return Err(format!("колонка {table}.{column} не создана"));
        }
    }
    for index in [
        "ai_workspaces_node_root_uq",
        "ai_tasks_active_conversation_uq",
    ] {
        let exists: Option<String> = sqlx::query_scalar("select to_regclass($1)::text")
            .bind(format!("public.{index}"))
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("не удалось проверить индекс {index}: {error}"))?;
        if exists.is_none() {
            return Err(format!("индекс {index} не создан"));
        }
    }
    sqlx::query("insert into ai_user_quotas (user_id) values ($1) on conflict do nothing")
        .bind(user)
        .execute(&pool)
        .await
        .map_err(|error| format!("не удалось создать квоту: {error}"))?;
    let default_iterations: i32 =
        sqlx::query_scalar("select max_iterations from ai_user_quotas where user_id = $1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("не удалось прочитать max_iterations: {error}"))?;
    if default_iterations != 10 {
        return Err(format!(
            "ожидался default max_iterations = 10, получено {default_iterations}"
        ));
    }
    let invalid = sqlx::query("update ai_user_quotas set max_iterations = 0 where user_id = $1")
        .bind(user)
        .execute(&pool)
        .await;
    if invalid.is_ok() {
        return Err("check constraint max_iterations (1..50) не применяется".to_owned());
    }
    pool.close().await;
    Ok(())
}
