-- Владелец сборки: приватная сборка доступна в Launcher API только ему.
-- `created_by` остаётся историческим автором для аудита, `owner_id` можно
-- передать другому пользователю без переписывания истории.

alter table minecraft_builds
    add column owner_id uuid references users(id) on delete set null;

update minecraft_builds
set owner_id = created_by
where owner_id is null;

create index minecraft_builds_owner_idx on minecraft_builds (owner_id);
