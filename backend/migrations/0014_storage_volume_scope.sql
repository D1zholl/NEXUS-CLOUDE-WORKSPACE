-- NEXUS WORKSPACE — фаза K1: привязка тома к модулю и владельцу.
--
-- `scope` задаёт рабочее пространство, которому принадлежит том:
--   * system    — общий том, доступный обоим модулям (поведение до миграции);
--   * cloud     — только NEXUS Cloud Storage;
--   * minecraft — только Minecraft Build Manager.
-- `owner_user_id` делает том персональным: null — общий, иначе только владелец.
-- Существующие тома получают scope='system' и owner_user_id=null, поэтому
-- после миграции продолжают работать как раньше.
--
-- Примечание: номер 0013 уже занят миграцией 0013_minecraft_builds_owner.sql,
-- поэтому K1 оформлена как 0014, чтобы не дублировать версию sqlx.

alter table storage_volumes
    add column scope text not null default 'system'
        check (scope in ('system', 'cloud', 'minecraft')),
    add column owner_user_id uuid references users(id) on delete set null;

create index storage_volumes_scope_idx
    on storage_volumes (scope, owner_user_id);
