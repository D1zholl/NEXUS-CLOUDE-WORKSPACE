# База данных NEXUS-launcher

**Версия документа:** 1.0
**Дата:** 21.11.2026
**Статус:** проект схемы. Реализация не начата.
**Связанные документы:** `docs/ARCHITECTURE.md`, `docs/DECISIONS.md`.

---

## 1. Роль базы данных

SQLite — **индекс и операционное состояние**, а не источник истины для
переносимых данных.

| Данные | Источник истины | Роль БД |
|---|---|---|
| Инстанс (имя, версия, память, источник) | `instances/<id>/instance.json` | Индекс для списков и запросов |
| Файлы инстанса | `instance.lock.json` + файловая система | Индекс для UI и мод-каталога |
| Сервер | `servers/<id>/server.json` | Индекс |
| Аккаунты | keyring (секреты) + БД (профиль) | Профиль, статус, `secret_ref` |
| Java runtime | файловая система + БД | Реестр установленных runtime |
| Сборка NEXUS | манифест релиза + lock | Связь «инстанс ↔ источник ↔ версия» |
| Настройки UI | БД `app_settings` | Основное хранилище |
| Bootstrap-настройки | `config.toml` | Дублируются только для чтения |
| Бэкапы | файлы архивов | Индекс и статус |
| Загрузки | staging/файлы | Возобновляемое состояние |
| Секреты | Windows Credential Manager | **Никогда** значения, только `secret_ref` |

Следствия:

- БД можно пересоздать сканированием манифестов (`repair index`), не теряя
  пользовательские данные.
- Ручное редактирование `instance.json` при остановленном лаунчере допустимо;
  при старте индекс сверяется с манифестом.
- Удаление строки БД не является удалением файлов; и наоборот.

---

## 2. Подключение и PRAGMA

Один процесс-писатель (single-instance plugin). На каждое соединение пула:

```sql
PRAGMA journal_mode = WAL;          -- персистентно для файла
PRAGMA foreign_keys = ON;           -- обязательно на каждом соединении
PRAGMA busy_timeout = 5000;         -- мс
PRAGMA synchronous = NORMAL;        -- WAL + NORMAL
PRAGMA temp_store = MEMORY;
PRAGMA journal_size_limit = 67108864;
PRAGMA trusted_schema = OFF;
```

Пул: `max_connections = 8`, `acquire_timeout = 5s`, `idle_timeout = 300s`.
Чтения — параллельно; записи — короткими транзакциями. Внутри транзакции
запрещены HTTP-вызовы, запуск процессов и `sleep`.

Файлы: `launcher.db`, `launcher.db-wal`, `launcher.db-shm`. Бэкап БД делается
через `VACUUM INTO '<path>'` (консистентный снимок при WAL), а не копированием
файлов.

---

## 3. Соглашения

| Соглашение | Значение |
|---|---|
| Идентификаторы | `TEXT`, UUID v4 в нижнем регистре, 36 символов |
| Время | `INTEGER`, Unix-время в **миллисекундах**, UTC |
| Булевы значения | `INTEGER` 0/1 + `CHECK (x IN (0,1))` |
| Перечисления | `TEXT` + `CHECK` |
| JSON | `TEXT` + `CHECK (json_valid(...))`; только для расширяемых данных |
| Пути | Только **относительные** (`root_kind` + `rel_path`) или абсолютные пути Java runtime (машинно-локальные) |
| Секреты | Только `secret_ref` (имя записи в keyring) |
| Таблицы | `STRICT`, `snake_case`, множественное число |
| Удаление | Явное; каскад только для дочерних записей инстанса/сервера |
| Мягкое удаление | Только `archived_at` у инстансов; остальное удаляется физически после подтверждения |
| Версионирование схемы | SQLx `_sqlx_migrations` (не своя таблица) |

Все таблицы создаются в `STRICT`-режиме (SQLite ≥ 3.37; bundled SQLite в
SQLx 0.9 это обеспечивает).

---

## 4. Схема

Порядок таблиц учитывает внешние ключи.

### 4.1. `app_meta`

Служебные метаданные приложения.

```sql
CREATE TABLE app_meta (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;
```

Обязательные ключи после миграции:

| Ключ | Пример | Назначение |
|---|---|---|
| `schema.name` | `nexus-launcher` | Идентификатор схемы |
| `schema.instance` | `1` | Версия `instance.json` |
| `schema.server` | `1` | Версия `server.json` |
| `schema.event` | `1` | Версия конверта событий |
| `contract.nexus` | `1.0.0` | Зафиксированная версия контракта NEXUS |
| `created_by` | `0.1.0` | Версия лаунчера, создавшего БД |
| `last_opened_by` | `0.2.0` | Версия последнего запуска |
| `last_integrity_check` | `1795274960000` | Время последней проверки |

### 4.2. `java_runtimes`

```sql
CREATE TABLE java_runtimes (
  id          TEXT PRIMARY KEY,
  vendor      TEXT NOT NULL,
  major       INTEGER NOT NULL CHECK (major BETWEEN 8 AND 40),
  arch        TEXT NOT NULL CHECK (arch IN ('x64','x86','arm64')),
  path        TEXT NOT NULL UNIQUE,
  source      TEXT NOT NULL CHECK (source IN ('managed','system','custom')),
  sha256      TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
  size_bytes  INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
  verified_at INTEGER,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_java_major_arch ON java_runtimes(major, arch);
```

`path` — единственное место, где хранится абсолютный путь, потому что runtime
привязан к машине. Для `source='managed'` путь обязан быть внутри
`<data>/java/`; это проверяется приложением.

### 4.3. `instances`

```sql
CREATE TABLE instances (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 96),
  description     TEXT NOT NULL DEFAULT '',
  dir_name        TEXT NOT NULL UNIQUE,
  game_version    TEXT NOT NULL,
  loader_type     TEXT NOT NULL
                  CHECK (loader_type IN ('vanilla','fabric','quilt','forge','neoforge')),
  loader_version  TEXT,
  java_runtime_id TEXT REFERENCES java_runtimes(id) ON DELETE SET NULL,
  memory_min_mb   INTEGER NOT NULL CHECK (memory_min_mb BETWEEN 512 AND 32768),
  memory_max_mb   INTEGER NOT NULL CHECK (memory_max_mb BETWEEN 512 AND 32768),
  icon_path       TEXT,
  source_kind     TEXT NOT NULL DEFAULT 'local'
                  CHECK (source_kind IN ('local','nexus','modpack')),
  source_id       TEXT,
  pack_id         TEXT,
  release         TEXT,
  archived_at     INTEGER,
  last_played_at  INTEGER,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  CHECK (memory_max_mb >= memory_min_mb),
  CHECK ((loader_type = 'vanilla' AND loader_version IS NULL)
      OR (loader_type <> 'vanilla' AND loader_version IS NOT NULL))
) STRICT;

CREATE INDEX idx_instances_archived ON instances(archived_at, updated_at DESC);
CREATE INDEX idx_instances_last_played ON instances(last_played_at DESC);
```

`dir_name` — UUID в виде строки, совпадает с `id`. Отдельная колонка нужна для
переходного периода и проверки целостности индекса. `icon_path` —
относительный (`icon.png`), не абсолютный.

`source_id` не имеет внешнего ключа: инстанс должен оставаться рабочим, даже
если источник NEXUS удалён или отключён. Связь с источником для установленных
сборок хранится в `installed_builds` с `ON DELETE RESTRICT`.

### 4.4. `instance_files`

Инвентарь файлов инстанса; соответствует `instance.lock.json`.

```sql
CREATE TABLE instance_files (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  path        TEXT NOT NULL,             -- 'game/mods/x.jar', NFC, '/'
  path_key    TEXT NOT NULL,             -- lower(NFC(path)) для Windows
  category    TEXT NOT NULL
              CHECK (category IN ('mods','config','resourcepacks','shaderpacks',
                                  'scripts','overrides','other')),
  side        TEXT NOT NULL CHECK (side IN ('client','server','both')),
  origin      TEXT NOT NULL
              CHECK (origin IN ('minecraft','loader','modrinth','curseforge',
                                'nexus','modpack','user')),
  size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
  sha256      TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
  required    INTEGER NOT NULL DEFAULT 1 CHECK (required IN (0,1)),
  enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  managed     INTEGER NOT NULL DEFAULT 1 CHECK (managed IN (0,1)),
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  UNIQUE (instance_id, path_key)
) STRICT;

CREATE INDEX idx_instance_files_instance ON instance_files(instance_id, category, path);
CREATE INDEX idx_instance_files_hash ON instance_files(sha256) WHERE sha256 IS NOT NULL;
```

`managed=0` — пользовательский файл, который лаунчер не удаляет при обновлении
сборки. `path_key` защищает от коллизий `Mods/A.jar` / `mods/a.jar` на Windows.

### 4.5. `mod_metadata`

Связь файла инстанса с каталогом модов.

```sql
CREATE TABLE mod_metadata (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  instance_id      TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
  instance_file_id INTEGER NOT NULL UNIQUE
                   REFERENCES instance_files(id) ON DELETE CASCADE,
  provider         TEXT NOT NULL
                   CHECK (provider IN ('modrinth','curseforge','nexus','local')),
  project_id       TEXT NOT NULL,
  version_id       TEXT NOT NULL,
  project_name     TEXT NOT NULL,
  version_name     TEXT NOT NULL,
  side             TEXT CHECK (side IN ('client','server','both')),
  installed_at     INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_mod_metadata_project ON mod_metadata(provider, project_id);
CREATE INDEX idx_mod_metadata_instance ON mod_metadata(instance_id);
```

### 4.6. `accounts`

```sql
CREATE TABLE accounts (
  id                  TEXT PRIMARY KEY,
  provider            TEXT NOT NULL
                      CHECK (provider IN ('offline','microsoft','elyby','azuriom')),
  provider_account_id TEXT NOT NULL,
  username            TEXT NOT NULL,
  uuid                TEXT,
  display_name        TEXT,
  secret_ref          TEXT UNIQUE,      -- NULL для offline
  status              TEXT NOT NULL DEFAULT 'active'
                      CHECK (status IN ('active','expired','error')),
  last_used_at        INTEGER,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  UNIQUE (provider, provider_account_id)
) STRICT;

CREATE INDEX idx_accounts_status ON accounts(status, last_used_at DESC);
```

Правила:

- `secret_ref` — ключ записи в Windows Credential Manager, не токен.
- Offline-аккаунт не имеет секрета; `provider_account_id` — нормализованный
  ник (casefold), `uuid` — offline UUID.
- Провайдеры `elyby`/`azuriom` зарезервированы; отдельной таблицы
  `auth_providers` в v1 нет (ADR-023).

### 4.7. `nexus_sources`

```sql
CREATE TABLE nexus_sources (
  id                TEXT PRIMARY KEY,
  name              TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
  base_url          TEXT NOT NULL UNIQUE,
  enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  channel_default   TEXT NOT NULL DEFAULT 'stable'
                    CHECK (channel_default IN ('stable','beta')),
  auth_mode         TEXT NOT NULL DEFAULT 'public'
                    CHECK (auth_mode IN ('public','bearer')),
  secret_ref        TEXT,
  contract_version  TEXT NOT NULL DEFAULT '1.0.0',
  last_checked_at   INTEGER,
  last_status       TEXT CHECK (last_status IN ('ok','unreachable','contract_mismatch')),
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
) STRICT;
```

Источник не удаляется, если на него ссылается `installed_builds`
(`ON DELETE RESTRICT`); UI предлагает отключить (`enabled=0`).

### 4.8. `installed_builds`

```sql
CREATE TABLE installed_builds (
  id              TEXT PRIMARY KEY,
  instance_id     TEXT NOT NULL UNIQUE REFERENCES instances(id) ON DELETE CASCADE,
  source_id       TEXT NOT NULL REFERENCES nexus_sources(id) ON DELETE RESTRICT,
  pack_id         TEXT NOT NULL,
  release         TEXT NOT NULL,
  channel         TEXT NOT NULL CHECK (channel IN ('stable','beta')),
  manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
  installed_at    INTEGER NOT NULL,
  last_checked_at INTEGER
) STRICT;

CREATE INDEX idx_installed_builds_source ON installed_builds(source_id, pack_id);
```

### 4.9. `servers`

```sql
CREATE TABLE servers (
  id                TEXT PRIMARY KEY,
  name              TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 96),
  dir_name          TEXT NOT NULL UNIQUE,
  game_version      TEXT NOT NULL,
  loader_type       TEXT NOT NULL
                    CHECK (loader_type IN ('vanilla','fabric','quilt','forge','neoforge')),
  loader_version    TEXT,
  java_runtime_id   TEXT REFERENCES java_runtimes(id) ON DELETE SET NULL,
  memory_mb         INTEGER NOT NULL CHECK (memory_mb BETWEEN 512 AND 65536),
  port              INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
  auto_restart_json TEXT NOT NULL DEFAULT '{}'
                    CHECK (json_valid(auto_restart_json)),
  eula_accepted_at  INTEGER,
  install_state     TEXT NOT NULL DEFAULT 'pending'
                    CHECK (install_state IN ('pending','installed','failed')),
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL,
  CHECK ((loader_type = 'vanilla' AND loader_version IS NULL)
      OR (loader_type <> 'vanilla' AND loader_version IS NOT NULL))
) STRICT;

CREATE INDEX idx_servers_install_state ON servers(install_state, updated_at DESC);
```

`eula_accepted_at` — журнал действия пользователя. Фактическое разрешение
читается из `server/eula.txt`; БД не заменяет EULA-gate.

### 4.10. `server_processes`

История запусков и текущая идентичность процесса.

```sql
CREATE TABLE server_processes (
  id                  TEXT PRIMARY KEY,
  server_id           TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  run_id              TEXT NOT NULL,
  pid                 INTEGER NOT NULL,
  process_created_at  INTEGER NOT NULL,
  exe_path            TEXT NOT NULL,
  cmdline_hash        TEXT,
  launch_nonce        TEXT NOT NULL,
  status              TEXT NOT NULL
                      CHECK (status IN ('running','stopped','crashed','lost')),
  started_at          INTEGER NOT NULL,
  ended_at            INTEGER,
  exit_code           INTEGER,
  UNIQUE (server_id, run_id)
) STRICT;

CREATE UNIQUE INDEX idx_server_process_one_running
  ON server_processes(server_id) WHERE status = 'running';
CREATE INDEX idx_server_process_history
  ON server_processes(server_id, started_at DESC);
```

Частичный уникальный индекс гарантирует один `running` на сервер. `lost` —
лаунчер не смог подтвердить процесс; такой процесс не убивается.

### 4.11. `download_tasks`

```sql
CREATE TABLE download_tasks (
  id              TEXT PRIMARY KEY,
  job_id          TEXT,
  kind            TEXT NOT NULL
                  CHECK (kind IN ('minecraft','java','loader','mod','modpack',
                                  'nexus','server','custom')),
  label           TEXT NOT NULL,
  status          TEXT NOT NULL
                  CHECK (status IN ('queued','running','paused','completed',
                                    'failed','cancelled')),
  priority        INTEGER NOT NULL DEFAULT 100,
  total_files     INTEGER NOT NULL DEFAULT 0 CHECK (total_files >= 0),
  completed_files INTEGER NOT NULL DEFAULT 0 CHECK (completed_files >= 0),
  total_bytes     INTEGER NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
  done_bytes      INTEGER NOT NULL DEFAULT 0 CHECK (done_bytes >= 0),
  error_code      TEXT,
  created_at      INTEGER NOT NULL,
  started_at      INTEGER,
  finished_at     INTEGER,
  updated_at      INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_download_tasks_queue
  ON download_tasks(status, priority, created_at);
CREATE INDEX idx_download_tasks_job ON download_tasks(job_id);
```

### 4.12. `download_files`

```sql
CREATE TABLE download_files (
  id               TEXT PRIMARY KEY,
  task_id          TEXT NOT NULL REFERENCES download_tasks(id) ON DELETE CASCADE,
  url              TEXT NOT NULL,
  root_kind        TEXT NOT NULL
                   CHECK (root_kind IN ('shared_assets','shared_libraries',
                                        'shared_versions','instance','server',
                                        'java','cache','downloads')),
  rel_path         TEXT NOT NULL,
  path_key         TEXT NOT NULL,
  size_bytes       INTEGER NOT NULL CHECK (size_bytes >= 0),
  sha1             TEXT CHECK (sha1 IS NULL OR length(sha1) = 40),
  sha256           TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
  required         INTEGER NOT NULL DEFAULT 1 CHECK (required IN (0,1)),
  status           TEXT NOT NULL
                   CHECK (status IN ('queued','running','paused','completed',
                                     'failed','skipped')),
  bytes_done       INTEGER NOT NULL DEFAULT 0 CHECK (bytes_done >= 0),
  attempts         INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  etag             TEXT,
  last_http_status INTEGER,
  error_code       TEXT,
  updated_at       INTEGER NOT NULL,
  UNIQUE (task_id, path_key)
) STRICT;

CREATE INDEX idx_download_files_task ON download_files(task_id, status);
```

`root_kind` + `rel_path` не позволяют БД хранить произвольный абсолютный путь
назначения. Резолвинг выполняется через `DataPaths`/`RootedDir`.

### 4.13. `backups`

```sql
CREATE TABLE backups (
  id          TEXT PRIMARY KEY,
  target_kind TEXT NOT NULL
              CHECK (target_kind IN ('instance','server','world')),
  target_id   TEXT NOT NULL,
  target_name TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('full','selected')),
  file_path   TEXT NOT NULL,            -- относительно <data>/backups
  size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
  sha256      TEXT NOT NULL CHECK (length(sha256) = 64),
  status      TEXT NOT NULL
              CHECK (status IN ('creating','ready','failed','restored')),
  verified_at INTEGER,
  created_at  INTEGER NOT NULL,
  note        TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX idx_backups_target ON backups(target_kind, target_id, created_at DESC);
CREATE INDEX idx_backups_status ON backups(status, created_at);
```

`target_id` без FK: цель может быть удалена, но запись о бэкапе остаётся
полезной для диагностики. `target_name` — снимок имени на момент создания.

### 4.14. `crash_reports`

```sql
CREATE TABLE crash_reports (
  id          TEXT PRIMARY KEY,
  instance_id TEXT REFERENCES instances(id) ON DELETE SET NULL,
  server_id   TEXT REFERENCES servers(id) ON DELETE SET NULL,
  occurred_at INTEGER NOT NULL,
  file_path   TEXT NOT NULL,            -- относительно game/crash-reports
  summary     TEXT NOT NULL,
  signature   TEXT,
  created_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_crash_reports_time ON crash_reports(occurred_at DESC);
CREATE INDEX idx_crash_reports_instance ON crash_reports(instance_id, occurred_at DESC);
```

Файлы crash report остаются на диске; таблица — индекс для UI.

### 4.15. `jobs`

```sql
CREATE TABLE jobs (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,
  status      TEXT NOT NULL
              CHECK (status IN ('queued','running','succeeded','failed',
                                'cancelled','interrupted')),
  stage       TEXT,
  done_units  INTEGER NOT NULL DEFAULT 0 CHECK (done_units >= 0),
  total_units INTEGER CHECK (total_units IS NULL OR total_units >= 0),
  entity_kind TEXT,
  entity_id   TEXT,
  error_code  TEXT,
  created_at  INTEGER NOT NULL,
  started_at  INTEGER,
  finished_at INTEGER,
  updated_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_jobs_status ON jobs(status, created_at);
CREATE INDEX idx_jobs_entity ON jobs(entity_kind, entity_id, created_at DESC);
```

Прогресс в БД не пишется на каждое событие: только стадии и финальный статус
(иначе WAL раздувается). При старте `running` → `interrupted`.

### 4.16. `activity_logs`

```sql
CREATE TABLE activity_logs (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  at           INTEGER NOT NULL,
  kind         TEXT NOT NULL,
  entity_kind  TEXT,
  entity_id    TEXT,
  summary      TEXT NOT NULL,
  details_json TEXT CHECK (details_json IS NULL OR json_valid(details_json))
) STRICT;

CREATE INDEX idx_activity_at ON activity_logs(at DESC);
CREATE INDEX idx_activity_entity ON activity_logs(entity_kind, entity_id, at DESC);
```

Журнал действий без секретов и без полных путей. Retention: 90 дней или
20 000 строк.

### 4.17. `app_settings`

```sql
CREATE TABLE app_settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL CHECK (json_valid(value_json)),
  updated_at INTEGER NOT NULL
) STRICT;
```

Ключи типизированы приложением (`ui.theme`, `ui.reduced_motion`,
`instances.default_memory_mb`, `dialogs.*`). Неизвестные ключи читаются, но не
применяются; при записи набора ключей неизвестные сохраняются.

---

## 5. Сводная карта связей

```
java_runtimes 1 ──── * instances
java_runtimes 1 ──── * servers
instances     1 ──── * instance_files ──── 1 mod_metadata
instances     1 ──── 1 installed_builds ──── * nexus_sources
servers       1 ──── * server_processes
servers       1 ──── * crash_reports
instances     1 ──── * crash_reports
download_tasks 1 ─── * download_files
backups       1 ──── * (файлы на диске)
jobs          —        (полиморфная ссылка entity_kind/entity_id)
```

Каскады:

- удаление `instances` → `instance_files`, `mod_metadata`,
  `installed_builds`, `crash_reports.instance_id` (SET NULL);
- удаление `servers` → `server_processes`, `crash_reports.server_id`
  (SET NULL);
- удаление `download_tasks` → `download_files`;
- удаление `nexus_sources` запрещено, пока есть `installed_builds`
  (`RESTRICT`);
- удаление `java_runtimes` → `instances/servers.java_runtime_id = NULL`
  (сервис дополнительно запрещает удаление используемого runtime).

---

## 6. Индексы и производительность

Обязательные индексы перечислены в DDL. Дополнительные правила:

- Списки инстансов: `ORDER BY archived_at IS NOT NULL, last_played_at DESC`
  с индексом `last_played_at`.
- Поиск модов внутри инстанса: `instance_id + category + path`.
- Хэш-поиск файлов: частичный индекс по `sha256`.
- Очередь загрузок: `status, priority, created_at`.
- История сервера: `server_id, started_at DESC`.
- Никаких `SELECT *` в репозиториях: явный список колонок.

Пагинация:

- `activity_logs`, `jobs`, `backups` — keyset (`(created_at, id) < (?, ?)`),
  не `OFFSET` на больших объёмах.
- `instance_files` — без пагинации (файлов немного), но с фильтром.
- Логи — не в БД; читаются из файлов.

---

## 7. Миграции

### 7.1. Политика

| Правило | Значение |
|---|---|
| Инструмент | `sqlx::migrate!` + таблица `_sqlx_migrations` |
| Направление | Только вперёд (`up`) |
| Имя файла | `NNNN_краткое_описание.sql`, `NNNN` — 4 цифры |
| Изменение применённой миграции | Запрещено (checksum mismatch = отказ старта) |
| Транзакционность | Каждая миграция в транзакции; без `PRAGMA` внутри |
| Down-миграции | Не поддерживаются |
| Откат | Восстановление БД из pre-migration бэкапа |
| Более новая БД | Отказ старта, `DB_SCHEMA_TOO_NEW` |

### 7.2. Порядок применения

1. Открыть БД, выставить PRAGMA.
2. Прочитать `_sqlx_migrations`.
3. Если максимальная применённая версия больше скомпилированной → отказ
   (`DB_SCHEMA_TOO_NEW`) с инструкцией обновить лаунчер.
4. Если есть неприменённые → `VACUUM INTO` бэкап
   `<data>/launcher.db.pre-migration-<version>-<timestamp>.bak`.
5. Применить миграции; публиковать `db:migration-progress`.
6. `PRAGMA quick_check`; при ошибке — не запускать приложение, показать
   восстановление из бэкапа.
7. Записать `app_meta.last_opened_by`, `last_integrity_check`.
8. Удалить старые pre-migration бэкапы, оставить 2 последних.

### 7.3. Разрушающие изменения

Пример: переименование таблицы/колонки.

1. Создать новую таблицу.
2. Скопировать данные в той же транзакции.
3. Удалить старую.
4. Пересоздать индексы.
5. Обновить тесты upgrade-фикстуры.

Удаление пользовательских данных в миграции запрещено. Если данные не нужны
приложению, они остаются в БД до явной очистки.

### 7.4. Тесты миграций

- `fresh`: пустая БД → все миграции → `quick_check` → проверка таблиц/индексов.
- `upgrade`: фикстура `launcher-v<N>.db` → миграции → сверка ключевых строк.
- `too_new`: БД с версией N+1 → `DB_SCHEMA_TOO_NEW`.
- `checksum`: изменённая миграция → внятная ошибка.
- `backup`: бэкап создаётся до применения и восстанавливается.
- `foreign_keys`: каскады работают после каждой миграции.

Фикстуры БД хранятся в `tests/fixtures/db/` и генерируются скриптом, а не
вручную.

### 7.5. Миграции манифестов

`instance.json`, `server.json`, `instance.lock.json`, `server.lock.json`
версионируются отдельно (`schema_version`). Правила:

- миграция выполняется в памяти при чтении;
- перед записью создаётся `.bak`;
- более новая версия → read-only, `INSTANCE_SCHEMA_TOO_NEW` /
  `SERVER_SCHEMA_TOO_NEW`;
- неизвестные поля сохраняются;
- миграция покрыта fixture-тестом «vN → vN+1 → round-trip».

---

## 8. Восстановление и обслуживание

### 8.1. Проверка целостности

- При старте: `PRAGMA quick_check` (быстро).
- По запросу диагностики: `PRAGMA integrity_check` (полная, может быть
  долгой).
- При ошибке: БД не удаляется. Переименовать в
  `launcher.db.corrupt-<timestamp>`, восстановить последний бэкап, показать
  отчёт. Пользовательские файлы инстансов не затрагиваются.

### 8.2. Перестройка индекса

Если БД потеряна, но каталоги целы:

1. Создать пустую БД и применить миграции.
2. Просканировать `instances/*/instance.json` и `servers/*/server.json`.
3. Прочитать lock-файлы и заполнить `instance_files`/`mod_metadata`.
4. Восстановить `download_tasks` как `cancelled`, `jobs` как `interrupted`.
5. Сверить `nexus_sources` и `installed_builds` по манифестам.
6. Отметить отсутствующие каталоги как `lost` и показать отчёт, не удаляя
   записи автоматически.

БД не содержит единственных копий пользовательских данных, поэтому эта
процедура допустима.

### 8.3. Retention

| Данные | Политика |
|---|---|
| `activity_logs` | 90 дней или 20 000 строк |
| `jobs` (завершённые) | 30 дней или 5 000 строк |
| `download_tasks` (завершённые) | 7 дней |
| `download_files` | удаляются вместе с задачей |
| `server_processes` | 90 дней |
| `crash_reports` | индекс чистится, если файл удалён |
| `backups` | политика Backup Manager, ≥ 1 валидный |
| `pre-migration` бэкапы | 2 последних |
| Логи | ротация по дням/объёму |

Очистка выполняется фоновой задачей при старте (не в горячем пути), в одной
транзакции на таблицу.

### 8.4. Бэкап перед рискованными операциями

Обязательный бэкап БД:

- перед миграцией;
- перед restore бэкапа инстанса/сервера (дополнительно к pre-restore snapshot);
- перед обновлением NEXUS-сборки (файлы + lock, не вся БД).

---

## 9. Безопасность данных

- В БД нет токенов, паролей, кодов авторизации.
- `activity_logs.details_json` не содержит абсолютных путей и тел HTTP.
- SQL формируется только через параметры SQLx; конкатенация значений
  запрещена.
- Имена таблиц/колонок не приходят из UI.
- `PRAGMA trusted_schema = OFF` снижает риск вредоносных schema-объектов.
- Файл БД не шифруется: он не содержит секретов, а шифрование SQLite
  добавило бы зависимость без выигрыша (ADR-005).
- Доступ к `launcher.db` наследуется от `%LOCALAPPDATA%` пользователя.

---

## 10. Открытые вопросы

| № | Вопрос | Решение |
|---|---|---|
| DB-Q1 | Нужна ли таблица `auth_providers` | Нет в v1 (ADR-023) |
| DB-Q2 | Нужна ли таблица `auth_sessions` | Нет: токены в keyring, метаданные в `accounts` |
| DB-Q3 | Нужна ли FTS для поиска модов | Нет: поиск у провайдера; локально — по `path` |
| DB-Q4 | Нужна ли таблица `instance_backups` | Нет: `backups` покрывает |
| DB-Q5 | Шифрование БД | Нет (ADR-005) |
| DB-Q6 | Хранение server world seed | Не нужно: `server.properties` — источник истины |
