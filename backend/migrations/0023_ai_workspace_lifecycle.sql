-- NEXUS 1.1: полный жизненный цикл AI Workspace.
--
-- Мягкое удаление остаётся в ai_workspaces.deleted_at. Отдельная таблица
-- хранит срок хранения, очередь физической очистки и историю. Она не удаляет
-- строку ai_workspaces: история задач и аудит сохраняются, а snapshots и
-- usage-семплы очищаются после подтверждённого физического удаления.

create table if not exists ai_workspace_lifecycle (
    workspace_id uuid primary key,
    owner_id uuid not null references users(id) on delete cascade,
    runtime_node_id uuid not null,
    original_name text not null,
    original_description text not null default '',
    state text not null default 'soft_deleted'
        check (state in ('soft_deleted', 'pending', 'running', 'retry',
                         'blocked', 'purged', 'restored')),
    requested_by uuid references users(id) on delete set null,
    requested_at timestamptz not null default now(),
    purge_after timestamptz not null,
    attempts integer not null default 0 check (attempts >= 0),
    last_attempt_at timestamptz,
    lease_until timestamptz,
    last_error text,
    purged_at timestamptz,
    restored_at timestamptz,
    bytes_freed bigint not null default 0,
    files_removed bigint not null default 0,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index if not exists ai_workspace_lifecycle_due_idx
    on ai_workspace_lifecycle (purge_after)
    where state in ('soft_deleted', 'pending', 'retry');

create index if not exists ai_workspace_lifecycle_state_idx
    on ai_workspace_lifecycle (state, purge_after);

-- Аудит физической очистки: отдельный журнал, потому что audit_events может
-- иметь собственную политику хранения, а отчёт об очистке нужен администратору.
create table if not exists ai_workspace_purge_events (
    id bigserial primary key,
    workspace_id uuid not null,
    runtime_node_id uuid,
    event text not null,
    attempt integer not null default 0,
    bytes_freed bigint not null default 0,
    files_removed bigint not null default 0,
    error text,
    created_at timestamptz not null default now()
);

create index if not exists ai_workspace_purge_events_ws_idx
    on ai_workspace_purge_events (workspace_id, created_at desc);
