-- NEXUS WORKSPACE — Представление корзины (PLAN §10, отклонение P2)
--
-- Корзина хранится полем `cloud_entries.trashed_at`, а не отдельной таблицей:
-- это исключает рассинхронизацию и упрощает транзакции. Представление
-- `cloud_trash` даёт совместимый взгляд «как таблица» без второго источника истины.

create or replace view cloud_trash as
select e.id,
       e.owner_id,
       e.parent_id,
       e.name,
       e.entry_type,
       e.object_id,
       e.size_bytes,
       e.content_type,
       e.trashed_at
from cloud_entries e
where e.trashed_at is not null;

comment on view cloud_trash is
    'Корзина Cloud Storage. Источник истины — cloud_entries.trashed_at.';
