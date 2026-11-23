//! Рантайм-переопределения системных настроек (PLAN §9.1).
//!
//! Значения из `system_settings` переопределяют `nexus.toml` там, где это
//! возможно без перезапуска: лимиты загрузки, TTL, число задач. Кэш с TTL
//! 10 секунд избавляет от запроса к БД на каждую операцию.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(10);

type Snapshot = (Instant, HashMap<String, serde_json::Value>);

#[derive(Clone)]
pub struct SettingsCache {
    entries: Arc<Mutex<Option<Snapshot>>>,
}

impl Default for SettingsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsCache {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(None)),
        }
    }

    async fn snapshot(&self, db: &PgPool) -> HashMap<String, serde_json::Value> {
        let mut guard = self.entries.lock().await;
        let stale = guard
            .as_ref()
            .map(|(at, _)| at.elapsed() > TTL)
            .unwrap_or(true);
        if stale {
            let rows: Vec<(String, serde_json::Value)> =
                sqlx::query_as("select key, value from system_settings")
                    .fetch_all(db)
                    .await
                    .unwrap_or_default();
            *guard = Some((Instant::now(), rows.into_iter().collect()));
        }
        guard
            .as_ref()
            .map(|(_, map)| map.clone())
            .unwrap_or_default()
    }

    pub async fn invalidate(&self) {
        *self.entries.lock().await = None;
    }

    async fn int(&self, db: &PgPool, key: &str) -> Option<i64> {
        self.snapshot(db)
            .await
            .get(key)
            .and_then(|value| value.as_i64())
    }

    /// Произвольное JSON-значение настройки (для `appearance.*`).
    pub async fn json(&self, db: &PgPool, key: &str) -> Option<serde_json::Value> {
        self.snapshot(db).await.get(key).cloned()
    }

    /// Максимальный размер файла (байты).
    pub async fn max_upload_bytes(&self, db: &PgPool, fallback: u64) -> u64 {
        match self.int(db, "limits.max_upload_mb").await {
            Some(mb) if mb > 0 => (mb as u64) * 1024 * 1024,
            _ => fallback,
        }
    }

    /// Размер чанка (байты).
    pub async fn chunk_max_bytes(&self, db: &PgPool, fallback: u64) -> u64 {
        match self.int(db, "limits.chunk_max_mb").await {
            Some(mb) if mb > 0 => (mb as u64) * 1024 * 1024,
            _ => fallback,
        }
    }

    pub async fn upload_ttl_hours(&self, db: &PgPool, fallback: i64) -> i64 {
        self.int(db, "limits.upload_ttl_hours")
            .await
            .unwrap_or(fallback)
    }

    pub async fn share_ttl_hours(&self, db: &PgPool, fallback: i64) -> i64 {
        self.int(db, "limits.share_ttl_hours")
            .await
            .unwrap_or(fallback)
    }

    pub async fn export_ttl_hours(&self, db: &PgPool, fallback: i64) -> i64 {
        self.int(db, "limits.export_ttl_hours")
            .await
            .unwrap_or(fallback)
    }

    /// Срок хранения записей корзины (дней).
    pub async fn trash_retention_days(&self, db: &PgPool, fallback: i64) -> i64 {
        self.int(db, "limits.trash_retention_days")
            .await
            .unwrap_or(fallback)
    }

    pub async fn session_ttl_hours(&self, db: &PgPool, fallback: i64) -> i64 {
        self.int(db, "security.session_ttl_hours")
            .await
            .unwrap_or(fallback)
    }

    pub async fn jobs_max_concurrent(&self, db: &PgPool, fallback: usize) -> usize {
        match self.int(db, "jobs.max_concurrent").await {
            Some(value) if value > 0 => value as usize,
            _ => fallback,
        }
    }

    /// Число параллельных загрузок в браузере.
    pub async fn upload_concurrency(&self, db: &PgPool, fallback: usize) -> usize {
        match self.int(db, "cloud.upload.concurrency").await {
            Some(value) if value > 0 => value as usize,
            _ => fallback,
        }
    }

    /// Порог неудачных попыток входа до блокировки учётной записи.
    pub async fn login_max_failures(&self, db: &PgPool, fallback: i64) -> i64 {
        match self.int(db, "security.login_max_failures").await {
            Some(value) if value > 0 => value,
            _ => fallback,
        }
    }

    /// Минимальная длина пароля.
    pub async fn password_min_length(&self, db: &PgPool, fallback: usize) -> usize {
        match self.int(db, "security.password_min_length").await {
            Some(value) if value > 0 => value as usize,
            _ => fallback,
        }
    }
}
