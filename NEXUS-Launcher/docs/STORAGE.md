# Локальная инфраструктура хранения NEXUS-launcher

**Статус:** реализовано.  
**Код:** `crates/nexus-core`, `crates/nexus-platform`, `src-tauri`.  
**Миграции:** `migrations/0001_core.sql`, `0002_content_downloads.sql`,
`0003_indexes_recovery.sql`.

---

## 1. Состав

| Модуль | Ответственность |
|---|---|
| `nexus-core::error` | `NexusError`, стабильные `ErrorCode`, IPC-конверт `ErrorDto` |
| `nexus-core::paths` | `DataPaths`, проверка абсолютных путей, атомарная запись, `.bak`/карантин файлов |
| `nexus-core::config` | `config.toml` (`schema_version = 1`), значения по умолчанию, сохранение с резервной копией |
| `nexus-core::settings` | типизированные настройки, `SettingsPatch`, валидация, известные ключи |
| `nexus-platform::db` | пул SQLite, PRAGMA, миграции, проверка версии схемы, `quick_check`, бэкапы, восстановление после сбоя |
| `nexus-platform::settings` | чтение/запись `app_settings`, транзакции, события |
| `nexus-platform::security` | `SecretStore` (Windows Credential Manager / memory для тестов), redaction |
| `nexus-platform::repositories` | явные запросы для instances, mods, accounts, downloads, sources, activity |
| `src-tauri` | команды `app_*` и `settings_*`, состояние приложения, graceful shutdown |

---

## 2. Порядок запуска

1. `DataPaths::resolve_default()` — `%LOCALAPPDATA%\NEXUS-Launcher` или
   `NEXUS_LAUNCHER_DATA_DIR` для dev/тестов.
2. `ensure_layout()` — создаёт каталоги, не удаляя существующие файлы.
3. `config::load_or_init()` — читает/создаёт `config.toml`; битый файл
   копируется в `config.toml.corrupt-<timestamp>`, после чего пишутся defaults.
4. `logging::init()` — файловые логи в `logs\`, ошибка логирования не блокирует
   запуск.
5. `Db::open_initialized()`:
   - открывает пул (WAL, `foreign_keys = ON`, `busy_timeout = 5000`,
     `trusted_schema = OFF`, `max_connections = 8`);
   - проверяет, что версия `_sqlx_migrations` не больше версии в бинарнике
     (`DB_SCHEMA_TOO_NEW`);
   - при наличии пользовательских данных делает `VACUUM INTO`-бэкап перед
     миграцией;
   - применяет SQLx-миграции и выполняет `PRAGMA quick_check`;
   - при `db.clean_shutdown != '1'` помечает `running` загрузки как `paused`,
     пишет запись в `activity_logs`;
   - обновляет метаданные `app_meta` (`last_opened_by`,
     `last_integrity_check`);
   - возвращает `InitReport` с версией схемы, бэкапом и отчётом recovery.
6. `SettingsService` читает `app_settings` поверх слоя из `config.toml`.
7. При выходе `Db::shutdown()` ставит `db.clean_shutdown = 1` и закрывает пул.

---

## 3. Схема данных

Все таблицы созданы в `STRICT`, идентификаторы — UUID v4 (`TEXT`), время —
Unix-миллисекунды (`INTEGER`).

| Таблица | Ключ | Особенности |
|---|---|---|
| `instances` | `id` (UUID) | `archived_at`, `deleted_at`, уникальное имя среди активных, FK на `java_runtimes` (`SET NULL`) |
| `instance_mods` | `id` (UUID) | FK на `instances` (`CASCADE`), уникальный `path_key` в рамках инстанса, soft delete |
| `accounts` | `id` (UUID) | только `secret_ref`, уникальность `(provider, provider_account_id)` среди активных, offline без секрета |
| `java_runtimes` | `id` (UUID) | уникальный путь runtime среди активных, `source`, `verified_at` |
| `download_tasks` | `id` (UUID) | очередь/статусы, FK на файлы |
| `download_files` | `id` (UUID) | FK на задачу (`CASCADE`), `root_kind + rel_path`, уникальность `(task_id, path_key)` |
| `servers` | `id` (UUID) | автономный экземпляр, FK на `java_runtimes` (`SET NULL`), soft delete |
| `backups` | `id` (UUID) | индекс архивов, `target_kind/target_id`, soft delete |
| `nexus_sources` | `id` (UUID) | FK от `installed_builds` (`RESTRICT`), `secret_ref`, soft delete |
| `installed_builds` | `id` (UUID) | один активный build на instance, FK на source (`RESTRICT`) |
| `app_settings` | `key` | типизированный JSON (`json_valid`), `created_at`, `updated_at` |
| `activity_logs` | `id` (INTEGER) | стабильный autoincrement id, redaction details, retention |
| `app_meta` | `key` | служебные метаданные, `db.clean_shutdown`, версии схем |

Дополнительно создано представление `v_accounts_safe` без `secret_ref`: в нём
есть только `has_secret`, поэтому UI не получает даже имя записи keyring.

Каскады:

- `instances` → `instance_mods`, `installed_builds` (`CASCADE`);
- `download_tasks` → `download_files` (`CASCADE`);
- `java_runtimes` → `instances/servers.java_runtime_id` (`SET NULL`);
- `nexus_sources` нельзя удалить, пока есть `installed_builds` (`RESTRICT`);
- для основных сущностей используется `deleted_at` (soft delete) и частичные
  уникальные индексы `WHERE deleted_at IS NULL`.

---

## 4. Секреты

- В SQLite нет колонок `access_token`, `refresh_token`, `password`.
- `accounts.secret_ref` и `nexus_sources.secret_ref` — только ссылки вида
  `nexus-launcher/account/<uuid>`.
- `validate_secret_ref` запрещает записывать JWT/произвольную строку в
  `secret_ref`.
- `KeyringSecretStore` использует Windows Credential Manager; при недоступности
  хранилища операция завершается `SECRET_STORE_UNAVAILABLE`, plaintext-fallback
  отсутствует.
- `sanitize_details` вычищает токены/пароли из `activity_logs.details_json`;
  `redact_text` маскирует `key=value`-секреты в summary и диагностике.

---

## 5. Настройки

Ключи `app_settings`:

| Ключ | Тип | Диапазон / значения |
|---|---|---|
| `paths.game_directory` | string/null | абсолютный путь; пустая строка очищает |
| `paths.java_directory` | string/null | абсолютный путь; пустая строка очищает |
| `ui.language` | enum | `ru`, `en` |
| `ui.theme` | enum | `dark`, `light`, `system` |
| `ui.background_quality` | enum | `off`, `low`, `medium`, `high` |
| `ui.background_fps` | integer | `0..240` (`0` — без ограничения) |
| `downloads.max_concurrent` | integer | `1..16` |
| `downloads.max_speed_kbps` | integer | `0..1 000 000` (`0` — без ограничения) |
| `logging.enabled` | bool | — |
| `logging.level` | enum | `error`, `warn`, `info`, `debug`, `trace` |

Приоритет: `defaults → config.toml → app_settings`. Неизвестные ключи в
`app_settings` сохраняются и не удаляются при частичном обновлении или сбросе
группы.

Повреждённое значение (неверный тип, невалидный путь) не блокирует запуск:
сервис пишет warning и использует значение нижнего слоя.

---

## 6. Tauri-команды

| Команда | Аргументы | Результат |
|---|---|---|
| `app_info` | — | версия, версия IPC/БД, каталог данных, recovery-флаг |
| `app_bootstrap` | — | `AppInfo + settings + db health + recovery + migration` |
| `settings_get` | — | `EffectiveSettings` |
| `settings_update` | `{ patch: SettingsPatch }` | `EffectiveSettings`, событие `app:settings-changed` |
| `settings_reset_group` | `{ group: "paths"\|"ui"\|"downloads"\|"logging" }` | `EffectiveSettings` |
| `db_health` | — | `journal_mode`, `foreign_keys`, `schema_version`, `integrity` |

Все ошибки возвращаются в едином конверте `ErrorDto`
(`code`, `message`, `details`, `retryable`, `requestId`).

---

## 7. Резервирование и восстановление

- `config.toml` → `config.toml.bak` перед каждой записью; невалидный файл
  копируется в `config.toml.corrupt-<timestamp>`.
- Перед миграцией существующей БД: `launcher.db.pre-migration-v<N>-<ts>-<uuid>.bak`
  через `VACUUM INTO`; хранятся 2 последних бэкапа.
- `Db::backup_snapshot(dir, tag)` — ручной консистентный снимок.
- Неожиданное завершение определяется по `app_meta.db.clean_shutdown`: при
  следующем старте `running`-загрузки становятся `paused`, событие пишется в
  `activity_logs`, приложение продолжает работу с сохранёнными данными.
- Более новая схема (`_sqlx_migrations.version > compiled`) не перезаписывается:
  возвращается `DB_SCHEMA_TOO_NEW`, файл БД остаётся на месте.

---

## 8. Тесты

Запуск:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Покрыто:

- unit: коды ошибок, пути, TOML round-trip с неизвестными полями, карантин
  битого конфига, `.bak`, настройки/валидация, redaction, secret refs;
- integration (`crates/nexus-platform/tests/storage_integration.rs`):
  первичная инициализация, наличие всех таблиц/индексов, WAL + `foreign_keys`,
  сохранение настроек после перезапуска, повторная миграция без потери данных,
  уникальные ограничения, soft delete, каскады/`RESTRICT`, `DB_SCHEMA_TOO_NEW`,
  recovery после убийства процесса, pre-migration бэкап, отсутствие токена в
  `.db`/`-wal`, redaction activity, сброс группы настроек, приоритет
  `config.toml → app_settings`.

Текущий результат: **55 тестов**, `0 failed`; `clippy -D warnings` и
`cargo fmt --check` проходят.
