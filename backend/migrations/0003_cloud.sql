-- NEXUS WORKSPACE — Cloud Storage (этап 5)
-- Метаданные файлов и папок, общий доступ, избранное.

create table cloud_entries (
    id uuid primary key default gen_random_uuid(),
    owner_id uuid not null references users(id) on delete cascade,
    parent_id uuid references cloud_entries(id) on delete cascade,
    name text not null,
    entry_type text not null check (entry_type in ('folder', 'file')),
    object_id uuid references storage_objects(id) on delete set null,
    size_bytes bigint not null default 0 check (size_bytes >= 0),
    content_type text not null default 'application/octet-stream',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    trashed_at timestamptz
);
-- Уникальное имя внутри одной папки владельца (только вне корзины).
create unique index cloud_entries_name_uq
    on cloud_entries (owner_id, coalesce(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), lower(name))
    where trashed_at is null;
create index cloud_entries_owner_idx on cloud_entries (owner_id, trashed_at);
create index cloud_entries_parent_idx on cloud_entries (parent_id);
create index cloud_entries_updated_idx on cloud_entries (owner_id, updated_at desc);
create index cloud_entries_object_idx on cloud_entries (object_id);

create table cloud_shares (
    id uuid primary key default gen_random_uuid(),
    entry_id uuid not null references cloud_entries(id) on delete cascade,
    owner_id uuid not null references users(id) on delete cascade,
    token_hash text not null unique,
    password_hash text,
    expires_at timestamptz,
    max_downloads integer,
    download_count integer not null default 0,
    revoked_at timestamptz,
    created_at timestamptz not null default now()
);
create index cloud_shares_entry_idx on cloud_shares (entry_id);
create index cloud_shares_owner_idx on cloud_shares (owner_id);

create table cloud_favorites (
    entry_id uuid primary key references cloud_entries(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    created_at timestamptz not null default now()
);

-- Папка назначения для возобновляемой загрузки (этап 5).
alter table upload_sessions
    add column if not exists parent_id uuid references cloud_entries(id) on delete set null;
