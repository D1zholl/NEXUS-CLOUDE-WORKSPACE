-- NEXUS WORKSPACE — MyLauncher repository compatibility.
--
-- 1. Канал релиза: stable/beta. Latest выбирается отдельно для каждого канала.
-- 2. Неизменяемый снимок манифеста в формате MyLauncher, его SHA-256 и версия
--    схемы. Снимок создаётся при публикации и больше не пересобирается.
--
-- Существующие релизы получают stable: до появления каналов все публикации
-- считались стабильными.

alter table minecraft_releases
    add column channel text not null default 'stable'
        check (channel in ('stable', 'beta')),
    add column launcher_manifest jsonb,
    add column launcher_manifest_hash text,
    add column launcher_manifest_schema integer,
    add column launcher_manifest_at timestamptz;

-- Выборка latest по каналу и списка версий без полного сканирования.
create index minecraft_releases_channel_idx
    on minecraft_releases (build_id, channel, published_at desc)
    where status in ('published', 'deprecated');

-- Каталог: опубликованные релизы по каналу.
create index minecraft_releases_channel_status_idx
    on minecraft_releases (channel, status, published_at desc);
