-- NEXUS WORKSPACE — Персональные квоты хранения (этап 12, PLAN §8.1, §9.1)

alter table users
    add column if not exists quota_bytes bigint
        check (quota_bytes is null or quota_bytes > 0);

create table if not exists quota_defaults (
    scope text primary key check (scope in ('cloud', 'minecraft')),
    bytes bigint not null check (bytes > 0),
    updated_at timestamptz not null default now()
);

insert into quota_defaults (scope, bytes) values
    ('cloud', 53687091200),
    ('minecraft', 107374182400)
on conflict (scope) do nothing;
