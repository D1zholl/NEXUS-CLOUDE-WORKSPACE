-- Восстанавливает согласованность статуса сборки и её релизов.
--
-- Ранее сборка могла остаться в статусе `draft`, хотя у неё уже был
-- опубликованный релиз (например, после архивирования текущего релиза и
-- активации другого). Из-за этого карточка показывала «Черновик», а каталог
-- лаунчера не видел сборку. Миграция лечит уже накопленные данные.

-- 1. Есть опубликованный релиз — сборка опубликована.
update minecraft_builds b
set status = 'published',
    updated_at = now()
where b.status = 'draft'
  and b.archived_at is null
  and exists (
      select 1
      from minecraft_releases r
      where r.build_id = b.id and r.status = 'published'
  );

-- 2. Текущим должен быть опубликованный релиз, а не удалённый/устаревший.
with latest as (
    select distinct on (build_id) build_id, id, version
    from minecraft_releases
    where status = 'published'
    order by build_id,
             published_at desc nulls last,
             created_at desc
)
update minecraft_builds b
set current_release_id = latest.id,
    version = latest.version,
    updated_at = now()
from latest
where latest.build_id = b.id
  and b.status = 'published'
  and b.archived_at is null
  and not exists (
      select 1
      from minecraft_releases current
      where current.id = b.current_release_id
        and current.status = 'published'
  );

-- 3. Опубликованных релизов нет — сборка снова черновик.
update minecraft_builds b
set status = 'draft',
    current_release_id = null,
    updated_at = now()
where b.status = 'published'
  and b.archived_at is null
  and not exists (
      select 1
      from minecraft_releases r
      where r.build_id = b.id and r.status = 'published'
  );
