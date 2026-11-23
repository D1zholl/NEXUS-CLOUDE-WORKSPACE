//! Параллельные скачивания артефактов: семафор на токен и учёт активных потоков.
//!
//! Частотный лимит (`TokenBucketLimiter`) ограничивает число запросов, но не
//! защищает диск и сеть от десятков одновременных потоков. Здесь на каждый
//! токен лаунчера выделяется семафор: permit удерживается обёрткой тела ответа
//! и освобождается при завершении потока или обрыве клиента (`Drop`).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::body::BodyDataStream;
use bytes::Bytes;
use dashmap::DashMap;
use futures_core::Stream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::system::requests::RequestMetrics;

/// `Retry-After` при исчерпании параллельных слотов.
///
/// Точное время освобождения слота неизвестно: оно зависит от размера файла и
/// скорости клиента. Клиенту достаточно рекомендации повторить запрос позже.
pub const DOWNLOAD_RETRY_AFTER_SECS: u64 = 5;

struct Inner {
    max_parallel: usize,
    semaphores: Mutex<HashMap<Uuid, Arc<Semaphore>>>,
    active: DashMap<Uuid, u64>,
    total_active: AtomicU64,
    metrics: RequestMetrics,
}

/// Лимитер параллельных скачиваний. Дешёво клонируется (внутри `Arc`).
#[derive(Clone)]
pub struct DownloadLimiter {
    inner: Arc<Inner>,
}

impl DownloadLimiter {
    pub fn new(max_parallel: usize, metrics: RequestMetrics) -> Self {
        Self {
            inner: Arc::new(Inner {
                max_parallel: max_parallel.max(1),
                semaphores: Mutex::new(HashMap::new()),
                active: DashMap::new(),
                total_active: AtomicU64::new(0),
                metrics,
            }),
        }
    }

    pub fn max_parallel(&self) -> usize {
        self.inner.max_parallel
    }

    /// Пытается занять слот. `Err` содержит `Retry-After` в секундах.
    ///
    /// Проверка неблокирующая: ожидающие запросы не копятся в очереди, клиент
    /// получает `503` и повторяет попытку сам.
    pub fn acquire(&self, token_id: Uuid) -> Result<DownloadSlot, u64> {
        let semaphore = {
            let mut guard = self
                .inner
                .semaphores
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .entry(token_id)
                .or_insert_with(|| Arc::new(Semaphore::new(self.inner.max_parallel)))
                .clone()
        };
        let permit = semaphore
            .try_acquire_owned()
            .map_err(|_| DOWNLOAD_RETRY_AFTER_SECS)?;

        *self.inner.active.entry(token_id).or_insert(0) += 1;
        self.inner.total_active.fetch_add(1, Ordering::Relaxed);
        self.inner.metrics.begin_download();

        Ok(DownloadSlot {
            inner: self.inner.clone(),
            token_id,
            _permit: permit,
        })
    }

    /// Активные скачивания по токену.
    pub fn active(&self, token_id: Uuid) -> u64 {
        self.inner
            .active
            .get(&token_id)
            .map(|value| *value)
            .unwrap_or(0)
    }

    /// Активные скачивания всех токенов.
    pub fn total_active(&self) -> u64 {
        self.inner.total_active.load(Ordering::Relaxed)
    }
}

/// Занятый слот. Освобождается при `Drop`, в том числе если клиент оборвал
/// соединение и тело ответа было отброшено до конца потока.
pub struct DownloadSlot {
    inner: Arc<Inner>,
    token_id: Uuid,
    _permit: OwnedSemaphorePermit,
}

impl Drop for DownloadSlot {
    fn drop(&mut self) {
        {
            if let Some(mut entry) = self.inner.active.get_mut(&self.token_id) {
                *entry = entry.saturating_sub(1);
            }
        }
        self.inner
            .active
            .remove_if(&self.token_id, |_, count| *count == 0);
        self.inner.total_active.fetch_sub(1, Ordering::Relaxed);
        self.inner.metrics.end_download();
    }
}

/// Поток тела ответа, удерживающий слот скачивания до конца передачи.
pub struct PermitStream {
    inner: BodyDataStream,
    _slot: DownloadSlot,
}

impl PermitStream {
    pub fn new(body: axum::body::Body, slot: DownloadSlot) -> Self {
        Self {
            inner: body.into_data_stream(),
            _slot: slot,
        }
    }
}

impl Stream for PermitStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slot_is_released_on_drop() {
        let limiter = DownloadLimiter::new(1, RequestMetrics::new());
        let token = Uuid::new_v4();
        let slot = limiter.acquire(token).expect("первый слот");
        assert_eq!(limiter.active(token), 1);
        assert_eq!(limiter.total_active(), 1);
        assert!(limiter.acquire(token).is_err(), "второй слот занят");

        drop(slot);
        assert_eq!(limiter.active(token), 0);
        assert_eq!(limiter.total_active(), 0);
        assert!(limiter.acquire(token).is_ok(), "слот освободился");
    }

    #[tokio::test]
    async fn tokens_are_isolated() {
        let limiter = DownloadLimiter::new(1, RequestMetrics::new());
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let _held = limiter.acquire(first).expect("слот первого токена");
        assert!(limiter.acquire(first).is_err());
        assert!(
            limiter.acquire(second).is_ok(),
            "лимит другого токена независим"
        );
    }
}
