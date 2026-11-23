//! Планировщик фоновых задач.
//!
//! Единственный потребитель очереди `background_jobs`: раз в 1–2 секунды
//! атомарно захватывает самую старую задачу `pending` через
//! `FOR UPDATE SKIP LOCKED` и запускает её выполнение в отдельной задаче.
//! Одновременно выполняется не более `jobs.max_concurrent` задач (fallback 2).
//!
//! При старте сервера незавершённые `running` возвращаются в `pending`
//! с причиной в `error`. При остановке новые задачи не берутся, а текущие
//! получают `cancel_requested` — обработчик завершается между единицами работы.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

/// Период опроса очереди. План требует цикл каждые 1–2 секунды.
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// Значение лимита, если `jobs.max_concurrent` недоступно.
const DEFAULT_MAX_CONCURRENT: usize = 2;
/// Причина, которая остаётся в `error` после аварийного перезапуска.
const RECOVERY_REASON: &str = "Задача прервана перезапуском сервера и возвращена в очередь.";
/// Периодические задачи обслуживания ставятся в очередь не чаще раза в час.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Задачи, которые планировщик запускает сам, без действия администратора.
///
/// `system.idempotency.cleanup` удаляет ключи старше 24 часов (E8),
/// `system.trash.cleanup` — записи корзины по TTL (E4). Остальные
/// обслуживают временные загрузки, ссылки и метрики.
const SCHEDULED_JOBS: [(&str, &str); 5] = [
    (
        "system.idempotency.cleanup",
        "Очистка устаревших ключей идемпотентности",
    ),
    ("system.uploads.cleanup", "Очистка временных загрузок"),
    ("system.shares.cleanup", "Удаление истёкших ссылок"),
    ("system.trash.cleanup", "Автоочистка корзины"),
    ("system.metrics.prune", "Очистка старых метрик"),
];

/// Разделяемый дескриптор планировщика: позволяет остановить цикл и
/// дождаться завершения текущих задач при graceful shutdown.
#[derive(Clone)]
pub struct SchedulerHandle {
    shutdown: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
}

impl SchedulerHandle {
    /// Запрещает захват новых задач. Запущенный цикл увидит флаг не позднее
    /// чем через `POLL_INTERVAL`.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Останавливает планировщик и помечает текущие задачи как отменяемые.
    pub async fn shutdown(&self, db: &PgPool) {
        self.request_shutdown();
        let result = sqlx::query(
            "update background_jobs
                set cancel_requested = true, updated_at = now()
              where status = 'running'",
        )
        .execute(db)
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "не удалось запросить отмену фоновых задач");
        }
    }

    /// Число задач, выполняющихся прямо сейчас.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Ждёт завершения активных задач не дольше `timeout`.
    /// Возвращает `false`, если задачи не успели завершиться.
    pub async fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while self.active() > 0 {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }
}

/// Запускает планировщик. Перед первым захватом задачи восстанавливает
/// прерванные `running`.
pub fn spawn(state: Arc<AppState>) -> SchedulerHandle {
    let handle = SchedulerHandle {
        shutdown: Arc::new(AtomicBool::new(false)),
        active: Arc::new(AtomicUsize::new(0)),
    };
    let loop_handle = handle.clone();
    tokio::spawn(async move {
        let recovered = recover_interrupted(&state.db).await;
        if recovered > 0 {
            tracing::warn!(
                recovered,
                "задачи, прерванные прошлым запуском, возвращены в очередь"
            );
        }
        run(state, loop_handle).await;
    });
    handle
}

/// Возвращает `running` в `pending` с причиной в `error`.
/// Вызывается при старте до первого захвата задач.
pub async fn recover_interrupted(db: &PgPool) -> u64 {
    match sqlx::query(
        "update background_jobs
            set status = 'pending', error = $1, updated_at = now()
          where status = 'running'",
    )
    .bind(RECOVERY_REASON)
    .execute(db)
    .await
    {
        Ok(result) => result.rows_affected(),
        Err(error) => {
            tracing::warn!(%error, "не удалось восстановить прерванные фоновые задачи");
            0
        }
    }
}

/// Ставит периодические задачи обслуживания, если такой задачи ещё нет в
/// очереди и она не завершалась за последний час.
///
/// Возвращает число созданных задач. Функция публичная, чтобы тесты могли
/// проверить расписание без запуска бесконечного цикла.
pub async fn enqueue_scheduled_jobs(db: &PgPool) -> u64 {
    let mut created = 0u64;
    for (job_type, label) in SCHEDULED_JOBS {
        let result = sqlx::query(
            "insert into background_jobs (job_type, status, payload, cancellable)
             select $1, 'pending', $2::jsonb, true
             where not exists (
                 select 1 from background_jobs
                 where job_type = $1
                   and (
                       status in ('pending', 'running')
                       or (status in ('completed', 'failed')
                           and finished_at > now() - interval '1 hour')
                   )
             )",
        )
        .bind(job_type)
        .bind(serde_json::json!({ "kind": job_type, "label": label, "scheduled": true }))
        .execute(db)
        .await;
        match result {
            Ok(outcome) => created += outcome.rows_affected(),
            Err(error) => {
                tracing::warn!(%job_type, %error, "не удалось поставить задачу обслуживания")
            }
        }
    }
    created
}

async fn run(state: Arc<AppState>, handle: SchedulerHandle) {
    let scheduled = enqueue_scheduled_jobs(&state.db).await;
    if scheduled > 0 {
        tracing::info!(
            scheduled,
            "периодические задачи обслуживания поставлены в очередь"
        );
    }
    let mut next_maintenance = tokio::time::Instant::now() + MAINTENANCE_INTERVAL;

    loop {
        if handle.shutdown.load(Ordering::SeqCst) {
            break;
        }

        if tokio::time::Instant::now() >= next_maintenance {
            let created = enqueue_scheduled_jobs(&state.db).await;
            if created > 0 {
                tracing::info!(created, "очередь обслуживания пополнена");
            }
            next_maintenance = tokio::time::Instant::now() + MAINTENANCE_INTERVAL;
        }

        let limit = match state.jobs_max_concurrent().await {
            0 => DEFAULT_MAX_CONCURRENT,
            value => value,
        };
        // Внутренний цикл заполняет свободные слоты без ожидания.
        loop {
            if handle.shutdown.load(Ordering::SeqCst) {
                break;
            }
            if handle.active.load(Ordering::SeqCst) >= limit {
                break;
            }
            match claim_next(&state.db).await {
                Ok(Some(job_id)) => {
                    if handle.shutdown.load(Ordering::SeqCst) {
                        // Остановка пришла между проверкой и захватом:
                        // возвращаем задачу в очередь, не запуская её.
                        release(&state.db, job_id).await;
                        break;
                    }
                    handle.active.fetch_add(1, Ordering::SeqCst);
                    let task_state = state.clone();
                    let active = handle.active.clone();
                    tokio::spawn(async move {
                        crate::system::jobs::execute(&task_state, job_id).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "не удалось захватить фоновую задачу");
                    break;
                }
            }
        }

        if handle.shutdown.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Новые задачи больше не берутся; текущим предлагаем завершиться.
    if let Err(error) = sqlx::query(
        "update background_jobs
            set cancel_requested = true, updated_at = now()
          where status = 'running'",
    )
    .execute(&state.db)
    .await
    {
        tracing::warn!(%error, "не удалось запросить отмену задач при остановке");
    }
    tracing::info!(
        active = handle.active(),
        "планировщик остановлен, новые задачи не захватываются"
    );
}

/// Атомарный захват самой старой задачи из очереди.
///
/// SQL совпадает с планом C2: `FOR UPDATE SKIP LOCKED` внутри подзапроса
/// не блокирует другие экземпляры планировщика. Публичная функция также
/// используется тестами для проверки конкурентного захвата.
pub async fn claim_next(db: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "update background_jobs
            set status = 'running', started_at = coalesce(started_at, now()), updated_at = now()
          where id = (
              select id from background_jobs
               where status = 'pending'
               order by created_at
               for update skip locked
               limit 1
          )
          returning id",
    )
    .fetch_optional(db)
    .await
}

/// Возвращает захваченную задачу в очередь (используется при гонке с shutdown).
async fn release(db: &PgPool, job_id: Uuid) {
    let _ = sqlx::query(
        "update background_jobs
            set status = 'pending', updated_at = now()
          where id = $1 and status = 'running'",
    )
    .bind(job_id)
    .execute(db)
    .await;
}
