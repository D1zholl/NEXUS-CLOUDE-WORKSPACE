-- NEXUS AI Workspace — доменная модель (AI Chat, AI Agent, Runtime, Skills, Approvals).
-- Все секреты хранятся только как hash/verifier или зашифрованный ciphertext.

insert into module_states (key, enabled) values ('ai', true)
on conflict (key) do nothing;

insert into permissions (key, module, label) values
    ('ai.access', 'ai', 'Доступ к AI Workspace'),
    ('ai.chat.use', 'ai', 'Использование AI Chat'),
    ('ai.agent.use', 'ai', 'Запуск AI Agent задач'),
    ('ai.workspace.read', 'ai', 'Чтение рабочего пространства AI'),
    ('ai.workspace.write', 'ai', 'Изменение рабочего пространства AI'),
    ('ai.skills.use', 'ai', 'Использование Skills'),
    ('ai.tools.use', 'ai', 'Использование инструментов Agent'),
    ('ai.tasks.cancel', 'ai', 'Отмена AI задач'),
    ('ai.actions.approve', 'ai', 'Подтверждение опасных действий'),
    ('ai.admin.manage', 'ai', 'Администрирование AI')
on conflict (key) do nothing;

insert into role_permissions (role_id, permission_key)
select '00000000-0000-0000-0000-000000000001', key
  from permissions where module = 'ai'
on conflict do nothing;

insert into role_permissions (role_id, permission_key) values
    ('00000000-0000-0000-0000-000000000002', 'ai.access'),
    ('00000000-0000-0000-0000-000000000002', 'ai.chat.use'),
    ('00000000-0000-0000-0000-000000000002', 'ai.agent.use'),
    ('00000000-0000-0000-0000-000000000002', 'ai.workspace.read'),
    ('00000000-0000-0000-0000-000000000002', 'ai.workspace.write'),
    ('00000000-0000-0000-0000-000000000002', 'ai.skills.use'),
    ('00000000-0000-0000-0000-000000000002', 'ai.tools.use'),
    ('00000000-0000-0000-0000-000000000002', 'ai.tasks.cancel'),
    ('00000000-0000-0000-0000-000000000002', 'ai.actions.approve')
on conflict do nothing;

-- ---------------------------------------------------------------------------
-- Runtime nodes и enrollment
-- ---------------------------------------------------------------------------

create table ai_runtime_nodes (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    platform text not null default '',
    runtime_version text not null default '',
    protocol_version integer not null default 1,
    status text not null default 'offline'
        check (status in ('online', 'offline', 'degraded', 'disabled', 'revoked')),
    token_hash text not null unique,
    token_rotated_at timestamptz,
    capabilities jsonb not null default '{}'::jsonb,
    last_seen_at timestamptz,
    last_heartbeat_at timestamptz,
    last_server_sequence bigint not null default 0,
    registered_at timestamptz not null default now(),
    revoked_at timestamptz,
    created_by uuid references users(id) on delete set null,
    metadata jsonb not null default '{}'::jsonb
);
create index ai_runtime_nodes_status_idx on ai_runtime_nodes (status);
create index ai_runtime_nodes_seen_idx on ai_runtime_nodes (last_seen_at desc);

create table ai_runtime_enrollment_codes (
    id uuid primary key default gen_random_uuid(),
    code_hash text not null unique,
    label text not null default '',
    scopes text[] not null default '{}',
    expires_at timestamptz not null,
    used_at timestamptz,
    used_by_node uuid references ai_runtime_nodes(id) on delete set null,
    revoked_at timestamptz,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now()
);
create index ai_runtime_enrollment_expiry_idx on ai_runtime_enrollment_codes (expires_at);

create table ai_runtime_commands (
    id uuid primary key default gen_random_uuid(),
    node_id uuid not null references ai_runtime_nodes(id) on delete cascade,
    message_id uuid not null unique,
    sequence bigint not null,
    kind text not null,
    correlation_id uuid,
    task_id uuid,
    payload jsonb not null default '{}'::jsonb,
    status text not null default 'pending'
        check (status in ('pending', 'sent', 'completed', 'failed', 'cancelled')),
    attempts integer not null default 0,
    created_at timestamptz not null default now(),
    sent_at timestamptz,
    completed_at timestamptz,
    error text
);
create index ai_runtime_commands_node_status_idx
    on ai_runtime_commands (node_id, status, created_at);
create index ai_runtime_commands_task_idx on ai_runtime_commands (task_id);

create table ai_runtime_events (
    id bigserial primary key,
    node_id uuid references ai_runtime_nodes(id) on delete cascade,
    kind text not null,
    message text not null default '',
    data jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);
create index ai_runtime_events_node_idx on ai_runtime_events (node_id, created_at desc);

-- ---------------------------------------------------------------------------
-- Providers
-- ---------------------------------------------------------------------------

create table ai_provider_configs (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    provider_type text not null default 'openai-compatible'
        check (provider_type in ('openai-compatible')),
    base_url text not null,
    api_key_cipher text,
    default_model text not null default '',
    generation jsonb not null default '{}'::jsonb,
    limits jsonb not null default '{}'::jsonb,
    runtime_node_id uuid references ai_runtime_nodes(id) on delete set null,
    enabled boolean not null default true,
    status text not null default 'unknown'
        check (status in ('unknown', 'ok', 'error', 'unavailable')),
    last_checked_at timestamptz,
    last_error text,
    last_models jsonb not null default '[]'::jsonb,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index ai_provider_configs_name_uq on ai_provider_configs (lower(name));

-- ---------------------------------------------------------------------------
-- Workspaces и квоты пользователей
-- ---------------------------------------------------------------------------

create table ai_workspaces (
    id uuid primary key default gen_random_uuid(),
    owner_id uuid not null references users(id) on delete cascade,
    runtime_node_id uuid not null references ai_runtime_nodes(id) on delete restrict,
    name text not null,
    root_path text not null,
    quota_bytes bigint not null default 1073741824 check (quota_bytes >= 0),
    max_files integer not null default 10000 check (max_files >= 0),
    max_file_bytes bigint not null default 16777216 check (max_file_bytes >= 0),
    max_parallel_tasks integer not null default 1 check (max_parallel_tasks >= 0),
    max_task_seconds integer not null default 900 check (max_task_seconds > 0),
    allowed_tools text[] not null default '{}',
    allowed_programs text[] not null default '{}',
    status text not null default 'active' check (status in ('active', 'suspended')),
    usage_bytes bigint not null default 0,
    usage_files bigint not null default 0,
    usage_checked_at timestamptz,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index ai_workspaces_owner_idx on ai_workspaces (owner_id);
create index ai_workspaces_node_idx on ai_workspaces (runtime_node_id);
create unique index ai_workspaces_owner_name_uq on ai_workspaces (owner_id, lower(name));

create table ai_user_quotas (
    user_id uuid primary key references users(id) on delete cascade,
    max_concurrent_tasks integer not null default 1 check (max_concurrent_tasks >= 0),
    max_tasks_per_day integer not null default 50 check (max_tasks_per_day >= 0),
    max_tool_calls_per_task integer not null default 40 check (max_tool_calls_per_task >= 0),
    max_task_seconds integer not null default 900 check (max_task_seconds > 0),
    max_storage_bytes bigint not null default 1073741824,
    max_files integer not null default 10000,
    max_file_bytes bigint not null default 16777216,
    updated_by uuid references users(id) on delete set null,
    updated_at timestamptz not null default now()
);

-- ---------------------------------------------------------------------------
-- Диалоги, сообщения, вложения
-- ---------------------------------------------------------------------------

create table ai_conversations (
    id uuid primary key default gen_random_uuid(),
    owner_id uuid not null references users(id) on delete cascade,
    title text not null default 'Новый диалог',
    mode text not null default 'chat' check (mode in ('chat', 'agent')),
    provider_id uuid references ai_provider_configs(id) on delete set null,
    model text not null default '',
    workspace_id uuid references ai_workspaces(id) on delete set null,
    runtime_node_id uuid references ai_runtime_nodes(id) on delete set null,
    state text not null default 'active' check (state in ('active', 'archived')),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    last_message_at timestamptz,
    deleted_at timestamptz
);
create index ai_conversations_owner_idx on ai_conversations (owner_id, updated_at desc);
create index ai_conversations_owner_state_idx on ai_conversations (owner_id, state);

create table ai_messages (
    id uuid primary key default gen_random_uuid(),
    conversation_id uuid not null references ai_conversations(id) on delete cascade,
    sequence integer not null,
    role text not null check (role in ('user', 'assistant', 'tool', 'system')),
    content text not null default '',
    parts jsonb not null default '[]'::jsonb,
    model text not null default '',
    status text not null default 'completed'
        check (status in ('streaming', 'completed', 'failed', 'cancelled')),
    usage jsonb not null default '{}'::jsonb,
    tool_calls jsonb not null default '[]'::jsonb,
    tool_call_id text,
    error_code text,
    error_message text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    deleted_at timestamptz
);
create unique index ai_messages_sequence_uq on ai_messages (conversation_id, sequence);
create index ai_messages_conversation_idx on ai_messages (conversation_id, sequence);
create index ai_messages_search_idx
    on ai_messages using gin (to_tsvector('simple', content));

create table ai_attachments (
    id uuid primary key default gen_random_uuid(),
    conversation_id uuid not null references ai_conversations(id) on delete cascade,
    message_id uuid references ai_messages(id) on delete set null,
    owner_id uuid not null references users(id) on delete cascade,
    name text not null,
    mime text not null default 'text/plain',
    size_bytes bigint not null check (size_bytes >= 0),
    sha256 text not null,
    content bytea not null,
    created_at timestamptz not null default now()
);
create index ai_attachments_conversation_idx on ai_attachments (conversation_id);
create index ai_attachments_message_idx on ai_attachments (message_id);

-- ---------------------------------------------------------------------------
-- Задачи, события, исполнения, approvals
-- ---------------------------------------------------------------------------

create table ai_tasks (
    id uuid primary key default gen_random_uuid(),
    conversation_id uuid not null references ai_conversations(id) on delete cascade,
    owner_id uuid not null references users(id) on delete cascade,
    runtime_node_id uuid not null references ai_runtime_nodes(id) on delete restrict,
    provider_id uuid references ai_provider_configs(id) on delete set null,
    model text not null default '',
    workspace_id uuid references ai_workspaces(id) on delete set null,
    status text not null default 'queued'
        check (status in ('queued', 'running', 'waiting_approval', 'completed', 'failed', 'cancelled')),
    stage text not null default 'queued',
    status_text text not null default '',
    progress double precision not null default 0,
    iterations integer not null default 0,
    max_iterations integer not null default 10,
    tool_calls_count integer not null default 0,
    timeout_seconds integer not null default 900,
    human_input text not null default '',
    context jsonb not null default '{}'::jsonb,
    cancel_requested boolean not null default false,
    cancelled_at timestamptz,
    started_at timestamptz,
    finished_at timestamptz,
    error_code text,
    error_message text,
    idempotency_key text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index ai_tasks_owner_idx on ai_tasks (owner_id, created_at desc);
create index ai_tasks_status_idx on ai_tasks (status, created_at);
create index ai_tasks_conversation_idx on ai_tasks (conversation_id, created_at desc);
create unique index ai_tasks_owner_idem_uq
    on ai_tasks (owner_id, idempotency_key)
    where idempotency_key is not null;

create table ai_task_events (
    id bigserial primary key,
    task_id uuid not null references ai_tasks(id) on delete cascade,
    kind text not null,
    stage text not null default '',
    message text not null default '',
    data jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);
create index ai_task_events_task_idx on ai_task_events (task_id, id);

create table ai_tool_executions (
    id uuid primary key default gen_random_uuid(),
    task_id uuid not null references ai_tasks(id) on delete cascade,
    tool_call_id text not null,
    tool_id text not null,
    tool_version integer not null default 1,
    arguments jsonb not null default '{}'::jsonb,
    args_hash text not null,
    risk text not null default 'low',
    approval_id uuid,
    status text not null default 'pending'
        check (status in ('pending', 'waiting_approval', 'running', 'completed',
                          'failed', 'cancelled', 'blocked')),
    result jsonb,
    result_text text,
    error_code text,
    error_message text,
    exit_code integer,
    duration_ms bigint,
    idempotency_key text not null,
    started_at timestamptz,
    finished_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index ai_tool_executions_task_call_uq
    on ai_tool_executions (task_id, tool_call_id);
create index ai_tool_executions_task_idx on ai_tool_executions (task_id, created_at);
create index ai_tool_executions_status_idx on ai_tool_executions (status);

create table ai_tool_approvals (
    id uuid primary key default gen_random_uuid(),
    task_id uuid not null references ai_tasks(id) on delete cascade,
    execution_id uuid not null references ai_tool_executions(id) on delete cascade,
    requester_id uuid not null references users(id) on delete cascade,
    approver_id uuid references users(id) on delete set null,
    tool_id text not null,
    args_hash text not null,
    arguments jsonb not null default '{}'::jsonb,
    decision text not null default 'pending'
        check (decision in ('pending', 'approved', 'rejected', 'expired')),
    reason text not null default '',
    expires_at timestamptz not null,
    decided_at timestamptz,
    consumed_at timestamptz,
    created_at timestamptz not null default now()
);
create unique index ai_tool_approvals_execution_uq on ai_tool_approvals (execution_id);
create index ai_tool_approvals_pending_idx
    on ai_tool_approvals (requester_id, decision, expires_at);

-- ---------------------------------------------------------------------------
-- Skills
-- ---------------------------------------------------------------------------

create table ai_skills (
    id uuid primary key default gen_random_uuid(),
    slug text not null,
    name text not null,
    description text not null default '',
    version integer not null default 1,
    skill_md text not null default '',
    checksum text not null,
    size_bytes bigint not null default 0,
    file_count integer not null default 0,
    dependencies text[] not null default '{}',
    enabled boolean not null default true,
    imported_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index ai_skills_slug_uq on ai_skills (lower(slug));

create table ai_skill_files (
    id uuid primary key default gen_random_uuid(),
    skill_id uuid not null references ai_skills(id) on delete cascade,
    path text not null,
    content bytea not null,
    size_bytes bigint not null,
    sha256 text not null,
    unique (skill_id, path)
);

create table ai_skill_versions (
    id uuid primary key default gen_random_uuid(),
    skill_id uuid not null references ai_skills(id) on delete cascade,
    version integer not null,
    skill_md text not null,
    checksum text not null,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    unique (skill_id, version)
);

create table ai_skill_assignments (
    id uuid primary key default gen_random_uuid(),
    skill_id uuid not null references ai_skills(id) on delete cascade,
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
create unique index ai_skill_assignments_uq
    on ai_skill_assignments (skill_id, scope, coalesce(user_id, '00000000-0000-0000-0000-000000000000'::uuid),
                             coalesce(workspace_id, '00000000-0000-0000-0000-000000000000'::uuid),
                             coalesce(role_id, '00000000-0000-0000-0000-000000000000'::uuid));
create index ai_skill_assignments_user_idx on ai_skill_assignments (user_id);
create index ai_skill_assignments_workspace_idx on ai_skill_assignments (workspace_id);

-- ---------------------------------------------------------------------------
-- Tool policies и программы (снимок возможностей Runtime)
-- ---------------------------------------------------------------------------

create table ai_tool_policies (
    tool_id text primary key,
    enabled boolean not null default true,
    risk text not null default 'low' check (risk in ('low', 'medium', 'high', 'critical')),
    approval_policy text not null default 'risk'
        check (approval_policy in ('never', 'risk', 'always')),
    max_timeout_seconds integer not null default 120 check (max_timeout_seconds > 0),
    max_output_bytes bigint not null default 262144,
    updated_by uuid references users(id) on delete set null,
    updated_at timestamptz not null default now()
);

create table ai_programs (
    id uuid primary key default gen_random_uuid(),
    runtime_node_id uuid not null references ai_runtime_nodes(id) on delete cascade,
    program_id text not null,
    name text not null,
    executable text not null default '',
    version text not null default '',
    available boolean not null default false,
    allowed_args jsonb not null default '{}'::jsonb,
    risk text not null default 'high' check (risk in ('low', 'medium', 'high', 'critical')),
    enabled boolean not null default true,
    updated_at timestamptz not null default now(),
    unique (runtime_node_id, program_id)
);
create index ai_programs_node_idx on ai_programs (runtime_node_id);
