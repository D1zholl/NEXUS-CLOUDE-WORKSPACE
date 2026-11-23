# NEXUS WORKSPACE 1.0.0

Единая self-hosted веб-платформа на Rust, объединяющая два независимых рабочих
пространства и общую административную систему:

- **Minecraft Build Manager** — создание сборок Minecraft, загрузка файлов,
  проверка, версионирование, релизы, манифесты и API для собственного лаунчера.
- **NEXUS Cloud Storage** — облачное файлохранилище с физическим размещением
  файлов на выбранных дисках сервера, возобновляемыми загрузками, предпросмотром,
  корзиной и публичными ссылками.
- **System Management** — пользователи, роли и granular-права, диски и тома,
  мониторинг, фоновые задачи, журнал событий, настройки 3D-фона и модулей.

Проект рассчитан на постоянную эксплуатацию на собственном сервере: реальные
серверные механизмы, PostgreSQL, объектное хранение на дисках, серверные сессии,
RBAC, WebSocket-события и проверяемые контракты API. Демонстрационных данных и
декоративных кнопок нет.

> Версия **1.0.0** (20.11.2026). Карта документации —
> [docs/README.md](docs/README.md). История изменений —
> [CHANGELOG.md](CHANGELOG.md). Аудит готовности и открытые пункты —
> [COMPLETE.md](COMPLETE.md), фактические результаты тестов —
> [docs/TESTING.md](docs/TESTING.md). Краткая инструкция установки и входа на
> Windows — [docs/INSTALL-WINDOWS.md](docs/INSTALL-WINDOWS.md).

---

## Возможности

### Общая платформа

- Модульный монолит: один процесс, чёткие границы модулей `minecraft`, `cloud`,
  `system`, `storage`, `auth`, `realtime`.
- Серверные сессии в cookie `HttpOnly` / `SameSite=Lax` / `Secure` (включается
  конфигурацией), CSRF-токен, привязанный к сессии через HMAC.
- Argon2id для паролей, ограничение попыток входа, временная блокировка аккаунта,
  выравнивание времени ответа для несуществующих пользователей.
- Роли и granular-права (20 разрешений), проверка прав на сервере для каждого
  защищённого запроса.
- Единый формат ответа и ошибок, `request_id`, коды ошибок
  (`SESSION_EXPIRED`, `FILE_NAME_CONFLICT`, `MODULE_DISABLED`, `QUOTA_EXCEEDED`,
  `RATE_LIMITED`, `INSUFFICIENT_SPACE` и др.).
- OpenAPI 3.1: `GET /api/v1/openapi.json`; Swagger UI — `/api/v1/docs`
  (только при `NEXUS_ENABLE_DOCS=true`). Карта документации —
  [`docs/README.md`](docs/README.md); API — [`docs/API.md`](docs/API.md),
  спецификация — [`docs/openapi.json`](docs/openapi.json),
  манифест релиза — [`docs/MANIFEST.md`](docs/MANIFEST.md),
  безопасность — [`docs/SECURITY.md`](docs/SECURITY.md),
  администрирование — [`docs/ADMIN_GUIDE.md`](docs/ADMIN_GUIDE.md),
  работа пользователя — [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md),
  развёртывание — [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md),
  тесты — [`docs/TESTING.md`](docs/TESTING.md).
- WebSocket `/api/v1/ws`: прогресс загрузок и задач, публикация сборок,
  метрики, обновления томов. События адресуются владельцу или системе.
- Серверный гейт модулей: отключение `minecraft` или `cloud` в системных
  настройках блокирует их API без перезапуска (публичные ссылки продолжают работать).
- Один активный экземпляр на базу данных: при старте берётся PostgreSQL
  advisory lock `hashtext('nexus.instance')`; второй процесс завершается с
  понятным сообщением. Это защищает in-memory лимитеры, кэш сессий и
  планировщик от дублирования.
- Публичные ссылки: пароль проверяется в `POST /unlock` и не попадает в URL;
  скачивание идёт по HMAC-тикету на 1 час, `Range` не обходит `maxDownloads`,
  пароль защищён от перебора (5 ошибок / 15 минут).

### Minecraft Build Manager

- Сборки: создание, редактирование, клонирование, архивирование, удаление,
  иконка, категория, видимость, статус.
- Пошаговый мастер из 5 шагов с валидацией и подстановкой требований Java.
- Metadata официальных источников: Mojang, Fabric Meta, Forge Maven, NeoForge
  Maven, Quilt — через backend с кешированием.
- Файловый менеджер сборки: загрузка файлов, папок и drag & drop, создание
  папок, переименование, перемещение, замена содержимого, удаление, поиск,
  фильтр по категории и стороне.
- Категории `mods`, `config`, `resourcepacks`, `shaderpacks`, `scripts`,
  `overrides`, `other`; стороны `client` / `server` / `both`.
- Проверка сборки: обязательные поля, совместимость версий, наличие и размеры
  файлов, SHA-256, конфликты путей, доступность объектов хранения.
- Релизы со статусами `draft` / `validating` / `ready` / `published` /
  `archived` / `deprecated`, неизменяемость опубликованных версий, changelog,
  сравнение релизов, история, откат и отключение распространения.
- Манифест с `schema_version: "1.0"`, относительными путями, SHA-256, размером
  и ссылками на скачивание через API. Локальные пути сервера не раскрываются.

### NEXUS Cloud Storage

- Несколько дисков и томов: обнаружение дисков, регистрация каталогов,
  ручной и автоматический выбор тома, резерв места, проверка доступности.
- Объектное хранение `<root>/.nexus/{objects,staging,quarantine}`; на диске
  лежит сгенерированный ключ, исходные имена и папки хранятся в PostgreSQL.
- Защита путей: нормализация, запрет `..` и абсолютных путей, канонизация,
  проверка символьных ссылок и junctions, запрет системных каталогов.
- Возобновляемые потоковые загрузки чанками с серверным offset, паузой,
  продолжением, отменой и очисткой просроченных сессий.
- Очередь загрузок в браузере: параллельность, прогресс каждого файла и общий,
  скорость, ETA, повтор после сетевых ошибок, восстановление после перезагрузки.
- Файловый менеджер: папки, список и сетка, поиск, сортировка, избранное,
  «Все файлы», «Недавние», «Общий доступ», корзина, массовые операции.
- Скачивание с `Content-Length` и HTTP Range; массовое скачивание в ZIP
  выполняется фоновой задачей с прогрессом и TTL архива.
- Предпросмотр изображений, видео, аудио, PDF, текста и кода; HTML/JS/SVG
  не исполняются inline.
- Публичные ссылки: срок действия, пароль, лимит скачиваний, отзыв; физические
  пути и внутренние идентификаторы не раскрываются.
- Корзина с восстановлением, очисткой записи и всей корзины; физический объект
  удаляется только после проверки всех ссылок.
- Квоты: индивидуальные и по умолчанию для Cloud и Minecraft.

### System Management

- 13 отдельных страниц: обзор, пользователи, роли, внешний вид и 3D-фон,
  хранилища, ограничения, сеть, безопасность, мониторинг, задачи, журнал,
  обслуживание.
- Пользователи: создание, роли, индивидуальные права, блокировка, сброс пароля,
  завершение сессий, квоты, доступ к модулям.
- Мониторинг реальных показателей: CPU (общий и по ядрам), RAM/swap, сеть,
  диски, процесс NEXUS, RPS, задержка, ошибки, активные запросы, скачивания,
  WebSocket, метрики по модулям; история за 5 м / 15 м / 1 ч / 24 ч / 7 дней.
- Фоновые задачи с типами, статусами, прогрессом, отменой: проверка объектов,
  очистка загрузок, ссылок, метрик и idempotency-ключей.
- Журнал событий: входы, изменения пользователей и прав, загрузки, замены,
  удаления, релизы, настройки, ошибки; без паролей и токенов.
- 3D-фон: пресеты, качество, FPS, параметры кубов, частиц и камеры,
  сохранение per-user, глобальные значения по умолчанию, `prefers-reduced-motion`.

---

## Технологический стек

| Слой                           | Технологии                                                                       |
| ---------------------------------- | ------------------------------------------------------------------------------------------ |
| Backend                            | Rust 2024 (stable), Axum 0.8, Tokio, Tower / tower-http                                    |
| База данных              | PostgreSQL, SQLx 0.8, миграции                                                     |
| Безопасность           | Argon2id, серверные сессии, HMAC CSRF, RBAC                                 |
| Realtime                           | Axum WebSocket, broadcast-брокер                                                     |
| Наблюдаемость         | tracing, ротация файлов журнала, собственные метрики |
| Frontend                           | React 19, TypeScript, Vite 7, React Router 7                                               |
| Состояние и данные | TanStack Query, Zustand                                                                    |
| 3D                                 | Three.js, WebGL, InstancedMesh, EffectComposer                                             |
| UI                                 | Lucide React, CSS Variables / структурированные CSS-файлы            |
| Тесты                         | Rust integration tests, Vitest, PowerShell smoke-скрипты                            |
| Развёртывание         | Windows-служба, Caddy, HTTPS                                                        |

---

## Структура репозитория

```text
NEXUS-WORKSPACE/
├── backend/
│   ├── Cargo.toml
│   ├── migrations/                 # 18 миграций PostgreSQL
│   ├── src/
│   │   ├── main.rs                 # CLI: serve / migrate / create-admin / check
│   │   ├── lib.rs                  # сборка роутера, слои middleware
│   │   ├── config.rs               # TOML + переменные окружения
│   │   ├── state.rs                # AppState, кэши модулей и настроек
│   │   ├── middleware.rs           # request_id, CSRF, гейт модулей, метрики
│   │   ├── auth/                   # сессии, пароли, лимиты, маршруты
│   │   ├── users/                  # пользователи и квоты
│   │   ├── permissions/            # роли, разрешения, каталог прав
│   │   ├── storage/                # Storage Engine, диски, пути, хеши
│   │   ├── cloud/                  # файлы, папки, загрузки, ссылки, ZIP
│   │   ├── minecraft/              # сборки, файлы, метаданные, релизы, манифесты
│   │   ├── launcher/               # каталог, токены, артефакты
│   │   ├── realtime/               # WebSocket-брокер
│   │   ├── system/                 # настройки, мониторинг, задачи, аудит
│   │   ├── api/                    # ApiOk/Page и OpenAPI
│   │   └── core/                   # ошибки, идемпотентность
│   └── tests/                      # 332 backend-теста (54 unit + 4 main + 274 integration)
├── frontend/
│   ├── src/
│   │   ├── app/                    # точка входа приложения
│   │   ├── components/             # AppLayout, UI-кит, мастер сборки, очередь
│   │   ├── lib/                    # API-клиент, realtime, загрузки, форматтеры
│   │   ├── pages/                  # вход, главная, Cloud, Minecraft, System
│   │   ├── services/               # HTTP-клиент
│   │   ├── stores/                 # Zustand: auth, UI, 3D-фон
│   │   ├── styles/                 # токены и стили
│   │   └── three/                  # движок 3D-фона
│   └── package.json
├── deployment/
│   ├── install-windows.ps1         # установка, служба, администратор
│   ├── update-windows.ps1 / backup.ps1 / restore.ps1
│   ├── backup-objects.ps1 / restore-objects.ps1
│   ├── Caddyfile.windows
│   ├── nexus.production.toml
│   └── env.production.example
├── tests/                          # smoke, нагрузка, отказоустойчивость, WebSocket
├── docs/                           # карта — docs/README.md; архитектура, API, руководства, развёртывание, тесты
├── MAIN_PLAN.md                    # техническое задание
├── COMPLETE.md                     # фактическая готовность по этапам ТЗ
├── .env.example
└── README.md
```

---

## Требования

- **Rust** 1.85+ (edition 2024), Cargo.
- **Node.js** 20+ и npm.
- **PostgreSQL** 14+ (проверено на 17).
- **Windows 11 / Windows Server** (целевая платформа).
- Свободный порт `8080` (или свой) и место на дисках хранения.

---

## Быстрый старт (разработка)

### 1. База данных

```sql
create role nexus login password 'nexus';
create database nexus owner nexus;
```

Тесты используют отдельную БД `nexus_test`; она создаётся автоматически, если
у пользователя есть право `createdb`.

### 2. Конфигурация

```powershell
Copy-Item .env.example .env
```

Обязательно задайте:

```dotenv
DATABASE_URL=postgres://nexus:nexus@127.0.0.1:5432/nexus
NEXUS_SECRET=<64 hex-символа, например openssl rand -hex 32>
```

Без `NEXUS_SECRET` сервер сгенерирует временный секрет: после перезапуска
CSRF-токены станут недействительны.

### 3. Backend

```powershell
cd backend
cargo run -- migrate
cargo run -- create-admin --username admin
cargo run -- serve
```

Первый администратор создаётся интерактивно. Для автоматической установки
можно передать пароль через переменную окружения:

```powershell
$env:NEXUS_ADMIN_PASSWORD = '...'
cargo run -- create-admin --username admin --password-env NEXUS_ADMIN_PASSWORD
```

### 4. Frontend

```powershell
cd frontend
npm install
npm run dev
```

Vite откроет `http://127.0.0.1:5173` и проксирует `/api` (включая WebSocket)
на `http://127.0.0.1:8080`.

### 5. Production-сборка

```powershell
cd frontend
npm run build            # frontend/dist

cd ..\backend
cargo build --release
.\target\release\nexus.exe serve
```

Сервер раздаёт статику из `static_dir` (`../frontend/dist` по умолчанию) и
отдаёт SPA-fallback. Приложение доступно на `http://127.0.0.1:8080`.

---

## Быстрый старт на Windows (BAT)

В корне проекта есть три сценария для cmd.exe. Они используют UTF-8 без BOM и
`chcp 65001` (BOM в первой строке ломает разбор `@echo off`).

| Файл | Назначение |
|---|---|
| `first-run.bat` | Проверяет Rust/Node/PostgreSQL, создаёт `.env`, собирает backend и frontend, применяет миграции, создаёт администратора, предлагает запуск или установку службы. Режимы: `--check`, `--env-only`, `--setup-only`, `--no-build` |
| `start.bat` | Обычный запуск: служба или консоль. Флаги: `--service`, `--console`, `--local`, `--force`, `--status`, `--no-browser` |
| `users.bat` | Меню управления пользователями: список, создание, сброс пароля, блокировка, разблокировка, удаление, сессии, роли, создание и изменение суперпользователя. Быстрый список: `users.bat --list` |

Все три файла поддерживают `--help`. Пароли вводятся только внутри
`nexus.exe` (скрытый ввод `rpassword`) и не передаются аргументами командной
строки, в URL или журналы BAT. Журналы запусков — в `logs\`. Подробности —
[`docs/INSTALL-WINDOWS.md`](docs/INSTALL-WINDOWS.md).

---

## CLI

| Команда                            | Назначение                                                                                              |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `nexus serve`                           | Запуск HTTP-сервера (по умолчанию)                                                        |
| `nexus migrate`                         | Применение миграций и синхронизация каталога прав                     |
| `nexus create-admin --username <login>` | Создание первого администратора                                                      |
| `nexus check [--json]`                  | Проверка конфигурации, БД, миграций, статики и администратора |
| `nexus user list [--json]`              | Список пользователей, ролей, статуса и последнего входа |
| `nexus user create --username <u> [--role <key>]` | Создание пользователя; пароль вводится скрыто |
| `nexus user passwd --username <u> [--revoke-sessions]` | Смена пароля и сброс блокировки |
| `nexus user disable/enable/unlock --username <u>` | Блокировка, разблокировка, снятие блокировки после неудачных входов |
| `nexus user delete --username <u> [--yes]` | Безвозвратное удаление пользователя (последнего суперадминистратора удалить нельзя) |
| `nexus user sessions revoke --username <u>` | Завершить все сессии пользователя |
| `nexus user role add/remove --username <u> --role <key>` | Выдать или снять роль |
| `nexus objects audit [--json]`          | Проверить наличие объектов на дисках, пометить `missing` |
| `nexus objects purge-missing [--dry-run]` | Удалить из БД отсутствующие объекты без ссылок |

`nexus check` возвращает код `0`, если проблем нет, и `1` — если есть. JSON-вывод
предназначен для скриптов установки и мониторинга.

---

## Конфигурация

Приоритет: переменные окружения → `nexus.toml` → значения по умолчанию.
Файл `nexus.toml` и `.env` ищутся в текущем каталоге, а для службы — рядом с
исполняемым файлом.

### Основные переменные

| Переменная                                      | Назначение                                                                   |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `DATABASE_URL`                                          | Строка подключения PostgreSQL                                         |
| `NEXUS_SECRET`                                          | Секрет подписи CSRF (≥ 32 символов)                              |
| `NEXUS_HOST`, `NEXUS_PORT`                            | Адрес и порт сервера                                                  |
| `NEXUS_PUBLIC_URL`                                      | Публичный URL для ссылок и cookie                                   |
| `NEXUS_STATIC_DIR`                                      | Каталог собранного frontend                                           |
| `NEXUS_COOKIE_SECURE`                                   | `true` при работе через HTTPS                                          |
| `NEXUS_TRUST_PROXY`                                     | Доверять `X-Forwarded-For` / `X-Real-IP`                                   |
| `NEXUS_CORS_ORIGINS`                                    | Список разрешённых origin через запятую                   |
| `NEXUS_PASSWORD_MIN_LENGTH`                             | Минимальная длина пароля для CLI (по умолчанию 12) |
| `NEXUS_MAX_UPLOAD_MB`                                   | Максимальный размер файла                                       |
| `NEXUS_UPLOAD_TTL_HOURS`                                | TTL незавершённой загрузки                                        |
| `NEXUS_EXPORT_TTL_HOURS`                                | TTL ZIP-архива массового скачивания                           |
| `NEXUS_LOG_DIR`, `NEXUS_LOG_FILE`, `NEXUS_LOG_KEEP` | Файловый журнал и ротация                                        |
| `NEXUS_ENABLE_DOCS`                                     | Включить Swagger UI                                                            |
| `RUST_LOG`                                              | Уровни `tracing`                                                               |

### `nexus.toml`

Секции: `[server]`, `[database]`, `[security]`, `[storage]`, `[logging]`.
Production-пример — `deployment/nexus.production.toml`.

Основные изменения лимитов и TTL также доступны из `/system/limits` и
применяются в рантайме через кэш настроек (см. COMPLETE.md — часть ключей
пока не влияет на поведение).

---

## API

Базовый префикс: `/api/v1`. Группы:

```text
/auth/          вход, выход, сессия, смена пароля
/users/         пользователи и квоты
/roles/         роли
/permissions/   каталог разрешений
/me/            личные настройки
/storage/       диски и тома
/cloud/         файлы, папки, загрузки, ссылки, экспорт
/minecraft/     сборки, файлы, метаданные, релизы, манифесты
/launcher/      внутренний Launcher API (Bearer-токен)
/admin/launcher управление токенами лаунчера
/api/packs/     публичный репозиторий MyLauncher (без токена)
/api/files/     файлы репозитория MyLauncher (SHA-256, Range)
/system/        настройки, модули, журнал, OpenAPI
/monitoring/    метрики и история
/jobs/          фоновые задачи
/public/shares/ публичные ссылки без сессии
/ws             WebSocket-события
```

Формат ошибки:

```json
{
  "success": false,
  "error": {
    "code": "FILE_NAME_CONFLICT",
    "message": "Файл с таким именем уже существует",
    "details": { "existingFileId": "uuid" }
  },
  "request_id": "uuid"
}
```

- Спецификация: `GET /api/v1/openapi.json` (117 путей, 142 операции).
- Swagger UI: `GET /api/v1/docs` при `NEXUS_ENABLE_DOCS=true`.
- Аутентификация браузера — cookie-сессия; изменяющие запросы требуют
  заголовок `X-CSRF-Token` из `/auth/me` или `/auth/login`.
- Внутренний Launcher API использует Bearer-токены (`/launcher/auth/token`),
  ограниченные scopes `launcher.catalog` и `launcher.artifacts`.
- MyLauncher подключается к публичным `/api/packs` и `/api/files` без токена;
  приватные сборки stock-клиенту недоступны (см. `docs/LAUNCHER_CONTRACT.md`).
- Загрузки: `POST /cloud/uploads`, затем `PATCH` чанков с `Upload-Offset`,
  затем `POST .../complete` с `Idempotency-Key`.

---

## Тестирование

```powershell
# Backend
cd backend
cargo check --all-targets
cargo test -j 2          # нужен локальный PostgreSQL и право createdb

# Frontend
cd ..\frontend
npm run typecheck
npm run test             # Vitest
npm run build
```

Интеграционные тесты backend работают с реальным PostgreSQL и отдельной БД
`nexus_test`. Если БД недоступна, тест падает с понятным сообщением, а не
проходит ложно.

Дополнительно, против запущенного сервера:

```powershell
powershell -File tests/smoke-new-features.ps1
powershell -File tests/ws-smoke.ps1
powershell -File tests/resilience.ps1
powershell -File tests/load.ps1
```

Фактические результаты финального прогона 1.0.0 (20.11.2026, Windows 11,
служба `NexusWorkspace`): backend **332/332** (54 unit + 4 main + 274
integration), frontend **63/63** на повторном прогоне 20.11.2026, Playwright E2E **24/24**, `nexus check` —
`ok=true`; живой прогон smoke/ws/resilience/load — PASS. Матрица тестов, команды
и открытые прогоны — в [`docs/TESTING.md`](docs/TESTING.md), отчёт приёмки —
`docs/reports/release-1.0.0-l2.md`.

---

## Развёртывание

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1
```

Скрипт собирает backend и frontend, создаёт каталог установки, генерирует
`.env`, регистрирует службу Windows и создаёт администратора.

Обновление, резервное копирование и восстановление:

```powershell
deployment\update-windows.ps1
deployment\backup.ps1
deployment\restore.ps1
```

### Reverse proxy и HTTPS

Готовая конфигурация: `deployment/Caddyfile.windows`. Caddy проксирует HTTP и
WebSocket (`Upgrade` / `Connection`) на `127.0.0.1:8080`. PostgreSQL не должен
быть доступен из интернета.

При работе через HTTPS:

```dotenv
NEXUS_PUBLIC_URL=https://nexus.example.com
NEXUS_COOKIE_SECURE=true
NEXUS_TRUST_PROXY=true
```

---

## Безопасность

- Argon2id, минимальная длина пароля 12 символов, буквы и цифры.
- Серверные сессии; токен хранится только в cookie, в БД — SHA-256 хеш.
- CSRF-защита всех изменяющих запросов, кроме явно публичных.
- Ограничение частоты входа и запросов лаунчера.
- Проверка прав на сервере; скрытие кнопок не считается защитой.
- Параметризованные запросы SQLx.
- Защита от path traversal, проверка symlink/junction, запрет системных корней.
- Потоковая передача файлов без загрузки в RAM; проверки размера и сигнатур иконок.
  MIME остальных файлов определяется по первым байтам при завершении загрузки;
  `Content-Type` клиента не используется. HTML/SVG скачиваются, JS — как текст.
- Финальная CSP (`frame-src`/`object-src 'none'`, `worker-src 'self' blob:`,
  `script-src 'self'`), `X-Content-Type-Options`, `X-Frame-Options`,
  `Referrer-Policy`, `Permissions-Policy`.
- На Windows кратковременная блокировка staging-файла антивирусом переживается
  retry; рекомендации по исключениям Defender — в `docs/DEPLOYMENT.md` §4.5
  (с оговоркой о риске).
- Внутренние пути сервера и технические трассировки не попадают в ответы API.

Секреты (`NEXUS_SECRET`, `DATABASE_URL`) хранятся только в `.env` на сервере
и не входят в release-архив.

---

## Известные ограничения

Подробный разбор — в [COMPLETE.md](COMPLETE.md). Кратко:

- Системные настройки применяются: 15 ключей — без перезапуска, `network.*` —
  при старте службы; `jobs.max_concurrent` ограничивает планировщик.
- Фоновые задачи выполняет планировщик (1,5 с, `FOR UPDATE SKIP LOCKED`);
  доступны пауза, продолжение и отмена.
- WebSocket публикует `upload.failed` и инкрементальный `download.progress`.
- PDF открывается через PDF.js; Playwright E2E (24 сценария) входит в релиз.
- Публичные ссылки «только для авторизованных» и автоочистка корзины
  (`system.trash.cleanup`) реализованы.
- Тома хранения привязаны к области (`system`/`cloud`/`minecraft`) и
  владельцу (K1).
- В `docs/` созданы ARCHITECTURE, API, DATABASE, ADMIN_GUIDE, USER_GUIDE,
  DEPLOYMENT, TESTING, SECURITY, MANIFEST и JSON Schema;
  актуальная карта — [`docs/README.md`](docs/README.md).
- 25-ГБ нагрузочный сценарий подтверждён отчётом G2
  (`docs/reports/large-build-20261119.md`, PASS).
- Платформа поставки — Windows 11 / Windows Server; поддержка Linux в проект
  не входит.
- Запускается ровно один экземпляр NEXUS на БД: кэши, лимитеры и планировщик
  in-memory, второй процесс останавливается на advisory lock. Горизонтальное
  масштабирование требует внешних лимитеров и очереди.

---

## Лицензия

MIT (см. `backend/Cargo.toml`).
