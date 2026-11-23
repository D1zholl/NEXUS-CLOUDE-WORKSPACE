use anyhow::{Context, Result};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;

pub async fn connect(config: &Config) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(if config.database.max_connections == 0 {
            10
        } else {
            config.database.max_connections
        })
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&config.database.url)
        .await
        .context("не удалось подключиться к PostgreSQL")?;
    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("не удалось применить миграции")?;
    Ok(())
}

/// Синхронизирует каталог прав из кода с БД: новые права появляются после обновления.
pub async fn sync_permission_catalog(pool: &PgPool) -> Result<()> {
    for (key, module, label) in crate::permissions::catalog::PERMISSIONS {
        sqlx::query(
            "insert into permissions (key, module, label) values ($1, $2, $3)
             on conflict (key) do update set module = excluded.module, label = excluded.label",
        )
        .bind(key)
        .bind(module)
        .bind(label)
        .execute(pool)
        .await
        .with_context(|| format!("не удалось синхронизировать право {key}"))?;
    }
    crate::ai::tools::ensure_policies(pool)
        .await
        .context("не удалось синхронизировать политики AI-инструментов")?;
    Ok(())
}
