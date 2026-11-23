//! Счётчики запросов API: RPS, задержка, ошибки.
//!
//! Хранятся в памяти процесса в виде секундных окон. Это реальные измерения
//! middleware, а не декоративные значения.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default)]
struct Bucket {
    epoch_sec: u64,
    requests: u64,
    errors: u64,
    latency_micros: u64,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestStats {
    pub requests_per_second: f64,
    pub average_latency_ms: f64,
    pub error_rate: f64,
    pub requests_last_minute: u64,
    pub errors_last_minute: u64,
}

#[derive(Clone)]
pub struct RequestMetrics {
    inner: Arc<Mutex<VecDeque<Bucket>>>,
    in_flight: Arc<std::sync::atomic::AtomicU64>,
    downloads: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for RequestMetrics {
    fn default() -> Self {
        Self::new()
    }
}

fn epoch_sec(now: Instant, origin: Instant) -> u64 {
    now.duration_since(origin).as_secs()
}

impl RequestMetrics {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(65))),
            in_flight: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            downloads: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Отмечает начало запроса в обработке.
    pub fn begin_request(&self) {
        self.in_flight
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Отмечает завершение запроса.
    pub fn end_request(&self) {
        self.in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Текущее число активных HTTP-запросов.
    pub fn active_connections(&self) -> u64 {
        self.in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Отмечает начало потоковой отдачи файла.
    pub fn begin_download(&self) {
        self.downloads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Отмечает завершение потоковой отдачи файла.
    pub fn end_download(&self) {
        self.downloads
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Текущее число активных скачиваний.
    pub fn active_downloads(&self) -> u64 {
        self.downloads.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Счётчик для обёртки потока: уменьшается автоматически при завершении.
    pub fn download_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.downloads.clone()
    }

    pub fn record(&self, latency: Duration, is_error: bool) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let epoch = now_second();
        match guard.back_mut() {
            Some(bucket) if bucket.epoch_sec == epoch => {
                bucket.requests += 1;
                bucket.latency_micros += latency.as_micros() as u64;
                if is_error {
                    bucket.errors += 1;
                }
            }
            _ => {
                guard.push_back(Bucket {
                    epoch_sec: epoch,
                    requests: 1,
                    errors: u64::from(is_error),
                    latency_micros: latency.as_micros() as u64,
                });
                let cutoff = epoch.saturating_sub(WINDOW.as_secs());
                while guard.front().map(|b| b.epoch_sec < cutoff).unwrap_or(false) {
                    guard.pop_front();
                }
            }
        }
    }

    pub fn snapshot(&self) -> RequestStats {
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let epoch = now_second();
        let cutoff = epoch.saturating_sub(WINDOW.as_secs());
        let mut requests = 0u64;
        let mut errors = 0u64;
        let mut latency = 0u64;
        let mut seconds = 0u64;
        for bucket in guard.iter().filter(|b| b.epoch_sec >= cutoff) {
            requests += bucket.requests;
            errors += bucket.errors;
            latency += bucket.latency_micros;
            seconds += 1;
        }
        let span = seconds.max(1) as f64;
        RequestStats {
            requests_per_second: requests as f64 / span,
            average_latency_ms: if requests > 0 {
                latency as f64 / requests as f64 / 1000.0
            } else {
                0.0
            },
            error_rate: if requests > 0 {
                errors as f64 / requests as f64
            } else {
                0.0
            },
            requests_last_minute: requests,
            errors_last_minute: errors,
        }
    }
}

/// Секунды с произвольной эпохи процесса (монотонные).
fn now_second() -> u64 {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = ORIGIN.get_or_init(Instant::now);
    epoch_sec(Instant::now(), *origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_reflect_recorded_requests() {
        let metrics = RequestMetrics::new();
        metrics.record(Duration::from_millis(10), false);
        metrics.record(Duration::from_millis(30), true);
        let stats = metrics.snapshot();
        assert_eq!(stats.requests_last_minute, 2);
        assert_eq!(stats.errors_last_minute, 1);
        assert!((stats.average_latency_ms - 20.0).abs() < 0.001);
        assert!((stats.error_rate - 0.5).abs() < 0.001);
    }
}
