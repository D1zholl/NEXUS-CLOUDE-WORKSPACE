use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Injectable wall clock.
///
/// All persisted entities use Unix time in milliseconds (UTC), matching the
/// SQLite schema (`created_at`, `updated_at`, ...).
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        system_now_ms()
    }
}

pub fn system_now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Deterministic clock for tests.
#[derive(Debug)]
pub struct FixedClock {
    now_ms: AtomicI64,
}

impl FixedClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    pub fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_is_deterministic() {
        let clock = FixedClock::new(1_795_274_960_000);
        assert_eq!(clock.now_ms(), 1_795_274_960_000);
        clock.advance(500);
        assert_eq!(clock.now_ms(), 1_795_274_960_500);
        let _: &dyn Clock = &clock;
    }

    #[test]
    fn system_clock_returns_unix_millis() {
        assert!(system_now_ms() > 1_600_000_000_000);
    }
}
