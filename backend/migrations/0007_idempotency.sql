-- NEXUS WORKSPACE — Идемпотентность опасных операций (этап 11, PLAN §8.7)
-- Повторный запрос с тем же Idempotency-Key не меняет состояние и возвращает
-- сохранённый ответ. Ключ привязан к пользователю и области операции.

create table idempotency_keys (
    key text not null,
    scope text not null,
    actor_id uuid not null references users(id) on delete cascade,
    request_hash text not null,
    status text not null default 'in_progress'
        check (status in ('in_progress', 'completed')),
    response_status integer,
    response_body jsonb,
    created_at timestamptz not null default now(),
    completed_at timestamptz,
    primary key (actor_id, scope, key)
);
create index idempotency_keys_created_idx on idempotency_keys (created_at);
