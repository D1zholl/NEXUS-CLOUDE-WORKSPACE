-- Снятие релиза с публикации без удаления снимка.
--
-- Статус `unpublished` отличается от `archived`: релиз скрыт из каталога и
-- файлов лаунчера, но его снимок можно опубликовать снова.

alter table minecraft_releases
    drop constraint if exists minecraft_releases_status_check;

alter table minecraft_releases
    add constraint minecraft_releases_status_check
        check (status in (
            'draft', 'validating', 'ready', 'published', 'archived',
            'deprecated', 'unpublished'
        ));
