use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use dashmap::DashMap;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::auth::rate_limit::{
    DEFAULT_LOGIN_MAX_FAILURES, LOGIN_LIMITER_BLOCK_SECS, LOGIN_LIMITER_MAX_FAILURES,
    LOGIN_LIMITER_WINDOW_SECS, LoginLimiter, SHARE_PASSWORD_BLOCK_SECS,
    SHARE_PASSWORD_MAX_FAILURES, SHARE_PASSWORD_WINDOW_SECS, SHARE_UNLOCK_PER_MINUTE,
    TokenBucketLimiter,
};
use crate::auth::session::AuthUser;
use crate::config::Config;
use crate::launcher::downloads::{DownloadLimiter, DownloadSlot};
use crate::realtime::Hub;
use crate::system::monitoring::MetricsService;
use crate::system::requests::RequestMetrics;
use crate::system::settings_cache::SettingsCache;

/// Кэш состояния модулей с коротким TTL.
///
/// Отключение модуля должно блокировать его API без перезапуска сервера,
/// но чтение `module_states` на каждый запрос недопустимо. Поэтому значения
/// кэшируются на 5 секунд и инвалидируются при изменении из `/system/modules`.
pub struct ModuleCache {
    entries: Mutex<Option<(Instant, HashMap<String, bool>)>>,
}

impl Default for ModuleCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(None),
        }
    }

    /// Читает состояние модуля. По умолчанию модуль включён.
    pub async fn enabled(&self, db: &PgPool, key: &str) -> bool {
        let mut guard = self.entries.lock().await;
        let stale = guard
            .as_ref()
            .map(|(at, _)| at.elapsed() > Duration::from_secs(5))
            .unwrap_or(true);
        if stale {
            let rows: Vec<(String, bool)> =
                sqlx::query_as("select key, enabled from module_states")
                    .fetch_all(db)
                    .await
                    .unwrap_or_default();
            let mut map: HashMap<String, bool> = HashMap::new();
            map.insert("minecraft".to_owned(), true);
            map.insert("cloud".to_owned(), true);
            map.insert("ai".to_owned(), true);
            for (name, enabled) in rows {
                map.insert(name, enabled);
            }
            *guard = Some((Instant::now(), map));
        }
        guard
            .as_ref()
            .and_then(|(_, map)| map.get(key).copied())
            .unwrap_or(true)
    }

    /// Сбрасывает кэш после изменения состояния модуля.
    pub async fn invalidate(&self) {
        *self.entries.lock().await = None;
    }

    /// Возвращает карту состояний (для `/auth/me` и `/system/overview`).
    pub async fn snapshot(&self, db: &PgPool) -> HashMap<String, bool> {
        // Прогреваем кэш, чтобы не дублировать логику.
        let _ = self.enabled(db, "minecraft").await;
        let guard = self.entries.lock().await;
        guard
            .as_ref()
            .map(|(_, map)| map.clone())
            .unwrap_or_default()
    }
}

/// Время жизни записи кэша проверки сессии.
pub const SESSION_CACHE_TTL: Duration = Duration::from_secs(20);

/// Проверенная сессия с моментом попадания в кэш.
#[derive(Clone)]
pub struct CachedSession {
    pub auth: AuthUser,
    pub cached_at: Instant,
}

pub struct AppState {
    pub db: PgPool,
    pub config: Config,
    pub started_at: Instant,
    pub login_limiter: LoginLimiter,
    pub metrics: MetricsService,
    pub requests: RequestMetrics,
    pub modules: ModuleCache,
    pub realtime: Hub,
    pub settings: SettingsCache,
    /// Лимит запросов каталога/манифестов лаунчера (по токену).
    pub launcher_limiter: TokenBucketLimiter,
    /// Лимит скачиваний артефактов (по токену).
    pub artifact_limiter: TokenBucketLimiter,
    /// Лимит одновременных скачиваний артефактов (по токену).
    pub downloads: DownloadLimiter,
    /// Защита пароля публичных ссылок от перебора (ключ `share_id|ip`).
    pub share_passwords: LoginLimiter,
    /// Общий лимит запросов `unlock` на IP.
    pub share_unlock: TokenBucketLimiter,
    /// Блокировки параллельных `PATCH` одной сессии загрузки.
    pub upload_locks: DashMap<Uuid, Arc<Mutex<()>>>,
    /// Кэш проверки сессий: ключ — SHA-256 токена сессии.
    pub session_cache: DashMap<String, CachedSession>,
    pub session_cache_hits: AtomicU64,
    pub session_cache_misses: AtomicU64,
    /// AI Workspace: Runtime hub, стримы и токены отмены.
    pub ai: crate::ai::AiState,
}

impl AppState {
    pub fn new(db: PgPool, config: Config, metrics: MetricsService) -> Arc<Self> {
        let requests = RequestMetrics::new();
        let downloads =
            DownloadLimiter::new(config.launcher.max_parallel_downloads, requests.clone());
        let ai = crate::ai::AiState::new(&config.security.secret);
        Arc::new(Self {
            db,
            started_at: Instant::now(),
            login_limiter: LoginLimiter::new(
                LOGIN_LIMITER_MAX_FAILURES,
                LOGIN_LIMITER_WINDOW_SECS,
                LOGIN_LIMITER_BLOCK_SECS,
            ),
            metrics,
            requests,
            modules: ModuleCache::new(),
            realtime: Hub::new(),
            settings: SettingsCache::new(),
            launcher_limiter: TokenBucketLimiter::new(
                config.launcher.requests_per_minute,
                config.launcher.requests_per_minute,
            ),
            artifact_limiter: TokenBucketLimiter::new(
                config.launcher.artifacts_per_minute,
                config.launcher.artifacts_per_minute,
            ),
            downloads,
            share_passwords: LoginLimiter::new(
                SHARE_PASSWORD_MAX_FAILURES,
                SHARE_PASSWORD_WINDOW_SECS,
                SHARE_PASSWORD_BLOCK_SECS,
            ),
            share_unlock: TokenBucketLimiter::new(SHARE_UNLOCK_PER_MINUTE, SHARE_UNLOCK_PER_MINUTE),
            upload_locks: DashMap::new(),
            session_cache: DashMap::new(),
            session_cache_hits: AtomicU64::new(0),
            session_cache_misses: AtomicU64::new(0),
            ai,
            config,
        })
    }

    /// Возвращает проверенную сессию из кэша, если TTL не истёк.
    pub fn cached_session(&self, token_hash: &str) -> Option<AuthUser> {
        let entry = self.session_cache.get(token_hash)?;
        let stale = entry.cached_at.elapsed() > SESSION_CACHE_TTL;
        let auth = entry.auth.clone();
        drop(entry);
        if stale || auth.expires_at <= Utc::now() {
            self.session_cache.remove(token_hash);
            return None;
        }
        self.session_cache_hits.fetch_add(1, Ordering::Relaxed);
        Some(auth)
    }

    /// Кладёт проверенную сессию в кэш.
    pub fn cache_session(&self, token_hash: String, auth: AuthUser) {
        if self.session_cache.len() > 10_000 {
            let ttl = SESSION_CACHE_TTL;
            self.session_cache
                .retain(|_, entry| entry.cached_at.elapsed() <= ttl);
        }
        self.session_cache.insert(
            token_hash,
            CachedSession {
                auth,
                cached_at: Instant::now(),
            },
        );
    }

    pub fn note_session_cache_miss(&self) {
        self.session_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Инвалидирует конкретную сессию (выход из системы).
    pub fn invalidate_session(&self, session_id: Uuid) {
        self.session_cache
            .retain(|_, entry| entry.auth.session_id != session_id);
    }

    /// Инвалидирует все сессии пользователя (смена пароля, блокировка, роли).
    pub fn invalidate_user_sessions(&self, user_id: Uuid) {
        self.session_cache
            .retain(|_, entry| entry.auth.id != user_id);
    }

    /// Полная инвалидация: изменение ролей и прав затрагивает многих.
    pub fn invalidate_all_sessions(&self) {
        self.session_cache.clear();
    }

    /// Счётчики hit/miss для мониторинга.
    pub fn session_cache_stats(&self) -> (u64, u64) {
        (
            self.session_cache_hits.load(Ordering::Relaxed),
            self.session_cache_misses.load(Ordering::Relaxed),
        )
    }

    /// Возвращает блокировку сессии загрузки, создавая её при необходимости.
    pub fn upload_lock(&self, session_id: Uuid) -> Arc<Mutex<()>> {
        self.upload_locks
            .entry(session_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Удаляет блокировку, если на неё больше никто не ссылается.
    pub fn release_upload_lock(&self, session_id: Uuid) {
        let removable = self
            .upload_locks
            .get(&session_id)
            .map(|entry| Arc::strong_count(entry.value()) == 1)
            .unwrap_or(false);
        if removable {
            self.upload_locks.remove(&session_id);
        }
    }

    /// Занимает слот параллельного скачивания для токена лаунчера.
    pub fn acquire_download(&self, token_id: Uuid) -> Result<DownloadSlot, u64> {
        self.downloads.acquire(token_id)
    }

    pub async fn session_ttl_hours(&self) -> i64 {
        self.settings
            .session_ttl_hours(&self.db, self.config.security.session_ttl_hours)
            .await
    }

    /// TTL ссылки общего доступа по умолчанию.
    pub async fn share_ttl_hours(&self) -> i64 {
        self.settings
            .share_ttl_hours(&self.db, self.config.storage.share_default_hours)
            .await
    }

    /// TTL готового ZIP-архива.
    pub async fn export_ttl_hours(&self) -> i64 {
        self.settings
            .export_ttl_hours(&self.db, self.config.storage.export_ttl_hours)
            .await
    }

    /// Срок хранения записей корзины (дней). Fallback — 30 дней.
    pub async fn trash_retention_days(&self) -> i64 {
        self.settings.trash_retention_days(&self.db, 30).await
    }

    /// Срок хранения мягко удалённых AI workspace (дней).
    /// Fallback — 30 дней; 0/некорректное значение не допускается каталогом.
    pub async fn ai_workspace_retention_days(&self) -> i64 {
        self.settings
            .ai_workspace_retention_days(&self.db, 30)
            .await
            .clamp(1, 3650)
    }

    /// Число параллельных загрузок в браузере.
    pub async fn upload_concurrency(&self) -> usize {
        self.settings.upload_concurrency(&self.db, 3).await
    }

    /// Порог неудачных попыток входа до блокировки.
    pub async fn login_max_failures(&self) -> i64 {
        self.settings
            .login_max_failures(&self.db, DEFAULT_LOGIN_MAX_FAILURES)
            .await
    }

    /// Минимальная длина пароля.
    pub async fn password_min_length(&self) -> usize {
        self.settings
            .password_min_length(&self.db, self.config.security.password_min_length)
            .await
    }

    /// Лимит параллельных фоновых задач.
    ///
    /// Значение читается планировщиком (`system/scheduler.rs`) на каждом цикле,
    /// поэтому изменение применяется без перезапуска. Fallback — 2.
    pub async fn jobs_max_concurrent(&self) -> usize {
        self.settings.jobs_max_concurrent(&self.db, 2).await
    }

    /// Включён ли модуль сейчас (с учётом кэша).
    pub async fn module_enabled(&self, key: &str) -> bool {
        self.modules.enabled(&self.db, key).await
    }

    /// Эффективный лимит размера файла с учётом рантайм-настроек.
    pub async fn max_upload_bytes(&self) -> u64 {
        self.settings
            .max_upload_bytes(&self.db, self.config.storage.max_upload_bytes)
            .await
    }

    /// Эффективный размер чанка загрузки.
    pub async fn chunk_max_bytes(&self) -> u64 {
        self.settings
            .chunk_max_bytes(&self.db, self.config.storage.chunk_max_bytes)
            .await
    }

    /// Эффективный TTL незавершённой загрузки.
    pub async fn upload_ttl_hours(&self) -> i64 {
        self.settings
            .upload_ttl_hours(&self.db, self.config.storage.upload_ttl_hours)
            .await
    }
}
