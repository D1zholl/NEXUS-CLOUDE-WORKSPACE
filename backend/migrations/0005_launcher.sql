-- NEXUS WORKSPACE — Launcher API (этап 7)
-- Логирование загрузок лаунчера и право управления токенами.

create table launcher_downloads (
    id uuid primary key default gen_random_uuid(),
    token_id uuid references access_tokens(id) on delete set null,
    build_id uuid references minecraft_builds(id) on delete set null,
    release_id uuid references minecraft_releases(id) on delete set null,
    artifact_id uuid,
    path text not null default '',
    ip text not null default '',
    user_agent text not null default '',
    bytes_sent bigint not null default 0,
    range_header text not null default '',
    status integer not null default 200,
    created_at timestamptz not null default now()
);
create index launcher_downloads_created_idx on launcher_downloads (created_at desc);
create index launcher_downloads_token_idx on launcher_downloads (token_id, created_at desc);
create index launcher_downloads_build_idx on launcher_downloads (build_id, created_at desc);

insert into permissions (key, module, label) values
    ('launcher.manage', 'launcher', 'Управление токенами лаунчера')
on conflict (key) do nothing;

insert into role_permissions (role_id, permission_key)
select '00000000-0000-0000-0000-000000000001', 'launcher.manage'
on conflict do nothing;
