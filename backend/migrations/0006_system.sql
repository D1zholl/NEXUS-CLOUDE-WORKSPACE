-- NEXUS WORKSPACE — System Management (этап 8)
-- Фоновые задачи: отмена и владелец типа; метрики приложения.

alter table background_jobs
    add column if not exists cancellable boolean not null default false,
    add column if not exists cancel_requested boolean not null default false,
    add column if not exists updated_at timestamptz not null default now();

-- Метрики приложения собираются вместе с системными, чтобы графики были реальными.
create index if not exists system_metrics_metric_time_idx
    on system_metrics (metric, captured_at desc);

-- Журнал событий: индекс по результату для фильтра ошибок.
create index if not exists audit_events_result_idx
    on audit_events (result, created_at desc);
