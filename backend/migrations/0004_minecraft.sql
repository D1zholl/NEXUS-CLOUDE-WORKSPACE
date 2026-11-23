-- NEXUS WORKSPACE — Minecraft Build Manager (этап 6)
-- Сборки, файлы сборок, релизы, кеш версий загрузчиков, история изменений.

create table minecraft_builds (
    id uuid primary key default gen_random_uuid(),
    slug text not null unique,
    name text not null,
    description text not null default '',
    version text not null default '1.0.0',
    category text not null default 'other',
    visibility text not null default 'private'
        check (visibility in ('public', 'private')),
    status text not null default 'draft'
        check (status in ('draft', 'published', 'archived')),
    minecraft_version text not null default '',
    loader_type text not null default 'vanilla'
        check (loader_type in ('vanilla', 'fabric', 'forge', 'neoforge', 'quilt')),
    loader_version text not null default '',
    java_major integer not null default 21 check (java_major between 8 and 30),
    java_min integer not null default 21 check (java_min between 8 and 30),
    java_notes text not null default '',
    icon_object_id uuid references storage_objects(id) on delete set null,
    current_release_id uuid,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    archived_at timestamptz
);
create index minecraft_builds_status_idx on minecraft_builds (status, updated_at desc);
create index minecraft_builds_slug_idx on minecraft_builds (lower(slug));
create index minecraft_builds_loader_idx on minecraft_builds (loader_type, minecraft_version);

create table minecraft_build_files (
    id uuid primary key default gen_random_uuid(),
    build_id uuid not null references minecraft_builds(id) on delete cascade,
    kind text not null default 'file' check (kind in ('file', 'dir')),
    path text not null,
    category text not null default 'other'
        check (category in ('mods', 'config', 'resourcepacks', 'shaderpacks', 'scripts', 'overrides', 'other')),
    side text not null default 'client' check (side in ('client', 'server', 'both')),
    required boolean not null default true,
    enabled boolean not null default true,
    description text not null default '',
    object_id uuid references storage_objects(id) on delete set null,
    size_bytes bigint not null default 0 check (size_bytes >= 0),
    sha256 text not null default '',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
-- Пути сравниваются без учёта регистра и в нормализованном виде.
create unique index minecraft_build_files_path_uq
    on minecraft_build_files (build_id, lower(path));
create index minecraft_build_files_build_idx on minecraft_build_files (build_id, category);
create index minecraft_build_files_object_idx on minecraft_build_files (object_id);

create table minecraft_releases (
    id uuid primary key default gen_random_uuid(),
    build_id uuid not null references minecraft_builds(id) on delete cascade,
    version text not null,
    status text not null default 'draft'
        check (status in ('draft', 'validating', 'ready', 'published', 'archived', 'deprecated')),
    changelog text not null default '',
    manifest jsonb not null default '{}'::jsonb,
    file_count integer not null default 0,
    total_bytes bigint not null default 0,
    created_by uuid references users(id) on delete set null,
    created_at timestamptz not null default now(),
    published_at timestamptz,
    archived_at timestamptz,
    deprecated_at timestamptz,
    unique (build_id, version)
);
create index minecraft_releases_build_idx on minecraft_releases (build_id, created_at desc);
create index minecraft_releases_status_idx on minecraft_releases (status);

alter table minecraft_builds
    add constraint minecraft_builds_current_release_fk
    foreign key (current_release_id) references minecraft_releases(id) on delete set null;

create table minecraft_release_files (
    id uuid primary key default gen_random_uuid(),
    release_id uuid not null references minecraft_releases(id) on delete cascade,
    path text not null,
    category text not null,
    side text not null,
    required boolean not null,
    enabled boolean not null default true,
    size_bytes bigint not null default 0,
    sha256 text not null default '',
    object_id uuid references storage_objects(id) on delete set null,
    source_file_id uuid
);
create index minecraft_release_files_release_idx on minecraft_release_files (release_id, path);

create table minecraft_loader_versions (
    id uuid primary key default gen_random_uuid(),
    loader_type text not null,
    loader_version text not null,
    minecraft_version text not null,
    fetched_at timestamptz not null default now(),
    unique (loader_type, loader_version, minecraft_version)
);
create index minecraft_loader_versions_lookup_idx
    on minecraft_loader_versions (loader_type, minecraft_version);

create table minecraft_release_history (
    id uuid primary key default gen_random_uuid(),
    build_id uuid not null references minecraft_builds(id) on delete cascade,
    release_id uuid references minecraft_releases(id) on delete set null,
    event text not null,
    actor_id uuid references users(id) on delete set null,
    actor_username text not null default '',
    details jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);
create index minecraft_release_history_build_idx
    on minecraft_release_history (build_id, created_at desc);

create table minecraft_upload_sessions (
    id uuid primary key default gen_random_uuid(),
    build_id uuid not null references minecraft_builds(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    volume_id uuid not null references storage_volumes(id) on delete cascade,
    target_path text not null,
    category text not null default 'other',
    side text not null default 'client',
    required boolean not null default true,
    description text not null default '',
    content_type text not null default 'application/octet-stream',
    declared_size bigint not null check (declared_size >= 0),
    received_bytes bigint not null default 0,
    status text not null default 'active'
        check (status in ('active', 'completed', 'aborted', 'expired')),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    expires_at timestamptz not null
);
create index minecraft_upload_sessions_user_idx
    on minecraft_upload_sessions (user_id, status);
create index minecraft_upload_sessions_expires_idx
    on minecraft_upload_sessions (expires_at);
