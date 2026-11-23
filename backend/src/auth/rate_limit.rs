use std::time::{Duration, Instant};

use dashmap::DashMap;

#[derive(Debug, Default, Clone)]
struct Entry {
    failures: u32,
    window_start: Option<Instant>,
    blocked_until: Option<Instant>,
}

/// Рекомендуемые лимиты запросов Launcher API.
pub const LAUNCHER_REQUESTS_PER_MINUTE: u32 = 120;
pub const LAUNCHER_ARTIFACTS_PER_MINUTE: u32 = 600;
/// Одновременных скачиваний артефактов на один токен.
pub const LAUNCHER_MAX_PARALLEL_DOWNLOADS: usize = 8;

/// Блокировка учётной записи после серии неудачных входов.
pub const ACCOUNT_LOCK_MINUTES: i64 = 15;
pub const ACCOUNT_LOCK_SECS: u64 = ACCOUNT_LOCK_MINUTES as u64 * 60;

/// Независимая защита от перебора: 5 ошибок за 15 минут → блок на 15 минут.
pub const LOGIN_LIMITER_MAX_FAILURES: u32 = 5;
pub const LOGIN_LIMITER_WINDOW_SECS: u64 = 15 * 60;
pub const LOGIN_LIMITER_BLOCK_SECS: u64 = ACCOUNT_LOCK_SECS;

/// Порог блокировки по умолчанию, если настройка в БД недоступна.
pub const DEFAULT_LOGIN_MAX_FAILURES: i64 = 10;

/// Пароль публичной ссылки: 5 ошибок за 15 минут → блок на 15 минут.
pub const SHARE_PASSWORD_MAX_FAILURES: u32 = 5;
pub const SHARE_PASSWORD_WINDOW_SECS: u64 = 15 * 60;
pub const SHARE_PASSWORD_BLOCK_SECS: u64 = 15 * 60;

/// Общий лимит запросов `POST /public/shares/{token}/unlock` на IP.
pub const SHARE_UNLOCK_PER_MINUTE: u32 = 10;

/// In-memory лимитер попыток входа (IP + username).
/// Дополнительно к нему в БД ведётся блокировка аккаунта.
pub struct LoginLimiter {
    entries: DashMap<String, Entry>,
    max_failures: u32,
    window: Duration,
    block: Duration,
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window_secs: u64, block_secs: u64) -> Self {
        Self {
            entries: DashMap::new(),
            max_failures,
            window: Duration::from_secs(window_secs),
            block: Duration::from_secs(block_secs),
        }
    }

    /// Возвращает Err(retry_after_seconds), если ключ заблокирован.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        if let Some(entry) = self.entries.get(key) {
            if let Some(until) = entry.blocked_until {
                let now = Instant::now();
                if until > now {
                    return Err((until - now).as_secs().max(1));
                }
            }
        }
        Ok(())
    }

    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let mut entry = self.entries.entry(key.to_owned()).or_default();
        match entry.window_start {
            Some(start) if now.duration_since(start) <= self.window => {
                entry.failures += 1;
            }
            _ => {
                entry.failures = 1;
                entry.window_start = Some(now);
            }
        }
        if entry.failures >= self.max_failures {
            entry.blocked_until = Some(now + self.block);
            entry.failures = 0;
            entry.window_start = Some(now);
        }
    }

    pub fn clear(&self, key: &str) {
        self.entries.remove(key);
    }

    /// Удаляет устаревшие записи, чтобы карта не росла бесконечно.
    pub fn prune(&self) {
        let now = Instant::now();
        self.entries.retain(|_, e| {
            let blocked = e.blocked_until.map(|u| u > now).unwrap_or(false);
            let recent = e
                .window_start
                .map(|s| now.duration_since(s) <= self.window)
                .unwrap_or(false);
            blocked || recent
        });
    }
}

/// Состояние одной корзины токенов.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    /// Сколько запросов подряд превысили лимит (для аудита).
    overruns: u32,
}

/// Состояние корзины после успешного запроса — для заголовков `X-RateLimit-*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitSnapshot {
    /// Ёмкость корзины (всплеск).
    pub limit: u32,
    /// Сколько запросов осталось.
    pub remaining: u32,
    /// Секунды до восстановления одного токена (0, если токен уже есть).
    pub reset_seconds: u64,
}

/// Плавный ограничитель частоты запросов для Launcher API (PLAN §7).
///
/// Каждый токен лаунчера имеет собственную корзину. Ёмкость — всплеск
/// `capacity`, пополнение — `refill_per_minute` токенов в минуту.
pub struct TokenBucketLimiter {
    buckets: DashMap<String, Bucket>,
    capacity: f64,
    rate_per_second: f64,
}

impl TokenBucketLimiter {
    pub fn new(capacity: u32, refill_per_minute: u32) -> Self {
        Self {
            buckets: DashMap::new(),
            capacity: capacity.max(1) as f64,
            rate_per_second: (refill_per_minute.max(1) as f64) / 60.0,
        }
    }

    /// Пытается израсходовать один токен.
    ///
    /// Возвращает `Ok(LimitSnapshot)` либо `Err(retry_after_secs)`.
    pub fn try_acquire(&self, key: &str) -> Result<LimitSnapshot, u64> {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: self.capacity,
            updated: now,
            overruns: 0,
        });
        let elapsed = now.duration_since(entry.updated).as_secs_f64();
        entry.tokens = (entry.tokens + elapsed * self.rate_per_second).min(self.capacity);
        entry.updated = now;

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            entry.overruns = 0;
            let reset_seconds = if entry.tokens >= 1.0 {
                0
            } else {
                ((1.0 - entry.tokens) / self.rate_per_second).ceil() as u64
            };
            Ok(LimitSnapshot {
                limit: self.capacity as u32,
                remaining: entry.tokens.floor().max(0.0) as u32,
                reset_seconds,
            })
        } else {
            entry.overruns = entry.overruns.saturating_add(1);
            let needed = 1.0 - entry.tokens;
            let wait = (needed / self.rate_per_second).ceil().max(1.0) as u64;
            Err(wait)
        }
    }

    /// Ёмкость корзины — для заголовков ответа об исчерпании лимита.
    pub fn limit(&self) -> u32 {
        self.capacity as u32
    }

    /// Сколько запросов подряд превысили лимит (для записи в журнал).
    pub fn overruns(&self, key: &str) -> u32 {
        self.buckets
            .get(key)
            .map(|bucket| bucket.overruns)
            .unwrap_or(0)
    }

    pub fn reset(&self, key: &str) {
        self.buckets.remove(key);
    }

    /// Удаляет корзины, которые полностью восстановились.
    pub fn prune(&self) {
        let now = Instant::now();
        let capacity = self.capacity;
        let rate = self.rate_per_second;
        self.buckets.retain(|_, bucket| {
            let tokens = (bucket.tokens + now.duration_since(bucket.updated).as_secs_f64() * rate)
                .min(capacity);
            tokens < capacity
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_limiter_blocks_after_limit() {
        let limiter = LoginLimiter::new(2, 60, 60);
        assert!(limiter.check("k").is_ok());
        limiter.record_failure("k");
        limiter.record_failure("k");
        assert!(limiter.check("k").is_err());
    }

    #[test]
    fn token_bucket_allows_burst_then_limits() {
        let limiter = TokenBucketLimiter::new(3, 60);
        assert!(limiter.try_acquire("t").is_ok());
        assert!(limiter.try_acquire("t").is_ok());
        assert!(limiter.try_acquire("t").is_ok());
        let error = limiter.try_acquire("t").expect_err("лимит исчерпан");
        assert!(error >= 1, "Retry-After не меньше секунды");
        // Другой ключ не затронут.
        assert!(limiter.try_acquire("other").is_ok());
    }

    #[test]
    fn snapshot_reports_rate_limit_headers() {
        let limiter = TokenBucketLimiter::new(3, 60);
        let first = limiter.try_acquire("headers").expect("первый запрос");
        assert_eq!(first.limit, 3);
        assert_eq!(first.remaining, 2);
        assert_eq!(first.reset_seconds, 0);
        assert_eq!(limiter.limit(), 3);
    }

    #[test]
    fn exhaustion_reports_retry_after() {
        let limiter = TokenBucketLimiter::new(1, 1);
        assert!(limiter.try_acquire("empty").is_ok());
        let retry = limiter.try_acquire("empty").expect_err("корзина пуста");
        assert!(retry >= 1);
    }

    #[test]
    fn overruns_are_counted() {
        let limiter = TokenBucketLimiter::new(1, 1);
        assert!(limiter.try_acquire("x").is_ok());
        for _ in 0..3 {
            let _ = limiter.try_acquire("x");
        }
        assert_eq!(limiter.overruns("x"), 3);
        limiter.reset("x");
        assert_eq!(limiter.overruns("x"), 0);
    }
}
