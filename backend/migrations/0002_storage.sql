-- NEXUS WORKSPACE — Storage Engine (этап 4)
-- Объектное хранение: тома, объекты, ссылки, сессии загрузки, использование квот.

create table storage_volumes (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    root_path text not null unique,
    disk_mount text not null default '',
    mode text not null default 'auto' check (mode in ('auto', 'manual')),
    priority integer not null default 100 check (priority between 0 and 1000),
    reserved_bytes bigint not null default 0 check (reserved_bytes >= 0),
    max_bytes bigint check (max_bytes is null or max_bytes > 0),
    enabled boolean not null default true,
    state text not null default 'online' check (state in ('online', 'offline', 'error')),
    state_message text not null default '',
    total_bytes bigint not null default 0,
    available_bytes bigint not null default 0,
    last_checked_at timestamptz,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index storage_volumes_enabled_idx on storage_volumes (enabled, state);
create index storage_volumes_disk_idx on storage_volumes (disk_mount);

create table storage_objects (
    id uuid primary key default gen_random_uuid(),
    volume_id uuid not null references storage_volumes(id) on delete restrict,
    object_key text not null,
    size_bytes bigint not null check (size_bytes >= 0),
    sha256 text not null check (sha256 ~ '^[0-9a-f]{64}$'),
    content_type text not null default 'application/octet-stream',
    state text not null default 'committed'
        check (state in ('staging', 'committed', 'quarantined', 'missing')),
    ref_count integer not null default 0 check (ref_count >= 0),
    created_at timestamptz not null default now(),
    verified_at timestamptz,
    unique (volume_id, object_key)
);
create index storage_objects_sha_idx on storage_objects (sha256);
create index storage_objects_volume_idx on storage_objects (volume_id);
create index storage_objects_state_idx on storage_objects (state);

create table storage_object_refs (
    id uuid primary key default gen_random_uuid(),
    object_id uuid not null references storage_objects(id) on delete cascade,
    owner_module text not null check (owner_module in ('cloud', 'minecraft', 'system')),
    owner_id uuid not null,
    display_name text not null default '',
    created_at timestamptz not null default now(),
    unique (owner_module, owner_id)
);
create index storage_object_refs_object_idx on storage_object_refs (object_id);

create table upload_sessions (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references users(id) on delete cascade,
    volume_id uuid not null references storage_volumes(id) on delete cascade,
    target_folder text not null default '',
    file_name text not null,
    content_type text not null default 'application/octet-stream',
    declared_size bigint check (declared_size is null or declared_size >= 0),
    received_bytes bigint not null default 0 check (received_bytes >= 0),
    sha256_expected text,
    temp_key text not null,
    status text not null default 'active'
        check (status in ('active', 'completed', 'aborted', 'expired')),
    metadata jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    expires_at timestamptz not null
);
create index upload_sessions_user_idx on upload_sessions (user_id, status);
create index upload_sessions_expires_idx on upload_sessions (expires_at);

create table storage_usage (
    user_id uuid primary key references users(id) on delete cascade,
    used_bytes bigint not null default 0 check (used_bytes >= 0),
    file_count bigint not null default 0 check (file_count >= 0),
    updated_at timestamptz not null default now()
);
