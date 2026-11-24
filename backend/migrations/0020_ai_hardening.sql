-- NEXUS AI Workspace Phase 9 hardening.
-- Добавляет: MCP registry, полные snapshot'ы версий Skills, учёт usage,
-- подтверждение отмены задач, восстановление после рестарта, network policy
-- программ и защиту от повторной доставки команд.

-- ---------------------------------------------------------------------------
-- Runtime: защита от повторной/устаревшей доставки
-- ---------------------------------------------------------------------------

alter table ai_runtime_nodes
    add column if not exists last_client_sequence bigint not null default 0;
alter table ai_runtime_nodes
    add column if not exists connected_at timestamptz;

-- Inbound/outbound receipts для восстановления неизвестных результатов.
create table if not exists ai_runtime_receipts (
    id bigserial primary key,
    node_id uuid not null references ai_runtime_nodes(id) on delete cascade,
    message_id uuid not null,
    task_id uuid,
    kind text not null,
    outcome jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    unique (node_id, message_id)
);
create index if not exists ai_runtime_receipts_task_idx
    on ai_runtime_receipts (task_id, created_at);

alter table ai_runtime_commands
    add column if not exists acknowledged_at timestamptz;
alter table ai_runtime_commands
    add column if not exists last_error text;

-- ---------------------------------------------------------------------------
-- Задачи: подтверждённая отмена и восстановление
-- ---------------------------------------------------------------------------

alter table ai_tasks
    add column if not exists cancel_status text not null default 'none';
alter table ai_tasks
    add column if not exists cancel_requested_at timestamptz;
alter table ai_tasks
    add column if not exists cancel_confirmed_at timestamptz;
alter table ai_tasks
    add column if not exists cancel_message_id uuid;
alter table ai_tasks
    add column if not exists recovery_state text;
alter table ai_tasks
    add column if not exists recovered_at timestamptz;

do $$
begin
    if not exists (
        select 1 from pg_constraint where conname = 'ai_tasks_cancel_status_chk'
    ) then
        alter table ai_tasks
            add constraint ai_tasks_cancel_status_chk
            check (cancel_status in ('none', 'requested', 'confirmed', 'failed'));
    end if;
end $$;

create index if not exists ai_tasks_cancel_idx
    on ai_tasks (cancel_status, cancel_requested_at)
    where cancel_status = 'requested';

alter table ai_tool_executions
    add column if not exists dispatch_message_id uuid;
alter table ai_tool_executions
    add column if not exists attempts integer not null default 0;
alter table ai_tool_executions
    add column if not exists receipt jsonb;

-- Уникальный dispatch message id нужен для сопоставления ack/receipt.
create unique index if not exists ai_tool_executions_dispatch_uq
    on ai_tool_executions (dispatch_message_id)
    where dispatch_message_id is not null;

-- ---------------------------------------------------------------------------
-- Workspace usage и network policy
-- ---------------------------------------------------------------------------

alter table ai_workspaces
    add column if not exists network_policy text not null default 'deny';
alter table ai_workspaces
    add column if not exists max_process_memory_bytes bigint not null default 0;
alter table ai_workspaces
    add column if not exists usage_reported_by uuid references ai_runtime_nodes(id) on delete set null;
alter table ai_workspaces
    add column if not exists usage_error text;

do $$
begin
    if not exists (
        select 1 from pg_constraint where conname = 'ai_workspaces_network_policy_chk'
    ) then
        alter table ai_workspaces
            add constraint ai_workspaces_network_policy_chk
            check (network_policy in ('deny', 'allow'));
    end if;
end $$;

create table if not exists ai_workspace_usage_samples (
    id bigserial primary key,
    workspace_id uuid not null references ai_workspaces(id) on delete cascade,
    node_id uuid references ai_runtime_nodes(id) on delete set null,
    used_bytes bigint not null default 0,
    used_files bigint not null default 0,
    source text not null default 'runtime',
    error text,
    created_at timestamptz not null default now()
);
create index if not exists ai_workspace_usage_samples_ws_idx
    on ai_workspace_usage_samples (workspace_id, created_at desc);

alter table ai_programs
    add column if not exists network_policy text not null default 'deny';
alter table ai_programs
    add column if not exists max_memory_bytes bigint not null default 0;

do $$
begin
    if not exists (
        select 1 from pg_constraint where conname = 'ai_programs_network_policy_chk'
    ) then
        alter table ai_programs
            add constraint ai_programs_network_policy_chk
            check (network_policy in ('deny', 'allow'));
    end if;
end $$;

-- ---------------------------------------------------------------------------
-- Skills: полный snapshot всех файлов каждой версии
-- ---------------------------------------------------------------------------

alter table ai_skill_versions
    add column if not exists size_bytes bigint not null default 0;
alter table ai_skill_versions
    add column if not exists file_count integer not null default 0;
alter table ai_skill_versions
    add column if not exists files_checksum text not null default '';

create table if not exists ai_skill_version_files (
    id uuid primary key default gen_random_uuid(),
    version_id uuid not null references ai_skill_versions(id) on delete cascade,
    path text not null,
    content bytea not null,
    size_bytes bigint not null check (size_bytes >= 0),
    sha256 text not null,
    unique (version_id, path)
);
create index if not exists ai_skill_version_files_version_idx
    on ai_skill_version_files (version_id, path);

-- Backfill текущей версии для уже установленных Skills: раньше snapshot
-- содержал только SKILL.md, теперь добавляем все файлы текущей версии.
insert into ai_skill_version_files (version_id, path, content, size_bytes, sha256)
select v.id, f.path, f.content, f.size_bytes, f.sha256
  from ai_skills s
  join ai_skill_versions v on v.skill_id = s.id and v.version = s.version
  join ai_skill_files f on f.skill_id = s.id
 where not exists (
       select 1 from ai_skill_version_files x
        where x.version_id = v.id and x.path = f.path
 );
update ai_skill_versions v
   set size_bytes = coalesce((select sum(size_bytes) from ai_skill_version_files f where f.version_id = v.id), 0),
       file_count = coalesce((select count(*) from ai_skill_version_files f where f.version_id = v.id), 0),
       files_checksum = coalesce((select min(checksum) from ai_skills s where s.id = v.skill_id and s.version = v.version), '')
 where file_count = 0;

-- ---------------------------------------------------------------------------
-- MCP: реестр соединений, обнаруженные tools, назначения
-- ---------------------------------------------------------------------------

create table if not exists ai_mcp_connections (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    runtime_node_id uuid not null references ai_runtime_nodes(id) on delete cascade,
    transport text not null default 'http' check (transport in ('http')),
    endpoint text not null,
    secret_cipher text,
    enabled boolean not null default false,
    allow_private_network boolean not null default false,
    status text not null default 'unknown'
        check (status in ('unknown', 'ok', 'error', 'disabled')),
    last_error text,
    last_discovered_at timestamptz,
    discovered_count integer not null default 0,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index if not exists ai_mcp_connections_name_uq
    on ai_mcp_connections (runtime_node_id, lower(name));

create table if not exists ai_mcp_tools (
    id uuid primary key default gen_random_uuid(),
    connection_id uuid not null references ai_mcp_connections(id) on delete cascade,
    tool_name text not null,
    tool_id text not null unique,
    model_name text not null unique,
    description text not null default '',
    input_schema jsonb not null default '{"type":"object","properties":{}}'::jsonb,
    risk text not null default 'high' check (risk in ('low', 'medium', 'high', 'critical')),
    enabled boolean not null default true,
    discovered_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (connection_id, tool_name)
);
create index if not exists ai_mcp_tools_connection_idx
    on ai_mcp_tools (connection_id, tool_name);

create table if not exists ai_mcp_assignments (
    id uuid primary key default gen_random_uuid(),
    connection_id uuid not null references ai_mcp_connections(id) on delete cascade,
    scope text not null check (scope in ('user', 'workspace', 'role')),
    user_id uuid references users(id) on delete cascade,
    workspace_id uuid references ai_workspaces(id) on delete cascade,
    role_id uuid references roles(id) on delete cascade,
    enabled boolean not null default true,
    assigned_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    check (
        (scope = 'user' and user_id is not null and workspace_id is null and role_id is null)
        or (scope = 'workspace' and workspace_id is not null and user_id is null and role_id is null)
        or (scope = 'role' and role_id is not null and user_id is null and workspace_id is null)
    )
);
create unique index if not exists ai_mcp_assignments_uq
    on ai_mcp_assignments (connection_id, scope,
        coalesce(user_id, '00000000-0000-0000-0000-000000000000'::uuid),
        coalesce(workspace_id, '00000000-0000-0000-0000-000000000000'::uuid),
        coalesce(role_id, '00000000-0000-0000-0000-000000000000'::uuid));
create index if not exists ai_mcp_assignments_user_idx on ai_mcp_assignments (user_id);
create index if not exists ai_mcp_assignments_workspace_idx on ai_mcp_assignments (workspace_id);

-- Политики MCP-tools хранятся в общем ai_tool_policies по tool_id.
