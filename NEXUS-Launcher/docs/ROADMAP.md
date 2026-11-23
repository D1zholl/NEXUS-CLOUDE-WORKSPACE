# NEXUS-launcher — roadmap

**Версия:** 1.0  
**Дата:** 21.11.2026  
**Основание:** `docs/AUDIT.md`, `docs/STATUS.md`, `MAIN-DEV-PLAN.md`

---

## 1. Принципы

1. **Никакой функциональности без теста и без реального результата.** Кнопка
   без команды, прогресс без загрузки, «демо-сборка» — запрещены.
2. **Этап не начинается, пока предыдущий не прошёл gate.** Исключение —
   параллельные работы, явно отмеченные как независимые.
3. **Сначала контракт, потом код.** Для каждого внешнего источника фиксируется
   версия контракта, фикстуры и поведение при ошибках.
4. **Безопасность — часть архитектуры.** Path/zip/process/secrets проверяются
   тестами на каждом этапе, а не «в конце».
5. **Один SQLite-слой.** SQLx, без `rusqlite` (конфликт `links = "sqlite3"`).
6. **Один HTTP-стек.** `reqwest 0.13`, как у Tauri 2.12; без второго `reqwest`.
7. **Данные пользователя неприкосновенны.** Обновление/восстановление не
   удаляет инстансы, миры, Java, аккаунты и бэкапы без явного подтверждения.
8. **Офлайн-первый.** Лаунчер работает без NEXUS Workspace и без сети для уже
   установленных инстансов.

---

## 2. Целевые релизы

| Релиз | Содержание | Критерий |
|---|---|---|
| 0.1 | Оболочка + БД + логи + навигация + фон | Приложение запускается, состояние сохраняется |
| 0.2 | Vanilla: установка, Java, запуск offline | Чистая ваниль ставится и запускается |
| 0.3 | Microsoft Auth + аккаунты | Реальный аккаунт входит, токен обновляется |
| 0.4 | Fabric/Quilt/Forge/NeoForge | Модифицированный инстанс запускается |
| 0.5 | Modrinth + модпаки | Реальная сборка импортируется и запускается |
| 0.6 | NEXUS Builds | Реальная сборка NEXUS ставится по SHA-256 |
| 0.7 | Локальные серверы + бэкапы | Сервер создаётся, работает, восстанавливается |
| 1.0 | Полировка, установщик, обновление, подпись | Чистая установка/удаление, updater, документация |

---

## 3. Карта модулей

Каждый модуль — независимая единица с своим API, ошибками и тестами.
Зависимости направлены только вниз; циклы запрещены.

| Модуль | Ответственность | Входы | Выходы | Зависит от | Не зависит от |
|---|---|---|---|---|---|
| `core` | ошибки, ID, время, конфиг, пути, fs-safety, DTO | параметры, env | типы, проверенные пути | — | Tauri, HTTP, БД |
| `db` | SQLite, миграции, репозитории | DTO | записи, транзакции | core | UI, Tauri |
| `net` | HTTP-клиент, retry, rate limit, redaction, SSRF-политика | URL, политика | стримы, ошибки | core | БД, UI |
| `download` | очередь, Range, resume, хэш, staging, лимит скорости | задачи | файлы, события | core, net | Tauri, UI |
| `java` | detect, verify, Adoptium, реестр runtime | метаданные | runtime | core, net, download | Minecraft |
| `minecraft` | version manifest, libraries, assets, natives, install, args | версия, каталог | установка, команда запуска | core, net, download, java | loaders, UI |
| `loaders` | Fabric/Forge/NeoForge/Quilt adapters | версия MC, версия loader | profile, install plan | minecraft, download | content, UI |
| `instances` | манифест, CRUD, clone, import/export, изоляция | DTO | каталоги, записи | core, db, minecraft | UI |
| `auth` | providers, offline, Microsoft, token store | аккаунт | session, токены | core, net, secrets | instances, UI |
| `content` | Modrinth, CurseForge, resolver | запрос, инстанс | план установки | core, net, instances | servers |
| `modpacks` | mrpack, CF zip, Prism/MultiMC, HMCL, raw zip | архив | instance plan | content, instances, download | UI |
| `nexus` | sources, catalog, manifest, install, update | base URL | instance plan | instances, download, auth | content |
| `servers` | install, supervisor, console, properties, players, EULA | server DTO | процесс, лог, статус | java, minecraft, loaders, db | UI |
| `backups` | backup/restore, retention, consistency | target | архив | core, db, instances, servers | UI |
| `diagnostics` | логи, crash reports, export | события | файлы, отчёт | core | все доменные |
| `ipc` (src-tauri) | команды, события, state, capabilities | DTO | invoke/event | все доменные | бизнес-логика |
| `ui` | React, страницы, stores, дизайн | IPC | экран | ipc-контракт | файловая система |

### 3.1. Матрица «модуль → этап»

| Модуль | Этапы |
|---|---|
| `core`, `db`, `ipc`, `ui` (каркас) | 0–1 |
| `net`, `download` | 2 |
| `instances` | 3 |
| `minecraft`, `java` | 4–6 |
| `auth` | 7 |
| `loaders` | 8 |
| `content` | 9 |
| `modpacks` | 10 |
| `nexus` | 11 |
| `servers` | 12 |
| `backups` | 13 |
| `diagnostics` | 6, 14 |
| updater/installer | 15 |

### 3.2. Предлагаемая структура

```
NEXUS-Launcher/
├── Cargo.toml                 # workspace
├── rust-toolchain.toml        # stable, pin
├── crates/
│   ├── nexus-core/
│   ├── nexus-db/
│   ├── nexus-net/
│   ├── nexus-download/
│   ├── nexus-java/
│   ├── nexus-minecraft/
│   ├── nexus-loaders/
│   ├── nexus-instances/
│   ├── nexus-auth/
│   ├── nexus-content/
│   ├── nexus-modpacks/
│   ├── nexus-nexus/
│   ├── nexus-servers/
│   ├── nexus-backups/
│   └── nexus-diagnostics/
├── src-tauri/                 # тонкий адаптер
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   ├── icons/
│   └── src/{main.rs,lib.rs,commands/,events/,state.rs}
├── frontend/
│   ├── package.json
│   ├── src/{app,components,pages,features,services,stores,hooks,styles}
│   └── tests/
├── migrations/
├── docs/
├── resources/
└── tests/                     # межкрейтовые интеграционные + фикстуры
```

Это отклонение от буквальной структуры ТЗ (§2.2), но оно сохраняет её
смысл — модульность — и делает её проверяемой. Решение фиксируется ADR.

---

## 4. Последовательность этапов

Оценка размера: **S** — до недели, **M** — 1–3 недели, **L** — 3–6 недель
для одного разработчика. Это ориентир, не обязательство.

### Этап 0. Фундамент и контракты (S)

**Цель:** подготовить репозиторий и зафиксировать решения, не добавляя
игровой функциональности.

Задачи:

1. Установить git, выполнить `git init`, настроить `.gitignore`
   (`target/`, `node_modules/`, `dist/`, `*.db`, `logs/`, `.env`).
2. Создать `README.md` лаунчера (не копию серверного README).
3. Скопировать в `docs/reference/` актуальные `LAUNCHER_CONTRACT.md`,
   `MANIFEST.md`, `API.md`, `openapi.json`, `SECURITY.md`, `TESTING.md`,
   `schemas/*.json`; зафиксировать версию контракта.
4. Написать ADR: структура workspace, baseline зависимостей, TypeScript 6.0.3,
   расположение данных, per-user установка, роутинг, фон.
5. Создать `docs/ARCHITECTURE.md`, `docs/API-INTEGRATION.md`,
   `docs/DATABASE.md`, `docs/UI-DESIGN.md` (требование ТЗ, этап 0).
6. Зафиксировать `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`,
   `.editorconfig`, лицензию.
7. Создать пустой workspace и `frontend` с tooling-конфигами.
8. Настроить CI (Windows): fmt, clippy, test, typecheck, lint, build.
9. Определить error envelope, IPC-контракт, события (документ).
10. Определить политику миграций и версионирования `instance.json`.

**Gate 0:**

- `git log` существует, CI зелёный на пустом проекте;
- `cargo check --workspace` и `npm run build` проходят;
- все ADR и 4 документа созданы;
- контракт NEXUS зафиксирован с версией и датой.

---

### Этап 1. Оболочка приложения (M)

**Цель:** запускаемое приложение с навигацией, БД, логами и непрерывным фоном.

Задачи:

- Tauri 2 window, кастомный titlebar, минимальные capabilities;
- React layout: sidebar, topbar, workspace, нижняя панель запуска (пока без
  запуска — только место и состояние);
- 10 разделов как пустые состояния с честным «не реализовано»;
- единый фон: один инстанс на всё приложение, не перезапускается при
  навигации, пауза при `visibilitychange`, FPS cap, `prefers-reduced-motion`,
  CSS fallback;
- SQLite + миграции + WAL + `busy_timeout`;
- `app_settings`, `activity_logs`;
- tracing + ротация логов + redaction;
- single-instance;
- error envelope + типизированные команды;
- Zustand/TanStack Query;
- тесты: stores, навигация, БД, миграции, redaction.

**Gate 1:**

- приложение запускается и закрывается без ошибок;
- навигация сохраняет состояние, фон не мигает;
- настройка переживает перезапуск;
- логи пишутся и ротируются;
- ни одного `unwrap()` в IPC-слое без обработки;
- нет permissions `shell`/`fs`.

---

### Этап 2. Сеть и загрузки (M)

**Цель:** надёжный общий загрузчик, на котором строятся все остальные модули.

Задачи:

- `net`: клиент, таймауты, редиректы, TLS, User-Agent, retry/backoff, 429,
  `Retry-After`, redaction, лимит размера, SSRF-политика;
- `download`: очередь, параллельность (semaphore), Range/resume, `If-Range`,
  fallback на полный файл, SHA-1/SHA-256, staging, атомарный rename,
  пауза/продолжение/отмена, ограничение скорости, прогресс/ETA;
- mock HTTP-сервер для тестов (206/416/500/429/обрыв);
- preflight свободного места;
- тесты: resume, hash mismatch, cancel, pause, 5xx, content-type.

**Gate 2:**

- 100% тестов на Range/resume/hash проходят офлайн;
- отмена не оставляет staging-мусор;
- ошибки не содержат токенов;
- повторный запуск после обрыва продолжает с корректного offset.

---

### Этап 3. Инстансы (M)

**Цель:** изолированные игровые экземпляры без установки игры.

Задачи:

- `instance.json` schema v1 (версионирование, неизвестные поля сохраняются);
- CRUD: create, rename, clone, archive, delete (с подтверждением);
- отдельные каталоги `game/`, `mods/`, `config/`, `saves/`, `logs/`;
- общий кэш `assets/`, `libraries/`, `java/`;
- блокировки от параллельных операций;
- иконка/описание;
- миграция манифеста;
- UI списка и деталей инстанса;
- тесты изоляции: два инстанса не меняют файлы друг друга.

**Gate 3:**

- clone создаёт полную независимую копию;
- удаление не задевает общий кэш;
- два инстанса с разными версиями/модами сосуществуют;
- manifest round-trip сохраняет неизвестные поля.

---

### Этап 4. Minecraft Core и vanilla (L)

**Цель:** установка и запуск чистой официальной версии.

Задачи:

- Mojang version manifest + version JSON + кэш/ETag;
- резолвер библиотек: правила ОС/arch, modern natives как библиотеки,
  legacy `classifiers`/`natives`;
- assets: objects, index, virtual/legacy;
- client jar, logging config;
- проверка SHA-1 и размеров, повторная загрузка;
- установка в общий кэш + профиль версии;
- args builder: modern `arguments`/rules, legacy `minecraftArguments`,
  classpath, natives dir, placeholders;
- offline UUID и offline-профиль;
- запуск процесса, Job Object, логи, код возврата;
- диагностика: понятные ошибки (Java, файлы, память).

**Gate 4:**

- чистая 26.3 (и 1.20.1 как якорь модов) ставится с нуля;
- SHA-1 всех файлов проверен;
- игра запускается offline;
- повторный запуск не перекачивает файлы;
- память при установке не растёт линейно (стриминг).

---

### Этап 5. Java Runtime Manager (M)

**Цель:** автоматический подбор и установка Java.

Задачи:

- detect: PATH, реестр, `%ProgramFiles%`, `%LOCALAPPDATA%`, данные лаунчера;
- parse `java -version` для 8 и 9+;
- проверка архитектуры и major;
- Adoptium API, выбор JRE, SHA-256, безопасная распаковка;
- несколько runtime, назначение инстансу;
- удаление неиспользуемых (с подтверждением);
- UI управления;
- тесты парсинга и выбора.

**Gate 5:**

- для MC 26.2 автоматически предлагается Java 25;
- JRE 8 корректно определяется как 8, а не 1;
- неверная архитектура отклоняется;
- удаление runtime не ломает инстансы (проверка ссылок).

---

### Этап 6. Запуск vanilla и диагностика (M)

**Цель:** стабильный запуск, логи, crash report.

Задачи:

- выбор Java, памяти, JVM/game args, окно, fullscreen;
- валидация аргументов (без shell);
- статус процесса, остановка, повторный запуск;
- сбор stdout/stderr, метки времени;
- парсинг crash report, страница диагностики;
- экспорт отчёта без секретов;
- тесты args builder и redaction.

**Gate 6:**

- игра запускается с пользовательскими аргументами;
- лаунчер не позволяет запустить второй экземпляр того же инстанса;
- crash report отображается и экспортируется;
- в логах нет токенов.

---

### Этап 7. Аккаунты и Microsoft Auth (M)

**Цель:** официальная авторизация и безопасное хранение.

Задачи:

- провайдеры: offline, Microsoft; интерфейс для будущих (Ely.by, Azuriom);
- device code flow: Azure client ID, `consumers`, scopes;
- XBL → XSTS → Minecraft Services → профиль;
- refresh, срок действия, повторная авторизация, выход;
- Credential Manager/DPAPI, `secret_ref` в БД;
- redaction, `zeroize`;
- UI аккаунтов, переключение, статус;
- тесты: offline UUID, refresh, отсутствие секретов в логах.

**Gate 7:**

- реальный аккаунт входит и запускает игру;
- токен переживает перезапуск и обновляется;
- удаление аккаунта удаляет секрет;
- offline-профиль явно помечен и не выдаётся за лицензию.

**Блокер:** без Azure client ID этап не начинается.

---

### Этап 8. Загрузчики (L)

**Цель:** Fabric, Quilt, Forge, NeoForge.

Порядок внутри этапа:

1. **Fabric** — самый простой и доступный; meta + installer JSON.
2. **Quilt** — локально; в NEXUS не публикуется.
3. **Forge** — promotions + installer; headless, sandbox.
4. **NeoForge** — только при доступной сети/зеркале.

Задачи:

- adapter trait: list versions, compatibility, install plan, launch profile;
- проверка версии MC/loader;
- загрузка и проверка артефактов;
- запуск официального installer с ограниченным каталогом;
- регистрация профиля, libraries, args;
- идемпотентность и откат;
- тесты на фикстурах + реальная установка.

**Gate 8:**

- Fabric-инстанс ставится и запускается;
- Quilt-инстанс ставится и запускается (для 26.x загрузчик Quilt может быть
  beta — это нужно показывать в UI, а не скрывать);
- Forge-инстанс ставится и запускается;
- NeoForge — либо работает, либо этап явно помечен как заблокированный;
- повторная установка не ломает инстанс.

---

### Этап 9. Modrinth и зависимости (L)

**Цель:** поиск, установка, обновление, удаление модов.

Задачи:

- API-клиент, UA, кэш, rate limit;
- поиск/фильтры/сортировка/пагинация;
- project/version metadata;
- совместимость MC/loader/side;
- resolver: обязательные/опциональные зависимости, конфликты, циклы;
- план установки с подтверждением;
- транзакционная установка и откат;
- обновление и удаление;
- UI каталога и очереди.

**Gate 9:**

- реальный мод ставится с зависимостями;
- несовместимый мод отклоняется с причиной;
- конфликт версий показывается, а не «молча ставится»;
- откат возвращает инстанс в согласованное состояние.

---

### Этап 10. Модпаки (L)

**Цель:** импорт/экспорт сборок.

Порядок:

1. `.mrpack` — спецификация Modrinth.
2. Prism/MultiMC (`mmc-pack.json`, `instance.cfg`).
3. Обычный ZIP с игровой структурой.
4. HMCL — в пределах документированного формата.
5. CurseForge ZIP — только при решённом API key.

Задачи:

- определение формата по содержимому, а не расширению;
- проверка архива (zip slip/bomb/limits);
- manifest validation;
- overrides без path traversal;
- создание инстанса, установка недостающего;
- экспорт без токенов/локальных путей;
- UI импорта с предпросмотром плана.

**Gate 10:**

- реальный `.mrpack` импортируется и запускается;
- Prism-сборка импортируется;
- архив с `../` отклоняется;
- экспорт не содержит секретов.

---

### Этап 11. NEXUS Builds (M)

**Цель:** установка опубликованных сборок NEXUS.

Порядок:

1. Публичный каталог (`/api/packs`) без токена.
2. Версии, манифест, файлы, SHA-256, client-only.
3. Несколько источников, проверка URL, HTTPS, отключение.
4. Приватный режим через внутренний API + Bearer (если решено).

Задачи:

- клиент контракта, строгая десериализация, игнор неизвестных полей;
- проверка `manifestVersion`, `packId`, `version`, путей, hash, size;
- загрузка с retry/backoff, fallback на полный файл;
- проверка `Content-Type`, не доверять 200;
- создание инстанса, сохранение `installed_builds`;
- обновление только по кнопке, не автоматически;
- работа при недоступном сервере: локальные инстансы живы;
- UI: источники, каталог, детали, установка, ошибки.

**Gate 11:**

- реальная сборка `vanilla 1.1` (MC 26.2/Fabric) ставится из живого сервера;
- все 124 файла проходят SHA-256;
- при 500/недоступности показывается реальная ошибка, данные не теряются;
- приватные сборки не доступны без токена;
- серверные файлы не попадают в клиентский инстанс.

---

### Этап 12. Локальные серверы (L)

**Цель:** создание, запуск и управление серверами.

Задачи:

- мастер: имя, версия, тип, loader, каталог, Java, RAM, порт;
- установка vanilla server jar; Fabric/Forge/NeoForge/Quilt server;
- EULA: явное подтверждение, запись `eula=true` только после него;
- supervisor: stdin/stdout/stderr, лог, команды, мягкая остановка,
  принудительное завершение, перезапуск;
- auto-restart с backoff и лимитом;
- recovery после перезапуска лаунчера (supervisor или log-only — решение D09);
- `server.properties` редактор с валидацией и пометкой «требует рестарта»;
- `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`,
  `usercache.json` — атомарные изменения, backup;
- UI: список, статус, RAM/CPU/PID, консоль с поиском, игроки, настройки.

**Gate 12:**

- сервер создаётся, запускается, принимает команду, останавливается;
- EULA нельзя принять автоматически;
- после перезапуска лаунчера статус и консоль восстанавливаются согласно
  выбранной схеме;
- auto-restart не уходит в бесконечный цикл;
- PID не считается достаточным доказательством.

---

### Этап 13. Резервные копии (M)

**Цель:** предсказуемое резервное копирование и восстановление.

Задачи:

- backup инстанса, мира, сервера;
- выбор каталогов, исключения (`session.lock`, кэши);
- согласованность работающего сервера (`save-off`/`save-all`);
- расписание, retention, удаление старых;
- restore с подтверждением, staging, атомарная замена;
- проверка целостности архива;
- UI: создание, список, восстановление, удаление.

**Gate 13:**

- backup/restore инстанса и мира;
- restore не удаляет текущие данные без подтверждения;
- backup работающего сервера консистентен;
- retention не удаляет последний валидный бэкап.

---

### Этап 14. Полировка и диагностика (M)

Задачи:

- финальный UI, анимации, пустые состояния, ошибки;
- страница логов с фильтрами/поиском/копированием/экспортом;
- crash report;
- проверка адаптации окна, клавиатуры, reduced motion;
- производительность: старт, RAM, FPS фона;
- accessibility-минимум;
- документация пользователя.

**Gate 14:**

- нет декоративных кнопок;
- все ошибки имеют понятный текст и код;
- экспорт диагностики не содержит секретов;
- performance budget выполнен.

---

### Этап 15. Релиз (M)

Задачи:

- Tauri updater: ключи, подпись, HTTPS-эндпоинт, каналы;
- сертификат подписи кода;
- MSI и NSIS, per-user;
- чистая установка/удаление, сохранение данных;
- проверка обновления и отката;
- финальный аудит зависимостей и уязвимостей;
- release notes, лицензии третьих сторон, юридические тексты.

**Gate 15:**

- установщик подписан;
- обновление проверяет подпись и не удаляет данные;
- откат работает;
- чистая машина: установка → запуск → удаление;
- документация и лицензии на месте.

---

## 5. Параллельные работы

| Работа | Когда | Условие |
|---|---|---|
| Дизайн-система и макеты | этап 0–1 | не блокирует Rust |
| Документация контрактов | этап 0 | версия и фикстуры |
| Security-тесты fs/zip | этап 1–2 | до первого импорта архивов |
| CI и dependency-аудит | этап 0 | git установлен |
| Azure app registration | до этапа 7 | внешняя |
| CurseForge legal/key | до этапа 10 | внешняя |
| NeoForge network | до этапа 8 | внешняя |
| Подпись/updater keys | до этапа 15 | внешняя |

---

## 6. Модель данных (черновик, не создаётся на этом этапе)

SQLite, WAL, `busy_timeout`, миграции SQLx. Все таблицы имеют `id`
(TEXT/UUID или INTEGER), `created_at`, `updated_at`; удаление — явное, с
политикой для связанных записей.

| Таблица | Назначение | Ключевые поля |
|---|---|---|
| `app_settings` | настройки | `key`, `value_json` |
| `instances` | инстансы | `name`, `slug`, `game_version`, `loader_type`, `loader_version`, `java_runtime_id`, `memory_min/max`, `game_dir`, `icon_path`, `archived`, `last_played_at` |
| `instance_files` | файлы инстанса | `instance_id`, `path`, `category`, `side`, `size`, `sha256`, `enabled`, `required`, `source` |
| `accounts` | аккаунты | `provider`, `username`, `uuid`, `display_name`, `secret_ref`, `last_used_at` |
| `java_runtimes` | Java | `major`, `arch`, `path`, `source`, `sha256`, `installed_at` |
| `downloads` | задачи | `kind`, `url`, `dest`, `size`, `sha256`, `status`, `bytes_done`, `error`, `priority` |
| `download_parts` | resume | `download_id`, `offset`, `etag`, `updated_at` |
| `servers` | серверы | `name`, `dir`, `game_version`, `loader`, `loader_version`, `java_runtime_id`, `memory_mb`, `port`, `auto_restart_json` |
| `server_processes` | процессы | `server_id`, `pid`, `started_at`, `exe_path`, `cmd_hash`, `status` |
| `backups` | бэкапы | `target_type`, `target_id`, `path`, `size`, `sha256`, `created_at` |
| `nexus_sources` | источники | `name`, `base_url`, `enabled`, `auth_mode`, `secret_ref`, `last_checked_at`, `status` |
| `installed_builds` | установленные сборки | `instance_id`, `source_id`, `pack_id`, `version`, `manifest_hash`, `installed_at` |
| `mod_metadata` | моды | `instance_id`, `source`, `project_id`, `version_id`, `name`, `version`, `side`, `hash` |
| `activity_logs` | журнал | `at`, `kind`, `entity`, `summary`, `details_json` (без секретов) |

Правила:

- секреты — только `secret_ref`, не значения;
- `instance.json` — источник истины для переносимого инстанса; БД — индекс;
- миграции только вперёд, с тестом апгрейда;
- каскадное удаление — только после проверки ссылок (общий кэш нельзя
  удалять, пока на него ссылается другой инстанс).

---

## 7. IPC и события

### 7.1. Команды

Именование: `<domain>_<action>`, например:

```
app_info
settings_get / settings_update
instances_list / instances_create / instances_update / instances_delete
instances_clone / instances_archive
minecraft_versions / minecraft_install
java_list / java_detect / java_install
launch_instance / stop_instance / process_status
accounts_list / accounts_add / accounts_remove / accounts_refresh
mods_search / mods_install / mods_remove / mods_update
modpacks_import / modpacks_export
downloads_list / downloads_pause / downloads_resume / downloads_cancel
servers_list / servers_create / servers_start / servers_stop
servers_restart / servers_command / servers_console
nexus_sources / nexus_catalog / nexus_install / nexus_update
backups_create / backups_restore / backups_delete
diagnostics_export
```

Длинные операции возвращают `jobId` сразу, результат — событием. Это
позволяет отменять и не блокировать UI.

### 7.2. Ошибки

```json
{
  "code": "INSTANCE_NOT_FOUND",
  "message": "Инстанс не найден.",
  "details": { "id": "..." },
  "requestId": "..."
}
```

Запрещено отдавать в UI сырые пути, SQL, тела HTTP и токены.

### 7.3. События

```
download:started|progress|completed|failed
instance:created|install-progress|installed
minecraft:launching|started|stopped|crashed
server:starting|started|stopped|crashed|console|metrics
nexus:connection-status|catalog-updated|install-progress
app:error
```

Каждое событие содержит `jobId`/`instanceId`/`serverId`. Прогресс
троттлится (например, 10 Гц), чтобы не забивать IPC.

---

## 8. Минимальные требования к тестированию

Полный перечень — в `docs/AUDIT.md` §15. Кратко:

### 8.1. Обязательно на каждом этапе

1. `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`.
2. `npm run typecheck`, `npm run lint`, `npm run test`, `npm run build`.
3. Новый код покрыт unit-тестами; security-кейсы — отдельными тестами.
4. Offline: unit-тесты не ходят в сеть.
5. Фикстуры внешних API с датой и источником.
6. Ручной smoke-сценарий этапа.

### 8.2. Обязательные security-тесты

- path traversal, абсолютные пути, UNC, ADS;
- reserved names, trailing dot/space, NFC, case collision;
- symlink/junction escape;
- zip slip, zip bomb, повреждённый архив;
- подмена файла, неверный SHA-256/размер;
- SSRF и редиректы;
- утечка токенов в логах и ошибках;
- неавторизованный вызов команд (frontend не может обойти проверку).

### 8.3. Обязательные интеграционные тесты

- установка vanilla (реальная сеть, opt-in);
- установка Fabric/Quilt/Forge (NeoForge — по доступности);
- импорт `.mrpack`;
- resume загрузки;
- установка NEXUS-сборки;
- запуск локального сервера и восстановление;
- миграция БД.

### 8.4. Критерии покрытия

- path/fs-safety, hash, args builder, secret redaction — 100% ветвей
  выявленных security-кейсов;
- остальной доменный код — не ниже 80% строк;
- UI — критические stores и компоненты, без погони за процентом.

### 8.5. Performance budget (проверяется на этапе 14)

- холодный старт UI до интерактивности: ≤ 3 с на целевой машине;
- тёплый старт: ≤ 1,5 с;
- idle RAM (без игры): ≤ 300 МБ;
- фон: 60 FPS cap, пауза при скрытии окна;
- установка: память не растёт линейно с размером файла;
- прогресс/события не чаще 10 Гц на задачу.

---

## 9. Definition of Done

Этап считается завершённым, когда:

1. Все задачи этапа выполнены, нет `TODO` в критическом пути.
2. Тесты этапа и всех предыдущих проходят.
3. Security-гейты этапа пройдены.
4. Ручной сценарий принят на Windows 11.
5. Документация обновлена (README/ADR/контракт).
6. Нет новых критических рисков без митигации.
7. Демо показывает реальный результат, а не макет.

---

## 10. Вне scope v1

- OptiFine и LiteLoader (кроме исследования);
- Linux и macOS;
- ARM64;
- delta-обновления;
- синхронизация инстансов между устройствами;
- телеметрия и аналитика;
- встроенный браузер модов/серверов;
- управление скинами и плащами;
- экспорт в CurseForge;
- приватные сборки NEXUS без явного UX ввода токена;
- серверные сборки NEXUS через публичный API;
- автообновление до появления ключей/хостинга;
- поддержка пиратских/обходных методов авторизации.

---

## 11. Следующий этап (предложение)

**Этап 0 «Фундамент и контракты»** — без игровой функциональности.

Конкретный чек-лист:

- [ ] Установить git, `git init`, первый коммит с текущими документами.
- [ ] `.gitignore`, `.gitattributes`, `.editorconfig`, `README.md`.
- [ ] Скопировать серверные контракты и схемы в `docs/reference/`,
      зафиксировать `CONTRACT_VERSION = 1.0.0` и дату.
- [ ] ADR-001: Cargo workspace и границы крейтов.
- [ ] ADR-002: baseline зависимостей (TS 6.0.3, reqwest 0.13, sqlx 0.9,
      zip 9, keyring 3.6/4, windows 0.62).
- [ ] ADR-003: LocalAppData, per-user, SQLite/WAL.
- [ ] ADR-004: роутинг и непрерывный фон (Canvas/CSS vs Three.js).
- [ ] `docs/ARCHITECTURE.md`, `docs/API-INTEGRATION.md`, `docs/DATABASE.md`,
      `docs/UI-DESIGN.md`.
- [ ] Пустой workspace + frontend tooling + CI.
- [ ] Прогнать `cargo check`, `npm ci`, `npm run build`, `tauri info`.
- [ ] Зафиксировать открытые решения D01–D18 из аудита.

После Gate 0 переходить к этапу 1 (оболочка). Реализацию Minecraft, auth,
загрузчиков и NEXUS начинать только после соответствующих внешних решений
(C3–C6).
