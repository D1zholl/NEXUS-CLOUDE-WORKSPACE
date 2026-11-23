-- NEXUS WORKSPACE — фаза C1: управление задачами (пауза и очередь).
-- Миграция добавляет поля паузы и частичный индекс для планировщика.
-- Статус `paused` уже разрешён check-ограничением background_jobs.

alter table background_jobs
    add column if not exists pause_requested boolean not null default false,
    add column if not exists paused_at timestamptz;

-- Планировщик выбирает самую старую задачу из очереди `pending`.
create index if not exists background_jobs_pending_idx
    on background_jobs (created_at)
    where status = 'pending';
