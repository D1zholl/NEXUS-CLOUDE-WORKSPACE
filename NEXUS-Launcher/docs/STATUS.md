# NEXUS-launcher — фактический статус

**Дата аудита:** 21.11.2026  
**Объект:** `C:\Users\max\Desktop\MWS-NEXUS\NEXUS-Launcher`  
**Принцип документа:** здесь фиксируется только то, что подтверждено файлом,
командой или HTTP-ответом. Всё остальное помечено как «нет» или «не проверено».

---

## 1. Краткий вывод

- Исходного кода лаунчера **нет**: ни `Cargo.toml`, ни `src-tauri/`, ни
  `package.json`, ни `frontend/`, ни миграций, ни тестов.
- Репозиторий **не является git-репозиторием**, а `git` в системе не установлен.
- В каталоге фактически лежат только техническое задание и копия трёх
  документов серверной части NEXUS Workspace.
- Среда разработки пригодна для Tauri 2 + Rust + Node + SQLite. Сборка под
  Windows технически возможна; это подтверждено референсным приложением
  `MC-launcher` на этой же машине, но **сам NEXUS-launcher ещё ни разу не
  собирался**.
- Для старта разработки не хватает четырёх внешних вещей: git, Azure-приложения
  для Microsoft Auth, решения по CurseForge API и сетевого доступа к
  `maven.neoforged.net` (сейчас недоступен).

---

## 2. Что физически существует

Все файлы проекта (полный обход каталога):

| Файл | Размер | SHA-256 (начало) | Назначение |
|---|---:|---|---|
| `MAIN-DEV-PLAN.md` | 62 312 Б | `8875CFC143CF0A24…` | Полное ТЗ, 1 654 строки |
| `NEXUS-server-docs/README.md` | 34 624 Б | `2665428480BC9690…` | Копия README сервера NEXUS Workspace 1.0.0 |
| `NEXUS-server-docs/LAUNCHER_CONTRACT.md` | 16 694 Б | `7051B4632B476634…` | Контракт MyLauncher ↔ NEXUS |
| `NEXUS-server-docs/MANIFEST.md` | 28 735 Б | `3AC6656BBBF84179…` | Формат манифеста сборки, schema 1.0 |
| `NEXUS-server-docs/Пример оформления интерфейса с 3d фоном.png` | 871 726 Б | — | Референс дизайна |

Проверено: `MANIFEST.md` и `LAUNCHER_CONTRACT.md` **побайтово совпадают** с
`C:\Users\max\Desktop\MWS-NEXUS\docs\`. `README.md` также побайтово совпадает с
корневым `C:\Users\max\Desktop\MWS-NEXUS\README.md`. Это именно копии, а не
самостоятельные документы лаунчера.

### Чего в репозитории нет

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`, `src-tauri/`;
- `package.json`, `package-lock.json`, `frontend/`, `node_modules/`;
- `migrations/`, `capabilities/`, `icons/`, `resources/`;
- `tests/`, `docs/` (до создания этого аудита), `README.md` лаунчера;
- `.gitignore`, `.editorconfig`, лицензии, CI-конфигурации;
- какого-либо рабочего кода, заглушек, схем БД или Tauri-конфига.

**Количество строк Rust:** 0.  
**Количество строк TypeScript/TSX:** 0.  
**Количество миграций:** 0.  
**Количество тестов лаунчера:** 0.

---

## 3. Инструменты (проверено командами)

| Инструмент | Версия / состояние | Проверка |
|---|---|---|
| rustc | 1.100.0 (0ef1bc386 2026-11-09) | `rustc --version` |
| cargo | 1.100.0 (3acc79c0c 2026-11-05) | `cargo --version` |
| rustup | 1.29.3 (edbc831ce 2026-11-02) | `rustup --version` |
| Активный toolchain | `stable-x86_64-pc-windows-msvc` | `rustup show` |
| Установленные targets | `x86_64-pc-windows-msvc`, `wasm32-unknown-unknown` | `rustup show` |
| Cargo-утилиты | только `wasm-bindgen-cli 0.2.130` | `cargo install --list` |
| Node.js | 26.13.1 | `node --version` |
| npm | 11.20.0 | `npm --version` |
| npx | 11.20.0 | `npx --version` |
| Python | 3.12.10 | `python --version` |
| PowerShell | 5.1.26100.9444 | `$PSVersionTable` |
| winget | 1.29.380 | `winget --version` |
| .NET runtimes | 8.0.22, 10.0.0 | `dotnet --list-runtimes` |
| Git | **не установлен** | `git --version` → command not found |
| pnpm / yarn | **не установлены** | `where.exe pnpm/yarn` |
| sqlite3 CLI | **не установлен** | `sqlite3 --version` |
| Tauri CLI (глобально) | **не установлен** | `cargo tauri --version` → no such command |
| Tauri CLI через npx | работает, скачан `@tauri-apps/cli 2.12.6` | `npx @tauri-apps/cli@2 info` |
| Playwright browsers | **не установлены** (системные Edge и Chrome есть) | проверка каталогов |
| Cargo registry cache | 819 файлов `.crate` | обход `~/.cargo/registry/cache` |
| npm cache | пуст | `npm config get cache` |
| WiX / NSIS cache Tauri | **отсутствует** (`%LOCALAPPDATA%\tauri` нет) | обход каталога |

Важно: глобальный `cargo-tauri` не нужен — правильный путь для проекта это
`@tauri-apps/cli` в `devDependencies`. Его отсутствие не является блокером.

---

## 4. Система и целевая платформа

| Параметр | Значение |
|---|---|
| ОС | Windows 11, 26H2, build 26300 |
| Архитектура | x64 (AMD64) |
| CPU | Intel Core 5 210H, 8 ядер / 12 потоков |
| RAM | 16 852 574 208 Б (~15,7 ГиБ) |
| Диск C: | NTFS, 511 018 266 624 Б, свободно 64 203 866 112 Б (~59,8 ГиБ) |
| Длинные пути | включены (`LongPathsEnabled=1`) |
| Права процесса | администратор |
| Visual Studio | Professional 2026, `C:\Program Files\Microsoft Visual Studio\18\Professional` |
| Windows SDK | 10.0.26100.0 |
| WebView2 Runtime | 157.0.4341.60 |
| Системная Java | JRE 1.8.0_503 (Oracle), других системных JDK нет |
| Java в данных MyLauncher | JRE 21 (`jdk-21.0.13.1+2-jre`) |
| `.minecraft` | отсутствует |
| Кодовая страница консоли | 866; ANSI — windows-1251; locale ru-RU |
| Developer Mode | не включён (значение не задано) |

---

## 5. Сеть: что реально доступно (21.11.2026)

| Источник | Результат |
|---|---|
| crates.io | 200 |
| registry.npmjs.org | 200 |
| Mojang version manifest | 200, 925 версий |
| Mojang libraries.minecraft.net | 200 |
| Fabric Meta / Maven | 200 |
| Quilt Meta / Maven | 200 |
| Forge promotions / Maven | 200 |
| **NeoForge Maven (`maven.neoforged.net`)** | **TCP 443 недоступен, все попытки ~21 с → timeout** |
| Modrinth API | 200 |
| CurseForge API | 403 без API-ключа |
| CurseForge www | 403 (Cloudflare) |
| Forge CDN `edge.forgecdn.net` | 200 |
| Adoptium API | 200 |
| Microsoft login / Xbox Live | endpoints отвечают (400/405 на probe — нормально) |
| GitHub | 200 |

NeoForge — единственный подтверждённый сетевой блокер. Он не мешает этапам
0–4, но мешает установке NeoForge без прокси/зеркала.

---

## 6. Живой NEXUS Workspace (использовался как реальный контракт)

- Процесс: `nexus.exe`, PID 7948, запущен 21.11.2026 20:11:45,
  путь `C:\Users\max\Desktop\MWS-NEXUS\backend\target\release\nexus.exe`.
- Адрес: `http://127.0.0.1:8080`.
- OpenAPI: 3.1.0, `NEXUS WORKSPACE API`, версия 1.0.0, 117 путей.
- Публичный каталог: `/api/packs?channel=stable` → 200, одна сборка `vanilla`,
  `latestVersion = 1.1`.
- Манифест `vanilla/1.1`: `manifestVersion=1`, Minecraft `26.2`,
  Fabric `0.20.1`, Java `25`, 124 файла, суммарно ~60,5 МБ, `settings={}`.
- Манифест **прошёл валидацию** по `docs/schemas/launcher-manifest-1.json`
  (JSON Schema 2020-12) через `jsonschema 4.26.0`.
- Внутренний Launcher API (`/api/v1/launcher/*`) без токена → 401
  `LAUNCHER_TOKEN_REQUIRED`.

### Аномалии, зафиксированные в момент аудита

| Наблюдение | Следствие |
|---|---|
| 5 запросов к `/api/files/...` вернули 500 за ~10 с | В логе сервера: `pool timed out while waiting for an open connection`; пул PostgreSQL `max_connections=10`, `acquire_timeout=10s` |
| Один запрос `Range` получил 416 только через 4,9 с | Деградация из-за ожидания соединения с БД |
| `/api/files/` (без хэша) вернул 200 `text/html` | SPA-fallback; нельзя считать 200 признаком API-ответа |
| `/api/packs` без `channel` вернул 200 | Контракт говорит, что `channel` обязателен |
| OpenAPI для `/api/files/{hash}` содержит только 200/404/429/500 | 206 и 416 фактически поддерживаются, но не описаны; query-параметры `packId/version/channel` тоже не описаны |
| `/api/packs/vanilla/icon` → 404 | У сборки нет иконки (`iconUrl=null`), это нормально |

Повторные проверки после паузы показали 200/206. То есть это не постоянный
баг протокола, а периодическая деградация сервера при исчерпании пула БД.
Для лаунчера это означает обязательные retry/backoff и fallback на полную
загрузку, а не «диагноз: Range сломан».

---

## 7. Референсный клиент `MC-launcher`

`C:\Users\max\Desktop\MC-launcher` — существующее Tauri-приложение MyLauncher
0.1.0. Оно **не является частью NEXUS-launcher** и не должно копироваться
целиком, но это полезный источник проверенных версий и найденных решений.

| Параметр | Факт |
|---|---|
| Tauri | 2.12.5, `tauri-build 2.7.4`, `wry 0.57.1`, `tao 0.37.2` |
| Rust | edition 2024, 12 файлов |
| Самый большой файл | `modpack.rs` — 123 022 Б; `minecraft.rs` — 118 650 Б; `modrinth.rs` — 93 219 Б; `server.rs` — 84 921 Б |
| Frontend | React 19.3.1, Vite 8.3.5, TypeScript 6.0.3, Vitest 5.0.5 |
| Rust-тесты | 81 функция `#[test]`/`#[tokio::test]` в 9 модулях |
| Frontend-тесты | 1 файл (`palettes.test.ts`), 2 теста |
| SQLite | `launcher.db`, таблицы `accounts`, `instances`, `settings`; инстансов 0 |
| Секреты | `keyring`; Microsoft `client_id` в БД пуст |
| Сборка | `my-launcher.exe` 10 341 888 Б; MSI 6 279 168 Б; NSIS 3 760 928 Б (31.10.2026) |
| Моно­литность | высокая; это антипример для структуры нового проекта |

---

## 8. Доказательства возможности сборки под Windows

| Проверка | Результат |
|---|---|
| Минимальный Rust-проект (edition 2024) | `cargo run` → `link-ok` |
| `npx @tauri-apps/cli@2 info` | WebView2 ✔, MSVC ✔, rustc ✔, cargo ✔, rustup ✔, Rust target ✔, node/npm ✔ |
| Референсное Tauri-приложение | собран `.exe`, MSI и NSIS на этой машине |
| Сам NEXUS-launcher | **не собирался: кода нет** |

Ограничения: собирается только x64; target ARM64 не установлен; WiX/NSIS
Tauri скачает при первой упаковке (сеть к GitHub есть); подписи кода нет.

---

## 9. Тесты

| Что | Состояние |
|---|---|
| Тесты NEXUS-launcher | отсутствуют, запускать нечего |
| Результаты тестов NEXUS-launcher | **нет и не может быть на этой стадии** |
| Тесты референсного клиента | существуют (81 Rust + 2 frontend), но в рамках аудита **не запускались** |
| Аудит-проверки | `cargo run` link-test, `cargo metadata`, HTTP-проверки, JSON Schema валидация — это проверки среды и контракта, а не тесты продукта |

Никакие «332/332», «63/63» и подобные цифры из серверного README к лаунчеру
не относятся и в статус не переносятся.

---

## 10. Документация

| Документ | Состояние |
|---|---|
| `MAIN-DEV-PLAN.md` | есть, 1 654 строки, содержит полное ТЗ и этапы 0–10 |
| `LAUNCHER_CONTRACT.md` | есть, актуален, совпадает с серверным |
| `MANIFEST.md` | есть, актуален, совпадает с серверным |
| `README.md` сервера | есть, но описывает NEXUS Workspace, а не лаунчер |
| `API.md`, `openapi.json`, `ARCHITECTURE.md`, `DATABASE.md`, `SECURITY.md`, `TESTING.md` | **отсутствуют в каталоге лаунчера**; лежат в `C:\Users\max\Desktop\MWS-NEXUS\docs\` |
| JSON-схемы (`manifest-1.0.json`, `launcher-manifest-1.json`) | **отсутствуют в каталоге лаунчера**; лежат в `…\MWS-NEXUS\docs\schemas\` |
| `docs/AUDIT.md`, `docs/ROADMAP.md`, `docs/STATUS.md` | созданы этим аудитом |

Несоответствие путей: ТЗ ссылается на `docs/LAUNCHER_CONTRACT.md`, а файл
лежит в `NEXUS-server-docs/LAUNCHER_CONTRACT.md`. Это нужно исправить
копированием/ссылкой, а не переписыванием контракта.

---

## 11. Блокеры и отсутствующие внешние ресурсы

| Блокер | Влияние | Статус |
|---|---|---|
| Нет git | нет версионирования, CI, code review | блокер этапа 0 |
| Нет Azure app / Microsoft client ID | Microsoft Auth нельзя реализовать и проверить | блокер этапа 5 |
| Нет CurseForge API key | CurseForge и импорт CF-модпаков невозможны | блокер этапа 6 |
| NeoForge Maven недоступен | установка NeoForge невозможна с этой сети | блокер/дефер этапа 4 |
| Нет сертификата подписи кода | SmartScreen-предупреждения, нет доверенного автообновления | риск релиза |
| Нет ключей Tauri updater и хостинга обновлений | автообновление невозможно | этап 10 |
| Нет Playwright browsers | UI E2E потребует установки или системного Edge/Chrome | не блокер |

---

## 12. Что это значит

Проект находится **до этапа 0**. Всё, что можно честно сказать сейчас:
«есть ТЗ, копия серверных контрактов и пригодная среда». Нельзя сказать
«есть каркас», «зависимости установлены», «тесты проходят» — этого нет.

Следующий разумный шаг — этап 0/1: инициализация репозитория и
инфраструктуры без игровой функциональности (см. `docs/ROADMAP.md`).
