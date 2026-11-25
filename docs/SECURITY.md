# Безопасность NEXUS WORKSPACE

Документ описывает модель угроз и **фактические** механизмы защиты в коде на
19.11.2026. Требования — `../MAIN_PLAN.md` §12; остаточные задачи и принятые
ограничения — `../COMPLETE.md`, разделы 15 ниже.

Обозначения:

* ✅ — реализовано в коде и покрыто тестом или проверкой;
* ⏳ — не реализовано полностью, указана задача и фаза;
* ⚠ — осознанное ограничение или остаточный риск.

Связанные документы: [`ARCHITECTURE.md`](ARCHITECTURE.md),
[`API.md`](API.md), [`MANIFEST.md`](MANIFEST.md).

---

## 1. Что защищается

| Актив | Где хранится | Почему важен |
|---|---|---|
| Учётные записи и пароли | `users`, PostgreSQL | доступ ко всем рабочим пространствам |
| Сессии браузера | cookie `nexus_session` + `sessions` | доступ без повторного входа |
| Токены лаунчера | `access_tokens`, только SHA-256 | доступ к каталогу и артефактам |
| Файлы Cloud | тома `.nexus/objects` | данные пользователей |
| Артефакты сборок | тома + `storage_objects` | целостность релизов |
| Манифесты и релизы | `minecraft_releases`, `minecraft_release_files` | то, что устанавливает лаунчер |
| Ссылки общего доступа | `cloud_shares`, только SHA-256 токена | доступ к выбранному файлу |
| Настройки и модули | `system_settings`, `app_*` | поведение и доступность системы |
| Журналы и аудит | `audit_events`, файлы логов | расследование инцидентов |
| Секреты конфигурации | `.env`, `nexus.toml`, окружение | подпись CSRF, доступ к БД |

### 1.1. Акторы и границы доверия

```text
Интернет
  │  HTTPS
  ▼
Caddy  ← доверенная граница; доверяет X-Forwarded-* только при trust_proxy
  │  HTTP + WebSocket, 127.0.0.1
  ▼
nexus serve (Rust)
  ├── PostgreSQL (локальная сеть, не из интернета)
  ├── тома хранения (локальные каталоги)
  └── frontend/dist (статические файлы)
```

* **Гость** — без сессии; видит только вход, health, OpenAPI и публичные
  ссылки.
* **Пользователь** — активная учётная запись с ролями и правами.
* **Администратор** — `is_superadmin` или роль с `system.*` правами.
* **Лаунчер** — владелец Bearer-токена с ограниченными scopes.
* **Локальный пользователь ОС** — вне модели: он может читать файлы процесса,
  если не настроены права ОС и `ProtectSystem`. Это зона ответственности
  администратора (раздел 8).

---

## 2. Модель угроз

| # | Угроза | Вектор | Последствие | Мера | Статус |
|---|---|---|---|---|---|
| T1 | Подбор пароля | `/auth/login`, `/launcher/auth/token` | захват аккаунта | Argon2id, лимитер, блокировка, аудит | ✅ |
| T2 | Кража сессии | XSS, небезопасная cookie, логи | доступ к аккаунту | HttpOnly/SameSite/Secure, хэш токена, CSP, revoke | ✅ |
| T3 | CSRF | сторонний сайт с cookie жертвы | действия от имени жертвы | HMAC-токен + обязательный заголовок | ✅ |
| T4 | Обход прав | прямой вызов API, смена ID | доступ к чужим данным | серверный RBAC, проверка владельца, гейт модулей | ✅ |
| T5 | SQL-инъекция | параметры запросов | чтение/изменение БД | только bind-параметры SQLx | ✅ |
| T6 | Path traversal | имя/путь файла | выход за каталог хранения | нормализация, `resolve_within`, канонизация | ✅ |
| T7 | Symlink/junction | ссылка в томе | чтение/запись вне тома | `ensure_no_links`, reparse points | ✅ |
| T8 | Огромный запрос | тело/чанк | OOM, отказ сервиса | лимиты тела, чанка, файла, квоты | ✅ |
| T9 | Подмена типа файла | `Content-Type`/расширение | XSS, исполнение | MIME по сигнатуре, allowlist предпросмотра, `nosniff`, иконки по сигнатуре | ✅ H1 |
| T10 | XSS через UI | имена файлов, changelog | кража сессии | React-экранирование, CSP, нет raw HTML | ✅ |
| T11 | Утечка секретов | сервер, лог, ответ API | полный компромисс | `.env.example` без секретов, безопасные ошибки, минимум в логах | ✅ |
| T12 | Перегрузка API | flood запросов | отказ сервиса | лимиты входа и лаунчера | ⚠ нет глобального IP-лимита |
| T13 | Подделка IP | `X-Forwarded-For` | обход лимитов, ложный аудит | `trust_proxy` только за прокси | ✅ |
| T14 | Повреждение объекта | сбой диска/БД | потеря данных | staging, SHA-256, quarantine, ref_count | ✅ |
| T15 | Утечка приватной сборки | каталог/манифест/артефакт | нарушение доступа | `owner_id`, фильтр `public or owner`, 404 на чужих | ✅ F4 |
| T16 | Компрометация хоста/БД | root/SQL superuser | полный доступ | вне модели приложения; меры ОС/БД | ⚠ |

Полный чек-лист §12 с привязкой к коду и тестам — в разделе 12. Статусы
✅ в таблице угроз — сводка; файлы реализации и проверки перечислены там же.

---

## 3. Аутентификация

### 3.1. Пароли

`backend/src/auth/password.rs`:

* **Argon2id** с параметрами библиотеки `argon2` по умолчанию: Argon2id
  v19, память 19 МиБ, 2 итерации, 1 поток, соль 16 байт из `OsRng`;
  в БД хранится PHC-строка (`$argon2id$...`), не пароль.
* Хеширование и проверка выполняются в `spawn_blocking`, чтобы не блокировать
  async-runtime.
* Политика: эффективный минимум из `security.password_min_length` (8–64,
  по умолчанию 12), максимум 128 символов (считаются Unicode-символы),
  обязательны буква и цифра. Имя пользователя: 3–32 символа, `[A-Za-z0-9._-]`.
* Для несуществующего пользователя проверяется фиктивный хеш, чтобы время
  ответа не выдавало существование логина.
* Смена пароля инвалидирует остальные сессии пользователя.
* CLI `create-admin` берёт минимум из конфига (`password_min_length`,
  `NEXUS_PASSWORD_MIN_LENGTH`), по умолчанию 12.

### 3.2. Сессии браузера

`backend/src/auth/session.rs`:

* Токен — 32 случайных байта (`rand::thread_rng`), hex; в БД хранится только
  SHA-256 (`sessions.token_hash`).
* Cookie `nexus_session`:
  `Path=/; HttpOnly; SameSite=Lax; Max-Age=<TTL>`; `Secure` добавляется при
  `NEXUS_COOKIE_SECURE=true` (обязательно для HTTPS).
* TTL — `security.session_ttl_hours` (1–720, по умолчанию 24); сессия
  скользяще продлевается при каждом запросе.
* При каждом запросе проверяются: наличие сессии, `revoked_at`, срок,
  `users.status = active`, `locked_until`; права загружаются заново, поэтому
  изменение ролей действует без перелогина.
* Отзыв: `POST /auth/logout`, `POST /auth/sessions/revoke-all`,
  `POST /users/{id}/sessions/revoke`; отключение пользователя и сброс пароля
  также отзывают сессии.
* TTL и порог блокировки читаются из `security.session_ttl_hours` и
  `security.login_max_failures` через кэш настроек; `LoginLimiter` (5/15 мин)
  остаётся независимой защитой от перебора.

### 3.3. Токены лаунчера

`backend/src/launcher/tokens.rs`:

* Формат `nxl_<64 hex>`, 32 случайных байта; в БД только SHA-256.
* Привязка к пользователю, scopes `launcher.catalog` и `launcher.artifacts`,
  срок 1–8760 часов (по умолчанию 30 дней), `revoked_at`, `last_used_at`
  (обновляется не чаще раза в минуту).
* Передаётся в `Authorization: Bearer …` или `X-Launcher-Token`.
* Выдача токена требует логин/пароль, проверяет статус и блокировку,
  ограничена тем же `LoginLimiter` (ключ `launcher-token:<username>`).
* Отзыв: `POST /launcher/auth/revoke` (свой токен),
  `DELETE /admin/launcher/tokens/{id}` (администратор).

---

## 4. CSRF

`backend/src/middleware.rs`, `backend/src/auth/session.rs`:

* CSRF-токен = `HMAC-SHA256(NEXUS_SECRET, "csrf:" + session_token)`.
* Для всех изменяющих методов (`POST`, `PUT`, `PATCH`, `DELETE`) обязательны
  cookie сессии и заголовок `X-CSRF-Token`; сравнение — constant-time
  (`subtle::ConstantTimeEq`).
* Исключения: `GET/HEAD/OPTIONS/TRACE`, `/auth/login`,
  `/auth/setup-status`, `/health`, `/launcher/auth/token`,
  `/launcher/auth/revoke`. Исключения не используют cookie-сессию для
  изменения состояния: логин проверяет пароль, лаунчер — Bearer-токен.
* Ошибка — `403 CSRF_INVALID` с безопасным сообщением.
* `NEXUS_SECRET` должен быть стабильным между перезапусками; иначе старые
  CSRF-токены перестают совпадать (пользователю нужно обновить страницу).

---

## 5. Brute-force и ограничение частоты

`backend/src/auth/rate_limit.rs`, `backend/src/auth/routes.rs`,
`backend/src/launcher/routes.rs`:

| Механизм | Параметры | Что защищает |
|---|---|---|
| `LoginLimiter` (память) | 5 неудач за 15 мин → блок 15 мин; ключ `IP\|lower(username)` | вход и выдача токена лаунчера |
| Блокировка аккаунта (БД) | 10 неудач → `locked_until = now() + 15 min` | подбор пароля с разных IP |
| Успешный вход | сбрасывает счётчики и блокировку | ложные срабатывания |
| Launcher token bucket (каталог) | всплеск 120, 120 запросов/мин на токен (настраивается) | перегрузка каталога |
| Launcher token bucket (артефакты) | всплеск 600, 600 запросов/мин на токен (настраивается) | перегрузка скачиваний |
| Параллельные скачивания | 8 потоков на токен (настраивается) | перегрузка диска и сети |

* При превышении — `429 RATE_LIMITED`, `Retry-After` и `X-RateLimit-*`;
  исчерпание параллельных слотов — `503 DOWNLOAD_LIMIT_REACHED` с
  `Retry-After`. Слот освобождается в `Drop` тела ответа, в том числе при
  обрыве клиента.
* Серии из 10+ превышений по токену пишутся в журнал предупреждением.
* Неудачные входы пишутся в `audit_events` с причиной (`unknown_user`,
  `bad_password`, `account_disabled`), числом попыток и IP; пароль не
  логируется.
* ⚠ Лимитеры in-memory и не разделяются между процессами. При нескольких
  экземплярах нужен внешний лимитер; текущая модель — один процесс.
* ⚠ Глобального лимита на все API по IP нет; ограничены вход, выдача
  токенов и Launcher API.
* ⚠ Пароль публичной ссылки (`cloud_shares.password_hash`) проверяется
  Argon2id, но отдельного лимитера попыток для публичных ссылок пока нет.

---

## 6. Авторизация и RBAC

`backend/src/permissions/catalog.rs`, `backend/src/auth/session.rs`,
`backend/src/middleware.rs`:

* Каталог из **20 прав** по модулям:
  * `minecraft.view`, `minecraft.build.create/edit/publish/delete`,
    `minecraft.artifacts.upload`;
  * `launcher.manage`;
  * `cloud.view`, `cloud.file.upload/download/delete/share`,
    `cloud.folder.create`, `cloud.storage.manage`;
  * `system.view`, `system.users.manage`, `system.storage.manage`,
    `system.settings.manage`, `system.monitoring.view`,
    `system.jobs.manage`.
* `is_superadmin` получает все права; иначе права = объединение прав ролей и
  персональных grants, из которого вычитаются персональные `granted = false`.
  Запрет имеет приоритет.
* Каждый защищённый обработчик извлекает `AuthUser`; операции, требующие
  права, вызывают `require(permission)`; лаунчер — `LauncherClient::require(scope)`.
* Дополнительно проверяется владелец ресурса в SQL (`owner_id = $user`), а для
  лаунчера — `(visibility = 'public' or owner_id = $viewer)`; чужой файл,
  сборка, манифест или артефакт отдаёт `404`, а не `403`, чтобы не раскрывать
  существование. Пути манифеста дополнительно проверяются на переносимость
  (Windows/UNC/ADS/NFC) и регистронезависимые коллизии.
* `module_guard` блокирует API отключённого модуля (`403 MODULE_DISABLED`);
  для лаунчера — `404`, чтобы не раскрывать состояние системы.
* Публичные маршруты: `/health`, `/auth/login`, `/auth/setup-status`,
  `/launcher/auth/token`, `/launcher/auth/revoke`, `/public/shares/{token}`,
  `/openapi.json` (Swagger UI выключен по умолчанию).
* Права проверяются на сервере; скрытие кнопок в UI — только удобство.
* Полная матрица «endpoint → право → негативный тест» не автоматизирована:
  `openapi_tests` гарантирует, что каждый из 103 путей описан, а
  представительные проверки прав и владельца покрыты `auth_tests`,
  `launcher_tests` и `cloud_tests`. Это ограничение зафиксировано в §12
  (пункт 6) и не выдаётся за полное покрытие.

---

## 7. Данные и файловая система

### 7.1. SQL

* Все запросы — параметризованные SQLx (`bind`).
* Динамические фильтры/сортировки выбираются из фиксированных `match`-веток,
  а не собираются из пользовательской строки.
* Ошибки БД не возвращаются клиенту: `ApiError::internal` логирует детали,
  наружу отдаёт `INTERNAL_ERROR`.

### 7.2. Path traversal

`backend/src/storage/paths.rs`:

* Пользователь передаёт только **относительный** путь; абсолютные пути,
  буквы дисков, UNC, `..`, обратные слэши и ADS (`file.txt:payload`)
  запрещены.
* Опасные имена Windows (`CON`, `NUL`, `COM1–9`, `LPT1–9`, завершающая
  точка/пробел) **отклоняются с `422 VALIDATION_ERROR`**, а не нормализуются
  молча. Регистронезависимая коллизия в одной папке даёт
  `409 FILE_NAME_CONFLICT` и не перезаписывает существующий объект.
* Полный виртуальный путь ограничен 240 UTF-16 единицами
  (`cloud/service.rs::PATH_LENGTH_MAX`); попытка создать более длинный путь
  даёт `422 PATH_TOO_LONG` с `maxLength`/`actualLength`, а не обрезание.
  Физические объекты лежат по коротким сгенерированным ключам.
* `normalize_relative` разбирает компоненты и отбрасывает `.`, но любой
  `ParentDir`/`RootDir`/`Prefix` — ошибка.
* `resolve_within` присоединяет путь к корню тома, проверяет компоненты,
  канонизирует существующий путь и требует, чтобы результат оставался внутри
  корня (`is_within`).
* Внутренние имена объектов на диске — сгенерированные ключи (SHA-256 +
  UUID), исходные имена хранятся в БД. Пользовательское имя никогда не
  становится путём на диске.
* Регистрация тома: только абсолютный существующий каталог, не корень диска,
  не системный каталог (`Windows`, `Program Files`, `/etc`, `/usr`, `/boot`,
  `/proc`, `/sys` и т.п.); выполняется проверка записи. Каталог, являющийся
  символической ссылкой или junction, не регистрируется.
* Скачивание отдаёт объект из `objects`, а не пользовательский путь.
* Проверки: `storage_tests::path_traversal_is_rejected`,
  `windows_invariants_tests::traversal_drive_unc_and_ads_are_rejected_with_422`,
  `windows_invariants_tests::case_insensitive_conflict_does_not_overwrite_physical_file`,
  `windows_invariants_tests::long_virtual_path_returns_clear_422_without_truncation`.

### 7.3. Symlink и junction

* `ensure_no_links` проходит **каждый** компонент пути от корня до цели через
  `symlink_metadata` и запрещает символические ссылки, junctions и любые
  reparse points Windows (`FILE_ATTRIBUTE_REPARSE_POINT`).
* Проверка выполняется при открытии объекта для чтения и при разрешении
  пути, а не только при загрузке. Нарушение возвращает `422` с безопасным
  сообщением (`storage/engine.rs::open_object`).
* `canonical_directory` не регистрирует том, если выбранный каталог сам
  является ссылкой/junction. `browse_directory` пропускает reparse points;
  рекурсивного обхода пользовательских каталогов в приложении нет (экспорт
  Cloud идёт по записям БД).
* Если ссылка появилась после проверки, канонизация и повторная проверка
  границ уменьшают окно, но не устраняют гонку полностью; запись в том
  должна быть доступна только процессу NEXUS (права ОС).
* Проверка: `windows_invariants_tests::reparse_points_cannot_escape_volume`
  создаёт реальные `mklink /J` и symlink `/D` за корень тома.

### 7.4. Временные каталоги и фиксация

* `staging/<session>.part` — незавершённые загрузки; `objects/…` — готовые
  объекты; `quarantine/` — объекты, которые не удалось согласовать с БД.
* Запись идёт потоком, без загрузки файла целиком в RAM; SHA-256 считается
  потоково.
* После успешной записи выполняется атомарный `rename` в пределах тома
  (fallback — копирование) и транзакция БД; при ошибке БД/транзакции объект
  перемещается в `quarantine`, а не считается готовым.
* `ref_count` и `storage_object_refs` не позволяют удалить объект, пока на
  него ссылаются файлы или снимки релизов.
* Просроченные staging-сессии и мусор удаляются задачами очистки; при
  аварийном завершении состояние согласуется проверкой томов.

### 7.5. Ограничение размеров

| Уровень | Фактическое значение | Где |
|---|---|---|
| Тело JSON API | 2 МБ | `build_router` (`DefaultBodyLimit`) |
| Тело загрузки Cloud/Minecraft | технический максимум 256 МБ | `lib.rs` (`UPLOAD_BODY_LIMIT`) |
| Чанк Cloud | `limits.chunk_max_mb` / `storage.chunk_max_bytes`, по умолчанию 64 МБ, максимум 256 МБ | `state.rs`, `cloud/uploads.rs` |
| Чанк Minecraft | `limits.chunk_max_mb` в обработчике потоковой записи | `minecraft/uploads.rs` |
| Файл Cloud | `storage.max_upload_bytes` / настройка, по умолчанию 10 ГБ | `cloud/uploads.rs` |
| Квота пользователя | `users.quota_bytes` / defaults | `cloud/quota.rs` |
| Иконка сборки | 2 МБ | `minecraft/builds.rs` |
| Папка Cloud | 2000 файлов, 100 уровней (проверка на клиенте до очереди, отказ без частичной загрузки; batch API — 200 имён) | `CloudStoragePage`, `cloudUpload.ts`, `cloud/entries/batch` |
| Загрузки | TTL незавершённой сессии | `storage.upload_ttl_hours` |

`NEXUS_MAX_UPLOAD_MB`, `limits.chunk_max_mb` и `limits.max_upload_mb`
валидируются при старте/сохранении; отрицательные и сверхбольшие значения
отклоняются.

---

## 8. Загружаемый контент и предпросмотр

### 8.1. Что реализовано

* Любой файл Cloud/Minecraft хранится как объект; исполнение файлов из
  хранилища невозможно — статика раздаётся только из `frontend/dist`.
* MIME определяется по первым 8192 байтам staging-файла при завершении
  загрузки (`storage/mime.rs`, crate `infer`). Результат сохраняется в
  `storage_objects.content_type` и `cloud_entries.content_type`.
* `Content-Type` клиента **не используется** для хранения и предпросмотра:
  подмена заголовка или расширения не меняет тип. Для текста без сигнатуры
  определяется `text/plain`, HTML — `text/html`, остальное без сигнатуры —
  `application/octet-stream`.
* Скачивание по умолчанию — `application/octet-stream` и
  `Content-Disposition: attachment` с ASCII-fallback и `filename*=UTF-8''`
  (RFC 5987). Inline-ответы также содержат ASCII-fallback и `filename*`,
  поэтому Edge/Chrome показывают исходное не-ASCII имя.
* Inline-предпросмотр разрешён только по allowlist (`cloud/files.rs::
  preview_content_type`): PNG/JPEG/WEBP/GIF/BMP/AVIF, MP4/WEBM,
  MP3/WAV/OGG/FLAC, PDF, а также `text/plain`, `text/markdown`, CSV, JSON,
  XML и код. Код, XML и прочий текст отдаются только как
  `text/plain; charset=utf-8`.
* HTML, XHTML и SVG **никогда не отдаются inline** (по сохранённому MIME и по
  расширению): ответ получает `application/octet-stream` и
  `Content-Disposition: attachment`. JS отдаётся как `text/plain`.
  Остальные неизвестные типы также скачиваются.
* Для inline-ответов и всех файловых ответов выставляется
  `X-Content-Type-Options: nosniff`.
* Иконка сборки проверяется по сигнатуре (PNG/JPEG/WEBP) и размеру 2 МБ;
  `Content-Type` клиента для неё не используется.
* Имя файла при скачивании очищается: ASCII-часть заменяет недопустимые
  символы, полное имя кодируется в `filename*`.
* На Windows чтение staging-файла для определения MIME и его перемещение в
  объектное хранилище повторяются с паузой при
  `ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION`/`PermissionDenied`:
  антивирус или индексатор может кратковременно удерживать только что
  записанный файл.

### 8.2. Совместимость и остаточный риск

Для объектов, загруженных до появления H1 и хранящих
`application/octet-stream`, предпросмотр использует консервативный allowlist
по расширению. Новые загрузки этим не пользуются.

Остаточный риск T9 снижен, но не устранён полностью: содержимое файла не
проверяется антивирусом, а тип определяется по сигнатуре, а не полным
разбором формата. Битый файл с валидной сигнатурой будет отдан с типом этого
формата; `nosniff` и CSP ограничивают последствия.

**Windows Defender:** при активной защите в реальном времени возможны две
ситуации:

1. файл кратковременно удерживается после записи — приложение повторяет
   чтение/перемещение с backoff, а не падает сразу;
2. Defender может **удалить** загруженный файл (ложное срабатывание), после
   чего завершение вернёт `UPLOAD_DATA_MISSING`, а уже сохранённый объект
   станет недоступен.

Для хостов с большим потоком загрузок администратор может добавить корни
томов хранения в исключения Defender. Это ослабляет защиту: исключённые
каталоги не сканируются, поэтому вредоносный файл, загруженный пользователем,
не будет обнаружен антивирусом. Исключения допустимы только для выделенных
томов NEXUS, не для системного диска и не для каталога установки с
`frontend/dist`, и только при наличии внешнего сканирования или доверия к
кругу пользователей. Команды и оговорки — в
[`DEPLOYMENT.md`](DEPLOYMENT.md#45-windows-defender-и-каталоги-хранения).

---

## 9. HTTP-заголовки, CSP, CORS и XSS

`backend/src/lib.rs`:

```text
Content-Security-Policy:
  default-src 'self';
  img-src 'self' data: blob:;
  media-src 'self' blob:;
  style-src 'self' 'unsafe-inline';
  script-src 'self';
  connect-src 'self' ws: wss:;
  font-src 'self' data:;
  object-src 'none';
  frame-src 'none';
  frame-ancestors 'none';
  base-uri 'self';
  form-action 'self';
  worker-src 'self' blob:
```

Дополнительно на всех ответах:

| Заголовок | Значение |
|---|---|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` |
| `X-Request-Id` | принятый или сгенерированный идентификатор |

* `frame-ancestors 'none'` и `X-Frame-Options: DENY` запрещают встраивание
  интерфейса.
* `script-src 'self'` без `unsafe-inline`/`unsafe-eval`; приложение не
  использует `dangerouslySetInnerHTML`, `innerHTML`, `eval` и `new Function`.
* `style-src 'unsafe-inline'` оставлен для inline-стилей UI; это не даёт
  исполнять скрипты.
* CORS: если `security.allowed_origins` не пуст, разрешены только точные
  origins, с credentials, методами `GET/POST/PUT/PATCH/DELETE/OPTIONS` и
  заголовками `Content-Type`, `Accept`, `Authorization`, `X-CSRF-Token`,
  `X-Request-Id`. Пустой список — same-origin, заголовки CORS не отдаются.
  Список собирается при старте: `serve()` переопределяет
  `Config.security.allowed_origins` значением `network.allowed_origins` из
  `system_settings` (B5), поэтому изменение через API вступает в силу только
  после перезапуска.
* HSTS настроен в `deployment/Caddyfile.windows`
  (`max-age=31536000; includeSubDomains`); Caddy выдаёт HTTPS автоматически.
  Приложение само HSTS не добавляет — это задача reverse proxy.
* Swagger UI `/api/v1/docs` выключен, если `NEXUS_ENABLE_DOCS` не равен
  `1/true/yes`; `openapi.json` публичен и не содержит секретов.
* WebSocket `/api/v1/ws` авторизуется cookie-сессией; события адресуются
  `user_id`, системные — только при `system.monitoring.view`; на пользователя
  допускается не более 3 соединений; медленный клиент теряет старые события,
  но не блокирует остальных.

**Финальная CSP (H2) зафиксирована:** `frame-src 'none'`, `object-src 'none'`,
`worker-src 'self' blob:`, `script-src 'self'` без `unsafe-inline`/`unsafe-eval`.
PDF.js работает: worker `pdf.worker.min.mjs` собирается Vite в `dist/assets` и
загружается с того же origin; PDF рендерится в `<canvas>`, без `iframe` и
`blob:`-фреймов.

Заголовки CSP, `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`
и `Permissions-Policy` проверяются интеграционным тестом
`backend/tests/security_headers_tests.rs` на успешных и ошибочных ответах.

⏳ **E2E-проверка консоли (I1):** Playwright в проекте ещё отсутствует.
Сценарий «открыть все страницы и PDF, убедиться в отсутствии CSP-ошибок в
консоли» зафиксирован как отложенный в [`TESTING.md`](TESTING.md).

---

## 10. Секреты

| Секрет | Где задаётся | Правило |
|---|---|---|
| `DATABASE_URL` | `.env` / окружение / `nexus.toml` | не коммитить; отдельный пользователь БД с минимальными правами |
| `NEXUS_SECRET` | `.env` / `nexus.toml` | ≥ 32 символов, случайный, стабильный; подпись CSRF |
| Пароли пользователей | БД, Argon2id | не логировать, не возвращать |
| Токены сессий | cookie + SHA-256 в БД | не логировать |
| Токены лаунчера | SHA-256 в `access_tokens` | показывать один раз при выдаче |
| Токены публичных ссылок | SHA-256 в `cloud_shares` | в URL, не логировать в идеале (см. ниже) |
| Пароли публичных ссылок | Argon2id | минимум 4 символа; не логировать |

* `.env`, `nexus.toml`, `*.pem`, `*.key` хранятся только на сервере и не
  входят в release-архив; `.env.example` содержит только placeholder `change-me`.
* Если `NEXUS_SECRET` не задан, при старте генерируется временный секрет и
  выводится предупреждение; для production это запрещено.
* `nexus check --json` считает проблемой короткий секрет и значение
  `change-me…`, а также `https` в `public_url` при выключенном
  `NEXUS_COOKIE_SECURE`.
* В production секреты передаются через Windows Service с правами только для
  служебной учётной записи или через `.env` установки, а не через командную
  строку.
* Смену `NEXUS_SECRET` и пароля БД следует выполнять по процедуре раздела 12
  с перезапуском; смена секрета инвалидирует CSRF-токены, но не сессии —
  сессии отзываются отдельно.

---

## 11. Журналирование и аудит

### 11.1. Что пишется

* `tracing` + `tower_http::trace`: метод, путь, статус, длительность;
  тела запросов/ответов и заголовки `Authorization`/`Cookie` не пишутся.
* `ApiError::internal` пишет техническую причину в журнал, пользователю
  отдаёт общий код и `request_id`.
* `audit_events`: вход/выход, смена пароля, отзыв сессий, действия с
  пользователями, ролями, томами, настройками, файлами, ссылками, сборками,
  релизами, токенами, задачами. Поля: тип, актор, цель, результат, IP,
  детали (`details`).
* В аудит не попадают пароли, хеши паролей, токены сессий/лаунчера,
  `NEXUS_SECRET`, `DATABASE_URL`, содержимое файлов.
* User-Agent усечён до 400 символов; IP берётся из peer или доверенных
  `X-Forwarded-For`/`X-Real-IP` только при `trust_proxy=true`.
* Файловый журнал ротируется при старте, хранится `NEXUS_LOG_KEEP` файлов
  (по умолчанию 7).

### 11.2. Ограничение

⚠ Публичные ссылки имеют вид `/s/{token}`, и reverse proxy/access log может
записать токен в URL. Журналы NEXUS и Caddy следует считать
конфиденциальными: ограничить права, не публиковать, очищать по политике.
Маскирование токенов в access log — задача эксплуатации (или отдельное
улучшение); токен можно отозвать.

### 11.3. Что проверять

* серии `auth.login.failed` и `ACCOUNT_LOCKED`;
* `RATE_LIMITED` и предупреждения о систематическом превышении лимита
  лаунчера;
* `403 CSRF_INVALID`/`FORBIDDEN` в большом количестве;
* рост 5xx и `INTERNAL_ERROR` с одним `request_id`;
* `storage_objects.state = quarantined`, `missing`, ошибки томов;
* изменения ролей, прав и настроек;
* выдачу/отзыв токенов лаунчера и публичных ссылок;
* аномальный объём скачиваний и число активных сессий.

Аудит хранится в БД; политика retention пока не автоматизирована —
администратор настраивает backup и очистку по требованиям организации.

---

## 12. Чек-лист §12 (H3): реализация и проверка

Каждый пункт содержит **файл с реализацией** и **тест либо явное
отклонение**. Галочка без ссылки на код не ставится. Тесты, помеченные
`windows_invariants_tests`, реально прогоняются на Windows 11 / NTFS
(см. [`TESTING.md`](TESTING.md#6-windowsntfs-инварианты-h4)).

| # | Требование | Код | Тест / проверка | Статус |
|---|---|---|---|---|
| 1 | Argon2id для паролей | `auth/password.rs` (`hash_password`, `verify_password`) | `auth_tests::password_hash_roundtrip` | ✅ |
| 2 | Cookie `HttpOnly`, `Secure`, `SameSite` | `auth/session.rs::session_cookie_header`, `clear_cookie_header` | `auth_tests::session_cookie_flags_are_http_only_and_same_site` | ✅ |
| 3 | CSRF-защита | `middleware.rs::csrf_guard`, `auth/session.rs::csrf_token`/`verify_csrf` | `auth_tests::csrf_token_is_hmac_and_verifies` | ✅ |
| 4 | Защита от brute-force | `auth/rate_limit.rs`, `auth/routes.rs::login`, `launcher/routes.rs` | `auth_tests::login_locks_after_repeated_failures`; `limits_tests::login_threshold_from_settings_locks_account` | ✅ |
| 5 | Rate limiting | `auth/rate_limit.rs`, `launcher/routes.rs` (token bucket) | `launcher_rate_limit_tests::{request_above_limit_returns_retry_after, request_121_is_rate_limited_with_retry_after, parallel_download_limit_returns_retry_after_and_recovers}` | ✅ для входа/лаунчера; ⚠ нет глобального IP-лимита (см. §5 и §15) |
| 6 | Авторизация каждого endpoint | `auth/extract.rs` (`AuthUser`), `middleware.rs` (CSRF/модули), `lib.rs` (маршруты) | `openapi_tests::specification_covers_every_route` (все 109 путей из `.route(`); `auth_tests::forbidden_error_reports_required_permission` | ✅ архитектурно; ⚠ негативный тест на каждый маршрут не автоматизирован — это ограничение, а не проверенный факт |
| 7 | RBAC и права на операции | `permissions/catalog.rs`, `auth/session.rs::load_user_for_token`, `AuthUser::require` | `auth_tests::effective_permissions_combine_roles_and_overrides`; `launcher_tests::foreign_token_cannot_see_private_build_on_any_launcher_endpoint` | ✅ |
| 8 | Защита от SQL-инъекций | Все запросы — `sqlx` с `bind`; динамические фрагменты выбираются `match`, не из ввода | `windows_invariants_tests::sql_metacharacters_in_names_are_stored_as_data` | ✅ |
| 9 | Защита от XSS | `lib.rs::CONTENT_SECURITY_POLICY`; React-экранирование; нет `dangerouslySetInnerHTML`/`innerHTML`/`eval` | `security_headers_tests::security_headers_and_final_csp_are_present_everywhere`; `windows_invariants_tests::preview_is_inert_foreign_files_are_404_and_csp_is_present` | ✅ |
| 10 | Строгий CORS | `lib.rs::build_cors` | `settings_tests::cors_uses_saved_origins_only_after_config_rebuild` | ✅ |
| 11 | Path traversal | `storage/paths.rs` (`normalize_relative`, `resolve_within`), `cloud/service.rs::validate_name`, `minecraft/validation.rs` | `storage_tests::path_traversal_is_rejected`; `windows_invariants_tests::traversal_drive_unc_and_ads_are_rejected_with_422`; `minecraft_tests::build_paths_are_validated_against_escapes` | ✅ |
| 12 | Symlink / junction / reparse points | `storage/paths.rs` (`is_link_like`, `ensure_no_links`, `canonical_directory`), `storage/engine.rs::open_object`, `storage/service.rs::browse_directory` | `windows_invariants_tests::reparse_points_cannot_escape_volume` (реальные `mklink /J` и symlink `/D`) | ✅ |
| 13 | Ограничения размеров | `lib.rs` (`DefaultBodyLimit`, `UPLOAD_BODY_LIMIT`), `cloud/uploads.rs`, `cloud/quota.rs` | `limits_tests::chunk_limit_over_http_reports_details_and_recovers`; `quota_tests::upload_over_quota_is_rejected_with_details`; `settings_tests::int_boundaries_are_enforced_for_every_key` | ✅ |
| 14 | Проверка типа и содержимого | `storage/mime.rs` (сигнатура, 8192 байта), `cloud/files.rs::preview_content_type`, `minecraft/builds.rs` (иконка) | unit `storage::mime::tests`; `cloud_tests::{content_type_comes_from_bytes_not_client_header, html_is_never_served_inline}`; `windows_invariants_tests::preview_is_inert_foreign_files_are_404_and_csp_is_present` | ✅ H1; антивирус — внешний (§8.2) |
| 15 | Ограничение параллельных загрузок | `launcher/downloads.rs` (семафор на токен) | `launcher_tests::ninth_parallel_download_is_limited_and_slots_recover`; `launcher_rate_limit_tests::parallel_download_limit_returns_retry_after_and_recovers` | ✅ F3 |
| 16 | Безопасный `Content-Disposition` | `cloud/files.rs::{content_disposition, inline_content_disposition, url_encode}` (RFC 5987 + ASCII-fallback), `launcher/artifacts.rs::sanitize_filename` (безопасный ASCII) | `windows_invariants_tests::non_ascii_download_name_uses_rfc5987_filename_star`; браузерная проверка Chromium 152 (см. `TESTING.md` §6) | ✅ |
| 17 | Защита временных каталогов | `storage/engine.rs` (`staging/`, `objects/`, `quarantine/`) | `storage_tests::failed_commit_leaves_no_committed_object`; `cloud_tests::wrong_offset_is_rejected_without_data_loss`; `realtime_tests::chunk_write_error_closes_session_and_publishes_failed` | ✅ |
| 18 | Сокрытие внутренних путей | `core/error.rs` (безопасные сообщения), `storage/paths.rs::display`, `cloud/files.rs::resolve_object` | `auth_tests::api_error_does_not_leak_internal_details`; `windows_invariants_tests::preview_is_inert_foreign_files_are_404_and_csp_is_present` (чужой файл → 404) | ✅ |
| 19 | Ограничение доступа к журналам и системным событиям | `deployment/Caddyfile.windows`, инструкции ОС; логи приложения не отдаются вебом | Автоматического теста нет. **Отклонение:** контроль прав ОС/reverse proxy; известный риск — токен публичной ссылки может попасть в access log (§11.2) | ⚠ отклонение |
| 20 | Секреты вне релиза | `.env.example` (placeholder), `config.rs`, `main.rs::check` | `cargo run -- check --json` в `TESTING.md` §3.4 (`ok=true`, секреты не выводятся); `config::tests::invalid_launcher_env_is_rejected` | ✅ |

### 12.1. Специфика Windows/NTFS

Политика проекта: **опасные имена и пути отклоняются, а не «исправляются»
молча**. Пользователь получает `422 VALIDATION_ERROR`/`PATH_TOO_LONG` и видит
причину; сервер не создаёт файл с другим именем, чем ввёл пользователь.

| Инвариант Windows/NTFS | Реализация | Проверка |
|---|---|---|
| Регистр имён не различается в одной папке | уникальный индекс `cloud_entries_name_uq` по `lower(name)` (`migrations/0003_cloud.sql`), предварительная проверка `service::entry_name_taken`, маппинг `23505` в `FILE_NAME_CONFLICT` | `cloud_tests::names_are_case_insensitive_in_one_folder`; `windows_invariants_tests::case_insensitive_conflict_does_not_overwrite_physical_file` (409, исходные байты не перезаписаны) |
| Зарезервированные имена устройств (`CON`, `PRN`, `AUX`, `NUL`, `COM1–9`, `LPT1–9`, `CONIN$`, `CONOUT$`, `CLOCK$`) | `cloud/service.rs::is_reserved_windows_name`, `minecraft/validation.rs::is_reserved_windows_name` | `windows_invariants_tests::reserved_device_names_and_trailing_chars_are_rejected_with_422`; `cloud_tests::ntfs_reserved_names_and_characters_are_rejected` |
| Завершающая точка или пробел | отклоняются, не обрезаются | те же тесты; отрицательные примеры `file.`, `file `, `...` |
| ADS (`file.txt:payload`, `::$DATA`) | `:` запрещён в `validate_name`; манифест запрещает `:` целиком | `windows_invariants_tests::traversal_drive_unc_and_ads_are_rejected_with_422`; `minecraft_tests::build_paths_are_validated_against_escapes` |
| Буква диска, UNC, обратные слэши | `validate_name` запрещает `\`; `normalize_path`/`check_manifest_path` отвергают `C:`, `\\host\share`, `\\?\` | те же тесты; `paths::normalize_relative` unit-тесты |
| `MAX_PATH` (260) | бюджет виртуального пути `PATH_LENGTH_MAX = 240` UTF-16; физические объекты лежат по коротким сгенерированным ключам | `windows_invariants_tests::long_virtual_path_returns_clear_422_without_truncation` (запрос на 359 единиц → `422 PATH_TOO_LONG`, `actualLength=359`) |
| Junction, symlink и любые reparse points | `is_link_like` проверяет `FILE_ATTRIBUTE_REPARSE_POINT`; `ensure_no_links` идёт по каждому компоненту; `canonical_directory` отвергает ссылку при регистрации тома; `open_object` возвращает `422` | `windows_invariants_tests::reparse_points_cannot_escape_volume` |
| Рекурсивный обход не уходит по ссылкам | `browse_directory` листает один уровень и пропускает reparse points; экспорт Cloud идёт по записям БД, а не обходом каталога | тот же тест: `normal-folder` виден, `junction-escape`/`symlink-escape` — нет |
| Файл занят антивирусом/индексатором | повтор с backoff на `ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION` | `replacement_tests::replace_retries_deleting_open_old_object`; `cloud_tests::purge_retries_when_object_is_open_in_another_process` |
| Unicode: NFC и регистр | `minecraft/validation.rs::path_key` (NFC + lower); `check_manifest_path` требует NFC | `minecraft_tests::duplicate_paths_are_rejected_case_insensitively`; `manifest_order_is_byte_stable_for_unicode_and_case` |
| Длинные пути в ответах администратору | `paths::display` убирает префикс `\\?\` | unit `storage::paths` |

Остаточные риски Windows/NTFS:

* `ensure_no_links` + канонизация уменьшают, но не устраняют гонку «ссылку
  подменили после проверки». Запись в том должна быть доступна только
  процессу NEXUS (права ОС).
* Defender может удалить загруженный файл (ложное срабатывание) — см. §8.2.
* NTFS регистронезависим, но PostgreSQL-индекс использует `lower(name)`;
  для не-ASCII имён это не полный аналог Windows `CompareStringOrdinal`.
  Дополнительно манифест сравнивает NFC+lower. Экзотические символы, которые
  NTFS считает эквивалентными, но `lower()` — нет, остаются теоретическим
  расхождением; физические имена на диск не попадают.

---

## 13. Чек-лист production

Отмечать перед каждым релизом; пункты без галочки — блокер production.

### 13.1. Сеть и TLS

- [ ] Домен и HTTPS работают; HTTP перенаправляется на HTTPS.
- [ ] TLS 1.2+; HSTS включён на прокси (`max-age` ≥ 1 год).
- [ ] Наружу открыты только 80/443; PostgreSQL и backend недоступны из
  интернета.
- [ ] `NEXUS_PUBLIC_URL=https://…`, `NEXUS_COOKIE_SECURE=true`.
- [ ] `NEXUS_TRUST_PROXY=true` **только** за доверенным прокси; иначе IP
  клиента подделывается.
- [ ] `NEXUS_CORS_ORIGINS` содержит только реальные origins (или пусто для
  same-origin).
- [ ] WebSocket Upgrade и Range работают через прокси.

### 13.2. Секреты и доступ

- [ ] `NEXUS_SECRET` ≥ 32 случайных символов, не `change-me`, хранится
  стабильно и с правами только для службы.
- [ ] `DATABASE_URL` использует отдельного пользователя с минимальными
  правами; пароль не в репозитории и не в истории команд.
- [ ] Администратор создан с уникальным паролем; общих учётных записей нет.
- [ ] Роли и права пересмотрены; лишние права сняты.
- [ ] Токены лаунчера имеют минимальные scopes и срок; неиспользуемые
  отозваны.
- [ ] Публичные ссылки имеют срок/лимит/пароль там, где нужно.
- [ ] `NEXUS_ENABLE_DOCS=false`.

### 13.3. Хранение и ОС

- [ ] Служба работает не от root; каталоги `data`, `logs`, тома доступны
  только ей.
- [ ] Тома хранения — отдельные каталоги, не корень диска и не системный
  каталог; места достаточно с запасом.
- [ ] `nexus check --json` не показывает проблем.
- [ ] Backup БД и (при необходимости) объектов настроен; восстановление
  проверено на чистой БД.
- [ ] Обновление и откат проверены по инструкции J3.
- [ ] Время синхронизировано (NTP); лимиты `LimitNOFILE`/`MemoryMax`
  соответствуют нагрузке.

### 13.4. Мониторинг и журналы

- [ ] `/system/monitoring` показывает реальные метрики.
- [ ] Журналы недоступны из веба; права ограничены; ротация работает.
- [ ] Настроены оповещения на 5xx, серии неудачных входов, 429, отказ тома,
  заполнение диска.
- [ ] Аудит регулярно просматривается; известные события задокументированы.
- [ ] Smoke-скрипты (`smoke-new-features`, `ws-smoke`, `resilience`)
  проходят на production-сборке.

### 13.5. Приёмка безопасности

- [ ] `cargo fmt --check`, `clippy -D warnings`, `cargo test -j 2` — зелёные.
- [ ] `npm run typecheck`, `npm run test`, `npm run build` — зелёные.
- [ ] `manifest_schema_tests` и `openapi_tests` — зелёные.
- [ ] Проверены заголовки: CSP, `X-Content-Type-Options`, `X-Frame-Options`,
  `Referrer-Policy`, `Permissions-Policy` (`security_headers_tests`).
- [ ] Известные ограничения (E2E-консоль — I1, внешний антивирус) приняты
  владельцем системы и внесены в план; они не выдаются за реализованные.

---

## 14. Реагирование на инцидент

### 14.1. Уровни

| Уровень | Пример | Первые действия |
|---|---|---|
| P1 | утечка БД/файлов, компрометация администратора, исполнение кода | изолировать, отозвать всё, сохранить доказательства, уведомить |
| P2 | подбор пароля, утечка токена лаунчера, вредоносный файл | заблокировать аккаунт/токен, проверить аудит |
| P3 | серия 429, ошибка конфигурации, подозрительный лог | разобрать, устранить, наблюдать |

### 14.2. Порядок

1. **Зафиксировать.** Не удалять журналы, не чистить `audit_events`, снять
   копию БД/логов, записать время, `request_id`, IP, аккаунт, затронутые
   файлы/сборки.
2. **Локализовать.**
   * отключить пользователя (`status = disabled`), это отзывает сессии;
   * отозвать сессии: `POST /api/v1/users/{id}/sessions/revoke` или
     `POST /api/v1/auth/sessions/revoke-all` для себя;
   * отозвать токены лаунчера: `DELETE /api/v1/admin/launcher/tokens/{id}`;
   * отозвать публичные ссылки: `DELETE /api/v1/cloud/shares/{id}`;
   * при компрометации секрета — сменить `NEXUS_SECRET` и/или пароль БД,
     перезапустить сервис; при необходимости ограничить доступ на прокси.
3. **Проверить целостность.** Запустить `system.storage.check`, сверить
   `storage_objects.sha256` с файлами, проверить `quarantined`/`missing`,
   манифесты релизов и `ref_count`. Повреждённые объекты не удалять до
   разбора; восстановить из backup (K2).
4. **Устранить причину.** Исправить код/конфигурацию, добавить регрессионный
   тест, обновить зависимости при необходимости.
5. **Восстановить.** Вернуть аккаунты/токены с новыми секретами, проверить
   вход, загрузку/скачивание, публикацию и манифест. `nexus check --json` и
   smoke-скрипты должны быть зелёными.
6. **Уведомить.** Сообщить владельцу системы и затронутым пользователям в
   согласованные сроки; не публиковать секреты и персональные данные в
   открытых каналах.
7. **Разобрать.** Краткий postmortem: таймлайн, корневая причина, что
   сработало/не сработало, какие тесты и меры добавлены. Запись хранить
   вместе с инцидентом.

### 14.3. Что нельзя делать

* удалять журналы и аудит «для чистоты»;
* менять пароль пользователя без отзыва его сессий;
* переиспользовать скомпрометированный токен или секрет;
* восстанавливать БД поверх рабочей без проверки backup;
* публиковать `NEXUS_SECRET`, `DATABASE_URL`, токены и содержимое файлов в
  issue/чатах.

---

## 15. Остаточные риски и roadmap безопасности

| Риск | Статус | Задача |
|---|---|---|
| MIME по расширению, а не по содержимому | ✅ | закрыто H1 (`storage/mime.rs`) |
| E2E-проверка консоли без CSP-ошибок | ⏳ | I1 (Playwright) |
| Полная матрица негативных тестов безопасности | ⚠ H4 закрыл Windows/NTFS-инварианты; негативный тест на каждый из 103 endpoint не автоматизирован | развитие H4 / I6 |
| Минимальная длина пароля/TTL/порог входа из настроек | ✅ | закрыто B4 |
| Лимит параллельных скачиваний лаунчера | ✅ | закрыто F3 |
| Нет глобального IP-лимита на все API | ⚠ | отдельная задача или лимит на прокси; H3/H4 его не закрывали |
| Пароль публичной ссылки без отдельного rate limit | ✅ | закрыто S3 (`SharePasswordLimiter`, 5/15 мин + 10/мин на IP) |
| Пароль публичной ссылки в URL и access log | ✅ | закрыто S2: пароль только в теле `unlock`, скачивание по HMAC-тикету |
| Токены публичных ссылок могут попадать в access log | ⚠ | эксплуатация/маскирование пути на прокси |
| In-memory лимитеры не работают между процессами | ⚠ | внешний лимитер при масштабировании |
| Нет антивирусной проверки загружаемых файлов | ⚠ | внешний сканер при требовании; см. §8.2 |
| Компрометация ОС/БД вне модели приложения | ⚠ | права ОС, backup, аудит |

---

## 16. Ссылки

* [`ARCHITECTURE.md`](ARCHITECTURE.md) — слои и поток запроса.
* [`checklists/security.md`](checklists/security.md) — краткий чек-лист перед вводом в эксплуатацию.
* [`API.md`](API.md) — коды ошибок, аутентификация, лимиты.
* [`MANIFEST.md`](MANIFEST.md) — целостность релиза и SHA-256.
* `../deployment/Caddyfile.windows` — TLS/HSTS/прокси.
* `../backend/tests/auth_tests.rs`, `resilience_tests.rs`,
  `manifest_schema_tests.rs` — проверки безопасности и целостности.

---

## AI Workspace и NEXUS AI Runtime

- Chat и Agent используют разные контракты: Chat не передаёт `tools`; Agent
  получает только серверный allowlist, а MCP-tools дополнительно требуют
  assignment и повторную проверку перед dispatch.
- Runtime аутентифицируется отдельным одноразовым enrollment и device token
  (SHA-256 hash в БД), не cookie-сессией; token отзывается/ротируется.
- Approval одноразовый, с TTL и SHA-256 args hash; аргументы после approve
  подменить нельзя. Approve/reject идемпотентны по `Idempotency-Key`.
- Workspace paths проверяются Runtime по `allowed_roots`, canonical containment
  и reparse points непосредственно перед операцией.
- API key провайдера и MCP secret шифруются AES-256-GCM; device token хранится
  как hash; секреты не попадают в UI/audit/logs/outbox.
- Prompt injection из файлов/Skills/tool output/MCP не расширяет политику.
- MCP: соединения выключены по умолчанию, endpoint проверяется на SSRF на
  backend и повторно на Runtime (loopback/private/link-local/metadata требуют
  явного `allowPrivateNetwork`), redirects запрещены, результаты недоверенные.
- Сеть программ: `network_policy='deny'` по умолчанию, Windows Firewall
  outbound block, fail-closed без прав администратора, Job Object с
  kill-on-close и лимитом памяти.
- Cancel: задача не считается отменённой до `task.cancelled{confirmed=true}`;
  при отсутствии подтверждения — `AI_CANCEL_UNCONFIRMED`, а не ложный успех.
- Replay: sequence/timestamp/messageId, receipts, уникальные dispatch/message
  id, HTTP Idempotency-Key.
- Retry: автоматически повторяются только идемпотентные инструменты и только
  при ошибке без подтверждённого side effect; тайм-аут, обрыв и неизвестный
  результат не повторяются.
- Receipts: ротация по размеру; перед удалением сегмента `messageId`
  сохраняются в компактный индекс, повторная доставка получает
  `AI_RECEIPT_ROTATED` и не выполняется.
- Agent Workspace: каждый файловый запрос проверяет RBAC, владение и
  `deleted_at`; путь нормализуется на backend и повторно на Runtime.
  Сохранение защищено `expectedSha256`, откат — `afterSha256`; при конфликте
  файл не изменяется. Загрузка ограничена лимитом workspace и 8 МБ, SVG/HTML
  отдаются только как `attachment` + `nosniff`. Создать workspace на
  произвольном Runtime нельзя: только на устройстве, где у пользователя уже
  есть workspace.
- Allowlist существующих workspace не расширяется автоматически: новые
  безопасные инструменты добавляются только явной административной операцией
  с audit.
- `changedFiles`/`diff` не содержат содержимого секретов; абсолютные пути
  Runtime вычищаются, UI показывает пути относительно workspace.
- Полный перечень ограничений:
  [`AI_WORKSPACE.md`](AI_WORKSPACE.md#13-ограничения-текущей-версии).
