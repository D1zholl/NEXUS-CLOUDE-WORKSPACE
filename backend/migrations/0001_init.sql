-- NEXUS WORKSPACE — базовая схема (этапы 1–2)
create extension if not exists pgcrypto;

create table users (
    id uuid primary key default gen_random_uuid(),
    username text not null,
    display_name text not null,
    password_hash text not null,
    status text not null default 'active' check (status in ('active', 'disabled')),
    is_superadmin boolean not null default false,
    failed_login_attempts integer not null default 0,
    locked_until timestamptz,
    last_login_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create unique index users_username_lower_uq on users (lower(username));
create index users_status_idx on users (status);

create table roles (
    id uuid primary key default gen_random_uuid(),
    key text not null unique,
    name text not null,
    description text not null default '',
    is_system boolean not null default false,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table permissions (
    key text primary key,
    module text not null,
    label text not null
);
create index permissions_module_idx on permissions (module);

create table role_permissions (
    role_id uuid not null references roles(id) on delete cascade,
    permission_key text not null references permissions(key) on delete cascade,
    primary key (role_id, permission_key)
);

create table user_roles (
    user_id uuid not null references users(id) on delete cascade,
    role_id uuid not null references roles(id) on delete cascade,
    assigned_at timestamptz not null default now(),
    primary key (user_id, role_id)
);
create index user_roles_role_idx on user_roles (role_id);

create table user_permissions (
    user_id uuid not null references users(id) on delete cascade,
    permission_key text not null references permissions(key) on delete cascade,
    granted boolean not null default true,
    primary key (user_id, permission_key)
);

create table sessions (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references users(id) on delete cascade,
    token_hash text not null unique,
    created_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now(),
    expires_at timestamptz not null,
    revoked_at timestamptz,
    ip text not null default '',
    user_agent text not null default ''
);
create index sessions_user_idx on sessions (user_id);
create index sessions_expires_idx on sessions (expires_at);

create table access_tokens (
    id uuid primary key default gen_random_uuid(),
    user_id uuid references users(id) on delete cascade,
    name text not null,
    token_hash text not null unique,
    scopes text[] not null default '{}',
    last_used_at timestamptz,
    expires_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz not null default now()
);
create index access_tokens_user_idx on access_tokens (user_id);

create table system_settings (
    key text primary key,
    value jsonb not null,
    updated_by uuid references users(id) on delete set null,
    updated_at timestamptz not null default now()
);

create table user_settings (
    user_id uuid not null references users(id) on delete cascade,
    key text not null,
    value jsonb not null,
    updated_at timestamptz not null default now(),
    primary key (user_id, key)
);

create table audit_events (
    id uuid primary key default gen_random_uuid(),
    created_at timestamptz not null default now(),
    event_type text not null,
    actor_id uuid references users(id) on delete set null,
    actor_username text not null default 'system',
    target_type text not null default '',
    target_id text not null default '',
    result text not null default 'success',
    ip text not null default '',
    details jsonb not null default '{}'::jsonb
);
create index audit_events_created_idx on audit_events (created_at desc);
create index audit_events_type_idx on audit_events (event_type);
create index audit_events_actor_idx on audit_events (actor_id);

create table background_jobs (
    id uuid primary key default gen_random_uuid(),
    job_type text not null,
    status text not null default 'pending'
        check (status in ('pending', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    progress double precision not null default 0,
    total_units bigint,
    done_units bigint,
    error text,
    payload jsonb not null default '{}'::jsonb,
    result jsonb,
    created_at timestamptz not null default now(),
    started_at timestamptz,
    finished_at timestamptz,
    created_by uuid references users(id) on delete set null
);
create index background_jobs_status_idx on background_jobs (status);
create index background_jobs_created_idx on background_jobs (created_at desc);

create table system_metrics (
    id bigserial primary key,
    captured_at timestamptz not null default now(),
    metric text not null,
    value double precision not null,
    labels jsonb not null default '{}'::jsonb
);
create index system_metrics_time_idx on system_metrics (captured_at desc);
create index system_metrics_name_idx on system_metrics (metric, captured_at desc);

create table module_states (
    key text primary key,
    enabled boolean not null default true,
    updated_at timestamptz not null default now(),
    updated_by uuid references users(id) on delete set null
);

-- Каталог прав
insert into permissions (key, module, label) values
    ('minecraft.view', 'minecraft', 'Просмотр сборок Minecraft'),
    ('minecraft.build.create', 'minecraft', 'Создание сборок'),
    ('minecraft.build.edit', 'minecraft', 'Редактирование сборок'),
    ('minecraft.build.publish', 'minecraft', 'Публикация релизов'),
    ('minecraft.build.delete', 'minecraft', 'Удаление сборок'),
    ('minecraft.artifacts.upload', 'minecraft', 'Загрузка файлов сборок'),
    ('cloud.view', 'cloud', 'Доступ к облачному хранилищу'),
    ('cloud.file.upload', 'cloud', 'Загрузка файлов'),
    ('cloud.file.download', 'cloud', 'Скачивание файлов'),
    ('cloud.file.delete', 'cloud', 'Удаление файлов'),
    ('cloud.file.share', 'cloud', 'Общий доступ к файлам'),
    ('cloud.folder.create', 'cloud', 'Создание папок'),
    ('cloud.storage.manage', 'cloud', 'Управление хранилищами'),
    ('system.view', 'system', 'Просмотр состояния системы'),
    ('system.users.manage', 'system', 'Управление пользователями'),
    ('system.storage.manage', 'system', 'Управление дисками и хранилищами'),
    ('system.settings.manage', 'system', 'Управление настройками'),
    ('system.monitoring.view', 'system', 'Просмотр мониторинга'),
    ('system.jobs.manage', 'system', 'Управление фоновыми задачами')
on conflict (key) do nothing;

insert into roles (id, key, name, description, is_system) values
    ('00000000-0000-0000-0000-000000000001', 'administrator', 'Administrator', 'Полный доступ ко всем модулям и системным настройкам', true),
    ('00000000-0000-0000-0000-000000000002', 'user', 'User', 'Базовый доступ к рабочим пространствам', true)
on conflict (key) do nothing;

insert into role_permissions (role_id, permission_key)
select '00000000-0000-0000-0000-000000000001', key from permissions
on conflict do nothing;

insert into role_permissions (role_id, permission_key) values
    ('00000000-0000-0000-0000-000000000002', 'minecraft.view'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.view'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.file.upload'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.file.download'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.file.delete'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.file.share'),
    ('00000000-0000-0000-0000-000000000002', 'cloud.folder.create')
on conflict do nothing;

insert into module_states (key, enabled) values
    ('minecraft', true),
    ('cloud', true)
on conflict (key) do nothing;
