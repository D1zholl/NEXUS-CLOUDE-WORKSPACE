-- Режим загрузки «заменить существующий файл» должен переживать паузу,
-- продолжение и повторное открытие страницы: сессия хранит его на сервере.
alter table minecraft_upload_sessions
    add column replace_existing boolean not null default false;
