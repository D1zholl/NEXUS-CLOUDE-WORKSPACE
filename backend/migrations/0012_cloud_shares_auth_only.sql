-- NEXUS WORKSPACE — фаза E3/E4: ссылки только для авторизованных и корзина.
--
-- `auth_only` запрещает анонимный доступ к публичной ссылке: сервер вручную
-- проверяет cookie-сессию `nexus_session` (см. cloud/routes.rs).
-- Частичный индекс ускоряет выборку записей корзины для задачи очистки
-- `system.trash.cleanup`, не увеличивая размер индекса активными файлами.

alter table cloud_shares
    add column if not exists auth_only boolean not null default false;

create index if not exists cloud_entries_trashed_idx
    on cloud_entries (trashed_at)
    where trashed_at is not null;
