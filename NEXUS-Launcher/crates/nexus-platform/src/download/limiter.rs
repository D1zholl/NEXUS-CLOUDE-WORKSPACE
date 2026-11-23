//! Global pacing primitives shared by every file transfer.
//!
//! * [`SpeedLimiter`] is one token bucket for the whole engine (all tasks and
//!   all files combined).
//! * [`ConcurrencyLimiter`] limits the number of file transfers in flight and
//!   can be re-tuned at runtime when settings change.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

const SLICE_BYTES: u64 = 64 * 1024;
const MIN_BUCKET_BYTES: f64 = 64.0 * 1024.0;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    capacity: f64,
    last: Instant,
}

/// Token bucket measured in bytes per second. `0` disables throttling.
#[derive(Debug)]
pub struct SpeedLimiter {
    rate_bps: AtomicU64,
    bucket: Mutex<Bucket>,
}

impl SpeedLimiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            rate_bps: AtomicU64::new(rate_bps),
            bucket: Mutex::new(Bucket {
                tokens: 0.0,
                capacity: MIN_BUCKET_BYTES,
                last: Instant::now(),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn rate_bps(&self) -> u64 {
        self.rate_bps.load(Ordering::Acquire)
    }

    pub fn set_rate_bps(&self, rate_bps: u64) {
        self.rate_bps.store(rate_bps, Ordering::Release);
        let mut bucket = lock_unpoisoned(&self.bucket);
        bucket.last = Instant::now();
        bucket.capacity = ((rate_bps as f64) * 0.25).max(MIN_BUCKET_BYTES);
        bucket.tokens = bucket.tokens.min(bucket.capacity);
    }

    /// Waits until `bytes` may be transferred at the global rate.
    ///
    /// The function is intentionally not cancel-safe: cancelling it may lose a
    /// small number of tokens. That is harmless and keeps callers simple.
    pub async fn acquire(&self, bytes: usize) {
        let mut remaining = bytes as u64;
        while remaining > 0 {
            let rate = self.rate_bps.load(Ordering::Acquire);
            if rate == 0 {
                return;
            }
            let slice = remaining.min(SLICE_BYTES);
            self.acquire_slice(slice as f64).await;
            remaining -= slice;
        }
    }

    async fn acquire_slice(&self, needed: f64) {
        loop {
            let current_rate = self.rate_bps.load(Ordering::Acquire);
            if current_rate == 0 {
                return;
            }
            let rate = current_rate as f64;

            let wait = {
                let mut bucket = lock_unpoisoned(&self.bucket);
                let now = Instant::now();
                bucket.capacity = (rate * 0.25).max(MIN_BUCKET_BYTES);
                let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
                bucket.last = now;
                bucket.tokens = (bucket.tokens + elapsed * rate).min(bucket.capacity);

                if bucket.tokens >= needed {
                    bucket.tokens -= needed;
                    return;
                }
                let deficit = needed - bucket.tokens;
                (deficit / rate).clamp(0.001, 1.0)
            };

            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }
    }
}

#[derive(Debug)]
pub struct ConcurrencyLimiter {
    active: Mutex<usize>,
    limit: AtomicUsize,
    notify: Notify,
}

impl ConcurrencyLimiter {
    pub fn new(limit: usize) -> Self {
        Self {
            active: Mutex::new(0),
            limit: AtomicUsize::new(limit.max(1)),
            notify: Notify::new(),
        }
    }

    #[allow(dead_code)]
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }

    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit.max(1), Ordering::Release);
        self.notify.notify_waiters();
    }

    #[cfg(test)]
    pub fn active(&self) -> usize {
        *lock_unpoisoned(&self.active)
    }

    /// Waits for a free slot. The permit releases the slot on drop.
    pub async fn acquire(&self) -> ConcurrencyPermit<'_> {
        loop {
            {
                let mut active = lock_unpoisoned(&self.active);
                if *active < self.limit.load(Ordering::Acquire) {
                    *active += 1;
                    return ConcurrencyPermit { limiter: self };
                }
            }
            self.notify.notified().await;
        }
    }
}

#[derive(Debug)]
pub struct ConcurrencyPermit<'a> {
    limiter: &'a ConcurrencyLimiter,
}

impl Drop for ConcurrencyPermit<'_> {
    fn drop(&mut self) {
        let mut active = lock_unpoisoned(&self.limiter.active);
        *active = active.saturating_sub(1);
        drop(active);
        self.limiter.notify.notify_one();
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_limiter_returns_immediately() {
        let limiter = SpeedLimiter::new(0);
        let start = Instant::now();
        limiter.acquire(10 * 1024 * 1024).await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn concurrency_limit_is_enforced_and_released() {
        let limiter = ConcurrencyLimiter::new(1);
        let first = limiter.acquire().await;
        assert_eq!(limiter.active(), 1);
        drop(first);
        assert_eq!(limiter.active(), 0);
        let _second = limiter.acquire().await;
        assert_eq!(limiter.active(), 1);
    }
}
