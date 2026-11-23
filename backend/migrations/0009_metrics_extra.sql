-- NEXUS WORKSPACE — Дополнительные метрики (этап 13, PLAN §9.3)
-- Соединения, активные скачивания и WebSocket хранятся в JSONB-колонке,
-- чтобы не добавлять по миграции на каждую новую метрику.

alter table system_metrics
    add column if not exists extra jsonb not null default '{}'::jsonb;
