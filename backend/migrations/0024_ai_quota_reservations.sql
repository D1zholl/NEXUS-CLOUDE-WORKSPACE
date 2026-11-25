-- NEXUS 1.1: транзакционное резервирование квот AI workspace.
--
-- Использование из ai_workspaces.usage_* остаётся последним измеренным
-- значением. Активные резервы прибавляются к нему при проверке, поэтому
-- параллельные операции (в том числе с разных Runtime/соединений) не могут
-- одновременно пройти проверку, опираясь на одно и то же устаревшее значение.

create table if not exists ai_quota_reservations (
    id uuid primary key default gen_random_uuid(),
    operation_id text not null unique,
    user_id uuid not null references users(id) on delete cascade,
    workspace_id uuid not null references ai_workspaces(id) on delete cascade,
    kind text not null,
    bytes bigint not null default 0 check (bytes >= 0),
    files integer not null default 0 check (files >= 0),
    status text not null default 'active'
        check (status in ('active', 'confirmed', 'released', 'expired', 'uncertain')),
    metadata jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null,
    resolved_at timestamptz
);

create index if not exists ai_quota_reservations_active_user_idx
    on ai_quota_reservations (user_id, expires_at)
    where status in ('active', 'uncertain');
create index if not exists ai_quota_reservations_active_ws_idx
    on ai_quota_reservations (workspace_id, expires_at)
    where status in ('active', 'uncertain');
create index if not exists ai_quota_reservations_status_idx
    on ai_quota_reservations (status, expires_at);

-- Журнал решений по квотам для диагностики и аудита. Не содержит содержимого
-- файлов и секретов.
create table if not exists ai_quota_events (
    id bigserial primary key,
    reservation_id uuid,
    workspace_id uuid,
    user_id uuid,
    event text not null,
    requested_bytes bigint not null default 0,
    requested_files integer not null default 0,
    used_bytes bigint not null default 0,
    used_files bigint not null default 0,
    limit_bytes bigint not null default 0,
    limit_files integer not null default 0,
    error text,
    created_at timestamptz not null default now()
);

create index if not exists ai_quota_events_ws_idx
    on ai_quota_events (workspace_id, created_at desc);
