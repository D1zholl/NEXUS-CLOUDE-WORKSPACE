# Технический аудит NEXUS-launcher

**Дата:** 21.11.2026  
**Версия документа:** 1.0  
**Статус:** аудит завершён, реализация не начиналась  
**Объект:** `C:\Users\max\Desktop\MWS-NEXUS\NEXUS-Launcher`  
**Связанные документы:** `docs/STATUS.md`, `docs/ROADMAP.md`, `MAIN-DEV-PLAN.md`,
`NEXUS-server-docs/LAUNCHER_CONTRACT.md`, `NEXUS-server-docs/MANIFEST.md`

---

## 1. Резюме

### 1.1. Вердикт

**Среда пригодна. Проект не начат. Запуск возможен при выполнении условий.**

Фактическое состояние:

- в каталоге нет ни одной строки кода лаунчера;
- нет git-репозитория, CI, линтеров, тестов, миграций и конфигурации Tauri;
- есть полное ТЗ на 1 654 строки и три серверных документа, описывающих
  реальный контракт NEXUS Workspace;
- Rust 1.100 + MSVC (VS 2026) + Windows SDK + WebView2 157 по версиям и
  предпосылкам совместимы с Tauri 2; это подтверждено линковкой минимального
  Rust-проекта и наличием собранного референсного Tauri-приложения на этой же
  машине. Полная сборка Tauri именно на rustc 1.100 ещё не выполнялась;
- сам NEXUS-launcher ещё не собирался, потому что его не существует.

### 1.2. Условия старта

| № | Условие | Почему критично |
|---|---|---|
| C1 | Установить git и инициализировать репозиторий | Без VCS нельзя вести этапы, откатывать и проверять изменения |
| C2 | Зафиксировать базовые версии зависимостей (ADR) | TS 7 несовместим с текущим линтером; sqlx и rusqlite конфликтуют |
| C3 | Зарегистрировать Azure-приложение и получить client ID | Без него Microsoft Auth не реализуется |
| C4 | Решить судьбу CurseForge API key | Ключ нельзя безопасно встраивать в desktop-клиент |
| C5 | Решить вопрос NeoForge (сеть/зеркало/дефер) | `maven.neoforged.net` недоступен с этой машины |
| C6 | Принять ограничение приватных/серверных сборок NEXUS | Публичный контракт отдаёт только клиентские файлы |
| C7 | Выбрать схему восстановления консоли сервера | Переподключение к stdio после перезапуска лаунчера невозможно без супервизора |
| C8 | Подготовить подпись кода и ключи обновлений до релиза | Иначе SmartScreen и недоверенное автообновление |

---

## 2. Методика и границы аудита

### 2.1. Что проверялось

1. Полная инвентаризация файлов репозитория, размеры, SHA-256.
2. Версии Rust, Cargo, rustup, toolchain, targets, установленные cargo-утилиты.
3. Node.js, npm, npx, Python, PowerShell, .NET, winget.
4. MSVC, Windows SDK, WebView2, Visual Studio, ОС и железо.
5. Возможность компиляции и линковки Rust под `x86_64-pc-windows-msvc`.
6. `tauri info` через `npx @tauri-apps/cli@2` (read-only диагностика).
7. Разрешение зависимостей через `cargo metadata` на временных probe-проектах
   (без компиляции и без изменения рабочего каталога).
8. Фактические версии npm-пакетов и их peer-требования.
9. Сетевую доступность Mojang, Fabric, Quilt, Forge, NeoForge, Modrinth,
   CurseForge, Adoptium, Microsoft, GitHub, crates.io, npm.
10. Живой сервер NEXUS Workspace на `127.0.0.1:8080`: каталог, версии,
    манифест, файлы, Range, коды ошибок, OpenAPI, внутренний API.
11. Валидацию живого манифеста по JSON Schema сервера.
12. Референсный клиент `MC-launcher`: версии, структура, тесты, артефакты,
    схема SQLite, состояние Microsoft-конфигурации.
13. Сверку ТЗ с реальными контрактами и ограничениями платформы.

### 2.2. Что не проверялось (и не должно считаться проверенным)

- установка и запуск Minecraft (ни одной версии);
- Microsoft OAuth, Xbox Live, XSTS, Minecraft Services;
- CurseForge API с ключом;
- NeoForge (сеть недоступна);
- исполнение Forge/NeoForge/Fabric/Quilt-инсталляторов;
- E2E-тесты Tauri/WebDriver;
- производительность UI, фоновой анимации, загрузчика;
- поведение под антивирусом, SmartScreen, UAC, другим пользователем Windows;
- автообновление;
- сборка MSI/NSIS именно NEXUS-launcher;
- юридическая совместимость (Minecraft EULA, CurseForge Terms, Adoptium).

### 2.3. Воспроизводимость

Все ключевые команды и результаты сведены в Приложении A. Проверки,
изменяющие состояние, не выполнялись. Временные probe-проекты находятся в
`C:\Users\max\AppData\Local\Temp\opencode\` и к репозиторию не относятся.

---

## 3. Состояние репозитория

### 3.1. Инвентаризация

| Файл | Размер | Комментарий |
|---|---:|---|
| `MAIN-DEV-PLAN.md` | 62 312 Б | ТЗ, 1 654 строки, этапы 0–10 |
| `NEXUS-server-docs/README.md` | 34 624 Б | README сервера, побайтово равен корневому README MWS-NEXUS |
| `NEXUS-server-docs/LAUNCHER_CONTRACT.md` | 16 694 Б | Побайтово равен `MWS-NEXUS/docs/LAUNCHER_CONTRACT.md` |
| `NEXUS-server-docs/MANIFEST.md` | 28 735 Б | Побайтово равен `MWS-NEXUS/docs/MANIFEST.md` |
| `NEXUS-server-docs/Пример оформления интерфейса с 3d фоном.png` | 871 726 Б | Дизайн-референс |

`git status` невозможен: git не установлен, каталог не является репозиторием.

### 3.2. Отсутствующие элементы проекта

- Rust: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src-tauri/`,
  `build.rs`, `tauri.conf.json`, `capabilities/`, `icons/`.
- Frontend: `package.json`, `package-lock.json`, `frontend/`, `index.html`,
  `vite.config.ts`, `tsconfig.json`, ESLint/Prettier-конфиги.
- Данные: `migrations/`, схема SQLite, миграционная политика.
- Тесты: `tests/`, фикстуры, mock-сервер, E2E.
- Инфраструктура: `.gitignore`, `.editorconfig`, `LICENSE`, `README.md`,
  CI-workflow, шаблоны issue/PR.
- Документация этапа 0 из ТЗ: `Architecture.md`, `API Integration.md`,
  `Database.md`, `UI Design Specification.md` — не созданы.

### 3.3. Проблемы документации

1. `NEXUS-server-docs/README.md` описывает сервер, а не лаунчер. Если положить
   его в корень как `README.md`, новый участник получит неверную картину.
2. ТЗ ссылается на `docs/LAUNCHER_CONTRACT.md`, фактический файл лежит в
   `NEXUS-server-docs/`. Нужна единая точка правды (копия в `docs/reference/`
   либо ссылка), иначе документы разойдутся.
3. В каталоге лаунчера нет `API.md`, `openapi.json`, `ARCHITECTURE.md`,
   `DATABASE.md`, `SECURITY.md`, `TESTING.md`, `docs/schemas/*.json`. Они есть
   в соседнем `MWS-NEXUS/docs`. Без них этап 0 из ТЗ выполнить нельзя.
4. Версия контракта не зафиксирована в лаунчере (нет `CONTRACT_VERSION`).
   Сервер уже 1.0.0; нужна явная фиксация, какой версии контракта следует
   клиент.

---

## 4. Toolchain и платформа

### 4.1. Rust

| Параметр | Значение |
|---|---|
| rustc | 1.100.0 (0ef1bc386 2026-11-09) |
| cargo | 1.100.0 (3acc79c0c 2026-11-05) |
| rustup | 1.29.3 (edbc831ce 2026-11-02) |
| Активный toolchain | stable-x86_64-pc-windows-msvc (default) |
| Targets | x86_64-pc-windows-msvc, wasm32-unknown-unknown |
| Cargo-утилиты | wasm-bindgen-cli 0.2.130 |
| Nightly | не установлен |

MSRV предлагаемого набора: максимум 1.95 (`sysinfo 0.40.4`). rustc 1.100
удовлетворяет всем проверенным крейтам. `wasm32` не нужен для лаунчера, но
наличию не мешает.

### 4.2. Node.js и npm

| Параметр | Значение |
|---|---|
| Node.js | 26.13.1 |
| npm | 11.20.0 |
| npx | 11.20.0 |
| pnpm / yarn | не установлены |
| npm cache | пуст |
| Registry | `https://registry.npmjs.org/` |

Node 26 удовлетворяет требованиям Vite 8 (`^20.19.0 || >=22.12.0`),
Vitest 5 (`^22.12.0 || ^24.0.0 || >=26.0.0`), jsdom 31 (`>=26.0.0`) и
React Router 8 (`>=22.22.0`).

### 4.3. Система

| Параметр | Значение |
|---|---|
| ОС | Windows 11, 26H2, build 26300 |
| Архитектура | x64 |
| Visual Studio | Professional 2026 |
| Windows SDK | 10.0.26100.0 |
| WebView2 | 157.0.4341.60 |
| RAM | ~15,7 ГиБ |
| Диск C: | NTFS, ~59,8 ГиБ свободно |
| Длинные пути | включены |
| Права | администратор |
| PowerShell | 5.1 |
| ANSI code page | windows-1251, консоль 866, locale ru-RU |
| .NET | 8.0.22, 10.0.0 |
| Python | 3.12.10 |
| winget | 1.29.380 |

### 4.4. Чего не хватает

| Инструмент | Обязательность | Комментарий |
|---|---|---|
| git | **обязательно** | нет VCS; установка через winget |
| Azure app registration | **обязательно** для Microsoft Auth | client ID + redirect/device code |
| CurseForge API key | **обязательно** для CF | хранить в клиенте нельзя |
| NeoForge network path | желательно | Maven недоступен |
| sqlite3 CLI | опционально | удобно для отладки, приложению не нужно |
| Playwright browsers | опционально | можно использовать системный Edge/Chrome |
| WiX/NSIS cache | скачается автоматически | первый bundle требует сети |
| Кодовая подпись | обязательно к релизу | иначе SmartScreen |
| Ключи Tauri updater | обязательно к релизу | подпись обновлений |

---

## 5. Возможность сборки под Windows

### 5.1. Подтверждённые факты

1. Минимальный Rust-проект (`edition = "2024"`, `fn main`) собран и запущен:
   вывод `link-ok`. Это проверяет `rustc` + MSVC linker + Windows SDK.
2. `npx @tauri-apps/cli@2 info` дал зелёные отметки по WebView2, MSVC, rustc,
   cargo, rustup, toolchain, node, npm.
3. Референсное приложение `MC-launcher` (Tauri 2.12.5) имеет на этой машине
   собранные артефакты: `my-launcher.exe` 10 341 888 Б,
   `MyLauncher_0.1.0_x64_en-US.msi` 6 279 168 Б,
   `MyLauncher_0.1.0_x64-setup.exe` 3 760 928 Б.
4. Tauri 2.12.7 требует Rust ≥ 1.90; установлен 1.100.

### 5.2. Ограничения

- Только x64. Target ARM64 не установлен; решение о поддержке не принято.
- MSI/NSIS-упаковка требует загрузки WiX/NSIS при первом запуске; кэш
  `%LOCALAPPDATA%\tauri` сейчас отсутствует, сеть к GitHub есть.
- Подпись кода отсутствует; установщик будет помечен SmartScreen.
- **NEXUS-launcher ни разу не собирался.** Утверждать «сборка проверена»
  можно будет только после первого `npm run tauri build` реального проекта.

**Вывод:** технических препятствий для сборки нет; юридико-организационные
(подпись, ключи обновления) остаются.

---

## 6. Tauri 2

### 6.1. Версии

| Компонент | Референс | Актуально на 21.11.2026 |
|---|---|---|
| `tauri` (crate) | 2.12.5 | 2.12.7 |
| `tauri-build` | 2.7.4 | 2.7.6 |
| `wry` | 0.57.1 | 0.57.3 |
| `tao` | 0.37.2 | 0.37.3 |
| `@tauri-apps/api` | 2.12.4 | 2.12.5 |
| `@tauri-apps/cli` | 2.12.4 | 2.12.6 |

CLI, JS API и crate живут в одной minor-линии 2.12; расхождение patch-версий
допустимо. Нужен lockfile и правило обновления только вместе.

### 6.2. Рекомендуемая конфигурация

- `identifier`: `com.mws.nexus.launcher` (не менять после релиза — от него
  зависят data-каталоги и WebView2-профиль).
- Один main-window, кастомный titlebar в стиле NEXUS, `decorations: false`.
- Минимальные capabilities: `core:default`, управление окном,
  `opener:allow-open-url`; **никаких** `shell`, `fs`, `http` permissions.
- `assetProtocol` только для каталога иконок инстансов, не для всего диска.
- Строгий CSP: `default-src 'self'`, `script-src 'self'`, `object-src 'none'`,
  `frame-ancestors 'none'`; изображения — `self`, `data:`, `blob:` и конкретный
  CDN мод-каталога, если он нужен.
- Плагины: `single-instance` (обязательно первым), `opener`, позже `updater`.
- `single-instance` — Rust-only плагин; npm-пакета
  `@tauri-apps/plugin-single-instance` не существует, его нельзя добавлять в
  `package.json`.
- Автообновление — только после появления ключей и HTTPS-хостинга статического
  JSON; до этого функция должна отсутствовать, а не быть декоративной.

### 6.3. Риски Tauri

| Риск | Митигация |
|---|---|
| Избыточные permissions | capabilities только на main-window, ревью на каждом этапе |
| XSS в WebView → доступ к командам | CSP, отсутствие `dangerousRemoteDomainIpcAccess`, валидация всех аргументов |
| Утечка локальных путей через asset protocol | scope только на конкретный каталог |
| Обновление ломает совместимость | подпись, каналы, откат, миграции |
| WebView2 отсутствует на старых системах | целевая платформа Win11, WebView2 встроен |

---

## 7. Frontend: React, TypeScript, Vite

### 7.1. Факты

- React 19.3.1 — актуален.
- Vite 8.3.8 — актуален, Node 26 подходит.
- TypeScript **7.0.2** — последний, но **несовместим** с текущим
  `typescript-eslint 8.73.0`, у которого peer `typescript >=4.8.4 <6.1.0`.
- Референс использует TypeScript 6.0.3 и успешно собирается.
- Vitest 5.1.1 совместим с Vite 8.
- `@vitejs/plugin-react 6.3.0` требует Vite ^8.
- jsdom 31 требует Node ≥26 — установленный Node 26.13.1 подходит.
- React Router 8.5.2 требует Node ≥22.22 — подходит, но это мажорный переход
  с 7; API и поведение нужно проверять.
- `lucide-react 1.77.0` — актуален.
- Three.js 0.187 — опционален; добавит заметный вес бандла.

### 7.2. Конфликт, который нельзя игнорировать

```
typescript-eslint@8.73.0 peerDependencies:
  typescript: ">=4.8.4 <6.1.0"

npm latest typescript: 7.0.2
```

Если поставить TS 7, `npm install` может пройти с warning, но lint и
type-aware правила сломаются или будут работать непредсказуемо. Для этапа 1
нужно зафиксировать **TypeScript 6.0.3** и не обновлять до появления
typescript-eslint с поддержкой 7.

### 7.3. Рекомендуемая матрица frontend

| Пакет | Версия | Обоснование |
|---|---|---|
| react / react-dom | 19.3.1 | актуально |
| typescript | **6.0.3** | совместимость с typescript-eslint |
| vite | 8.3.8 | актуально, Node 26 |
| @vitejs/plugin-react | 6.3.0 | требует Vite 8 |
| vitest | 5.1.1 | Vite 8 |
| jsdom | 31.0.1 | Node 26 |
| @testing-library/react | 16.3.3 | React 19 |
| eslint | 10.15.0 | актуально |
| typescript-eslint | 8.73.0 | TS <6.1.0 |
| prettier | 3.10.1 | — |
| @tauri-apps/api | 2.12.5 | одна линия с crate |
| @tauri-apps/cli | 2.12.6 | devDependency |
| zustand | 5.1.0 | локальное состояние |
| @tanstack/react-query | 5.108.2 | кэш серверных данных |
| lucide-react | 1.77.0 | иконки |
| zod | 4.6.7 | валидация IPC/манифестов (опционально) |
| react-router | 8.5.2 | решение требуется; см. §17 |
| three | 0.187.0 | только если выбран WebGL-фон |

---

## 8. Зависимости: анализ и конфликты

### 8.1. Как проверялось

Созданы два временных Cargo-проекта и выполнен `cargo metadata` (разрешение
графа, без компиляции):

- **baseline** — версии, проверенные референсным `MC-launcher`;
- **latest** — актуальные версии на 21.11.2026.

Дополнительно проверено добавление `rusqlite` к `sqlx`.

### 8.2. Результаты разрешения

| Параметр | Baseline | Latest |
|---|---:|---:|
| Пакетов в графе | 606 | 600 |
| tauri | 2.12.7 | 2.12.7 |
| reqwest | 0.12.28 + 0.13.5 | **0.13.5 (одна копия)** |
| windows | 0.57.0 + 0.62.2 | **0.62.2 (одна копия)** |
| sqlx | 0.8.6 | 0.9.1 |
| libsqlite3-sys | 0.30.1 | 0.40.0 |
| zip | 2.6.1 | 9.8.1 |
| keyring | 3.6.3 | 4.2.0 |
| sysinfo | 0.33.1 | 0.40.4 |
| sha2 | 0.10.9 + 0.11.1 | 0.10.9 + 0.11.1 |
| sha1 | 0.10.7 | 0.11.0 |
| base64 | 0.21/0.22/0.23 | 0.21/0.22/0.23 |

Важные выводы:

1. **`tauri 2.12.7` сам зависит от `reqwest 0.13.5`.** Если выбрать
   `reqwest 0.12`, в бинарнике окажутся две HTTP-стек-копии. Поэтому для
   нового проекта предпочтителен **reqwest 0.13**, а не 0.12 из референса.
2. **`sysinfo 0.33` тянет `windows 0.57`.** `sysinfo 0.40` использует
   `windows 0.62.2`, то есть совпадает с Tauri и убирает дубль.
3. **`sha2 0.10` остаётся транзитивно через `wry`.** Прямое использование
   `sha2 0.11` не устраняет дубль полностью, но это допустимо: разные мажоры
   не конфликтуют.
4. **`base64 0.21/0.22` остаются транзитивно.** Прямой `base64 0.23`
   совпадает с `reqwest`, `hyper-util`, `tauri-codegen`; это лучший выбор.
5. **`thiserror 1.x` остаётся через glib/jni** (Linux/Android-зависимости
   Tauri). Прямой `thiserror 2` допустим.
6. **`sqlx` + `rusqlite` несовместимы.** Проверка дала жёсткую ошибку Cargo:

   ```
   package `libsqlite3-sys` links to the native library `sqlite3`,
   but it conflicts with a previous package which links to `sqlite3` as well
   ```

   Вывод: SQLite-слой должен быть один. Для проекта выбран **SQLx**, значит
   `rusqlite` не добавляется никогда (даже для тестов), иначе Cargo не
   разрешит граф.

### 8.3. MSRV предлагаемого набора

| Крейт | rust_version |
|---|---|
| tauri 2.12.7 | 1.90 |
| sqlx 0.9.1 | 1.94 |
| sysinfo 0.40.4 | 1.95 |
| zip 9.8.1 | 1.89 |
| keyring 4.2.0 | 1.88 |
| sha2 0.11.1 | 1.88 |

Максимум 1.95 < установленных 1.100. Ограничений нет.

### 8.4. Изменения API, которые нужно учесть

| Крейт | Изменение | Действие |
|---|---|---|
| reqwest 0.12 → 0.13 | feature `rustls-tls` заменён на `rustls`; появился `system-proxy` | проверить TLS/прокси в spike |
| sqlx 0.8 → 0.9 | feature `sqlite` → `sqlite-bundled`; API миграций/запросов | включить `sqlite-bundled`, не использовать `query!` без offline-кэша |
| zip 2 → 9 | смена API чтения/записи, тяжёлые default features | `default-features = false, features = ["deflate"]` |
| keyring 3 → 4 | рефакторинг на `keyring-core` + store-крейты | либо 3.6.3, либо проверить 4.2 в spike |
| sysinfo 0.33 → 0.40 | смена сигнатур `refresh_processes` | использовать latest, покрыть тестом |
| sha2 0.10 → 0.11 | digest 0.11 | согласовать с sha1 0.11 |
| quick-xml 0.38 → 0.42 | API | нужен только если парсить Maven XML вручную |

### 8.5. Рекомендуемый Rust-набор

**Базовый вариант (минимум дублей, актуальные версии):**

```toml
tauri = { version = "2.12", features = ["protocol-asset"] }
tauri-build = "2.7"
tauri-plugin-opener = "2"
tauri-plugin-single-instance = "2"

serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = [
  "rt-multi-thread", "macros", "process", "fs", "io-util", "sync", "time", "net"
] }
reqwest = { version = "0.13", default-features = false, features = [
  "json", "stream", "rustls", "gzip", "brotli", "system-proxy"
] }
sqlx = { version = "0.9", default-features = false, features = [
  "runtime-tokio", "sqlite-bundled", "migrate", "chrono", "uuid"
] }
zip = { version = "9", default-features = false, features = ["deflate"] }
keyring = "3.6"          # проверенный вариант; 4.2 — после spike
sysinfo = "0.40"
sha1 = "0.11"
sha2 = "0.11"
uuid = { version = "1", features = ["v4", "serde"] }
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
walkdir = "2"
tempfile = "3"
url = "2"
semver = "1"
base64 = "0.23"
futures = "0.3"
tokio-util = { version = "0.7", features = ["io"] }
windows = { version = "0.62", features = [
  "Win32_Foundation",
  "Win32_System_JobObjects",
  "Win32_System_Threading",
  "Win32_Security_Credentials"
] }
```

Почему не «всё latest»:

- `keyring 4` — свежий рефакторинг; для хранения токенов безопаснее начать с
  проверенной 3.6.3 или прямого DPAPI.
- `sqlx 0.9` и `zip 9` — правильный выбор для нового кода, но их API нужно
  подтвердить в отдельном spike до этапа 3.
- `reqwest 0.13`, `sysinfo 0.40`, `windows 0.62` — лучший вариант: меньше
  дублей и совпадение с Tauri.

### 8.6. Политика версий

1. Один `Cargo.lock` и один `package-lock.json`, оба в git.
2. Обновление зависимостей — отдельным PR, с `cargo deny`/`cargo audit` и
   прогоном тестов.
3. Запрещены `*` и «плавающие» git-зависимости.
4. Любая смена мажора — ADR с причиной и планом отката.
5. `Cargo.toml` и `package.json` не должны содержать разные версии Tauri
   API/CLI/crate: обновлять вместе.

---

## 9. Необходимые библиотеки по модулям

### 9.1. Rust

| Модуль | Крейты | Назначение |
|---|---|---|
| Core / errors | `thiserror`, `serde`, `uuid`, `time`/`chrono`, `tracing` | типы ошибок, ID, время, логи |
| DB | `sqlx` (sqlite-bundled), `migrate` | SQLite, миграции, репозитории |
| Net | `reqwest` (rustls, stream), `tokio`, `tokio-util`, `futures`, `bytes`, `url` | HTTP, стриминг, отмена |
| Download | `reqwest`, `sha1`, `sha2`, `tempfile`, `tokio-util` | Range, хэши, staging, атомарная замена |
| FS safety | `dunce`, `walkdir`, `same-file` (опц.) | канонизация Windows, обход, защита от reparse |
| Archives | `zip` (deflate), `tempfile` | `.mrpack`, CF zip, MultiMC/Prism, бэкапы |
| Java | `zip`, `serde_json`, `winreg`, `windows` | detect, Adoptium, реестр, архитектура |
| Minecraft | `serde`, `serde_json`, `sha1`, `reqwest` | version JSON, libraries, assets, natives |
| Loaders | `serde_json`, `quick-xml` (если нужно), `zip`, `tokio::process` | Fabric/Forge/NeoForge/Quilt |
| Launch | `tokio::process`, `windows` | аргументы, Job Objects, процессное дерево |
| Auth | `reqwest`, `keyring`/DPAPI, `zeroize`, `secrecy`, `sha1`, `sha2` | OAuth, токены, redaction |
| Modrinth | `reqwest`, `serde`, `url` | API, `.mrpack`, зависимости |
| CurseForge | `reqwest`, `serde`, `murmur2` | API, fingerprint, импорт |
| Modpacks | `zip`, `serde_json`, `toml`, `configparser`/свой INI | mrpack, CF, Prism, HMCL |
| NEXUS | `reqwest`, `serde`, `url`, `sha2` | каталог, манифест, файлы |
| Servers | `tokio::process`, `sysinfo`, `windows`, `serde_json`, `zip` | запуск, консоль, player-файлы, EULA |
| Backups | `zip`, `walkdir`, `tempfile`, `chrono` | архивы, retention, restore |
| Diagnostics | `tracing`, `tracing-subscriber`, `tracing-appender`, `zip` | логи, ротация, отчёт |

`rusqlite` **не добавляется**: конфликт `links = "sqlite3"` с SQLx доказан.

### 9.2. Frontend

| Слой | Пакеты |
|---|---|
| UI | `react`, `react-dom`, `lucide-react` |
| Состояние | `zustand` |
| Данные IPC | `@tanstack/react-query` (или собственный кэш) |
| Валидация | `zod` (опционально) |
| Стили | CSS variables + CSS-модули; без тяжёлого UI-кита |
| Фон | Canvas/CSS или `three` (lazy, опционально) |
| Тесты | `vitest`, `jsdom`, `@testing-library/react`, `@testing-library/user-event` |
| E2E | `@playwright/test` (системный Edge/Chrome) |

Запрещено: `axios` (достаточно Tauri invoke), `electron`, `node-fetch`,
CSS-in-JS с runtime-нагрузкой, любые shell/fs-плагины Tauri.

---

## 10. Целевая платформа Windows 11: возможности и ограничения

### 10.1. Что даёт платформа

| Возможность | Использование |
|---|---|
| NTFS + длинные пути | глубокие структуры модов/ассетов; всё равно проверять путь |
| Job Objects | гарантированное завершение дерева процессов Minecraft/сервера |
| `tokio::process` | stdin/stdout/stderr без shell |
| DPAPI | шифрование секретов на уровне пользователя |
| Windows Credential Manager | хранение refresh/access токенов |
| WebView2 evergreen | UI без встраивания Chromium |
| Реестр | обнаружение Java, версии системы |
| `sysinfo` | RAM/CPU процессов сервера |
| `GetVolumeInformation`/WinAPI | проверка места и файловой системы |
| MSI/NSIS | установка per-user/per-machine |

### 10.2. Ограничения и подводные камни

1. **MAX_PATH и канонизация.** Длинные пути включены, но Java, zip-крейты и
   сторонние инструменты могут не поддерживать `\\?\`. Нужна единая функция
   нормализации (`dunce::canonicalize` + при необходимости префикс `\\?\`).
2. **Junctions/reparse points.** `canonicalize` может уйти за пределы инстанса.
   Перед записью проверять, что итоговый путь остаётся внутри корня инстанса,
   и отклонять symlink/junction-компоненты.
3. **Зарезервированные имена и ADS.** `CON`, `PRN`, `AUX`, `NUL`, `COM1–9`,
   `LPT1–9`, `file.jar:stream`, хвостовые точки/пробелы — запрещать до записи.
4. **Регистронезависимость.** `Mods/A.jar` и `mods/a.jar` — коллизия на
   Windows. Проверять уникальность путей без учёта регистра и Unicode NFC.
5. **Антивирус.** Defender может кратковременно блокировать staging-файл.
   Нужны retry с backoff и атомарный rename; не отключать защиту.
6. **SmartScreen.** Неподписанный установщик и автообновление будут
   предупреждать. Нужен сертификат подписи (EV предпочтительно).
7. **UAC и установка.** Решить: per-user (без UAC, данные в `%LOCALAPPDATA%`)
   или per-machine (UAC, данные в `%PROGRAMDATA%`). Для лаунчера рекомендуется
   per-user.
8. **Firewall.** Первый запуск локального сервера вызовет запрос правила
   Windows Firewall; это нормальный UX, его нужно объяснять.
9. **Консоль.** GUI-приложение не имеет консоли; для сервера нужен
   перенаправленный stdin/stdout, а не `cmd.exe`.
10. **Кодовая страница.** Консоль 866 и ANSI 1251 — источник mojibake в
    скриптах и логах. Везде использовать UTF-8 и явные кодировки.
11. **Профили пользователей.** Данные в `%LOCALAPPDATA%` не синхронизируются
    между профилями; это правильно для гигабайтов ассетов, но требует
    пояснения.
12. **ARM64.** Не поддерживается текущим toolchain и не проверялся.

### 10.3. Рекомендуемые каталоги

```
%LOCALAPPDATA%\NEXUS-Launcher\
  config.toml
  launcher.db
  instances\
  assets\
  libraries\
  java\
  downloads\
  backups\
  logs\
```

Тяжёлые данные — только `LOCALAPPDATA`. `Roaming` не использовать для
много-гигабайтных инстансов и SQLite. Секреты — в Credential Manager/DPAPI,
не в `config.toml` и не в БД.

---

## 11. Внешние контракты: фактическая проверка

### 11.1. NEXUS Workspace, публичный API

Живой сервер: `http://127.0.0.1:8080`, OpenAPI 1.0.0. Публичные endpoints:

| Метод | Путь | Факт |
|---|---|---|
| GET | `/api/packs?channel=stable` | 200, `packs[]` |
| GET | `/api/packs/{id}/versions?channel=` | 200, `versions[]` |
| GET | `/api/packs/{id}/versions/{v}/manifest?channel=` | 200, манифест |
| GET | `/api/packs/{id}/icon` | 404, если иконки нет |
| GET | `/api/files/{hash}?packId=&version=&channel=` | 200/206, Range |
| GET | `/api/v1/health` | 200 |
| GET | `/api/v1/openapi.json` | 200 |

Реальные данные сборки `vanilla` версии `1.1`:

```text
minecraft.version = 26.2
minecraft.loader = fabric
minecraft.loaderVersion = 0.20.1
java.major = 25
files = 124
sum(size) ≈ 60.5 MB
settings = {}
```

Манифест **успешно прошёл** JSON Schema `launcher-manifest-1.json`
(Draft 2020-12, `jsonschema 4.26.0`).

### 11.2. Аномалии публичного API

| Наблюдение | Причина / следствие |
|---|---|
| 500 за ~10 с на `/api/files/...` (5 случаев) | Лог сервера: `pool timed out while waiting for an open connection`; пул БД `max_connections=10`, `acquire_timeout=10s` |
| 416 с задержкой 4,9 с | Ожидание соединения с БД, не ошибка Range |
| `/api/files/` → 200 `text/html` | SPA-fallback; проверять `Content-Type` и JSON |
| `/api/packs` без `channel` → 200 | Контракт требует `channel`; не полагаться на default |
| `channel=weird` → 422 | Валидация enum работает |
| OpenAPI не описывает 206/416 и query-параметры файла | Спецификация неполная; источник истины — `LAUNCHER_CONTRACT.md` + фактическое поведение |
| `If-Range` и suffix-range в итоге дали 206 | Не считать Range сломанным; 500 были следствием пула |

**Требования к лаунчеру:**

- retry с backoff для 408/429/500/502/503/504;
- не считать `200` доказательством корректности: проверять `Content-Type`,
  размер и SHA-256;
- при 500 на частичной загрузке — повторять с текущего offset, при
  невозможности — начинать файл заново;
- не использовать suffix-range (`bytes=-N`), только `bytes=N-`;
- учитывать, что сервер может отдавать `206` без описания в OpenAPI.

### 11.3. NEXUS, внутренний API

- `/api/v1/launcher/*` без токена → 401 `LAUNCHER_TOKEN_REQUIRED`.
- Токены: Bearer, scopes `launcher.catalog`, `launcher.artifacts`.
- Внутренний манифест содержит `category` и `side`, в отличие от публичного.
- Приватные сборки и серверные файлы доступны только через этот контур.
- Токен нельзя хранить в `config.toml`; только Credential Manager/DPAPI.

### 11.4. Ограничение, важное для scope

Публичный манифест NEXUS **уже отфильтрован** сервером: в нём только
`side = client|both`. Полей `side`/`category` в публичном манифесте нет.
Следовательно:

- установка клиентской сборки из NEXUS — возможна публично;
- установка **серверной** сборки из NEXUS через публичный API — невозможна;
- для серверных сборок нужен внутренний API + выданный токен, либо отдельный
  формат/эндпоинт.

Это нужно зафиксировать в продуктовом решении, иначе этап 7 ТЗ («серверная
сторона») окажется невыполнимым.

### 11.5. Mojang

Проверено 21.11.2026:

- version manifest: 925 версий (103 release, 761 snapshot, 26 old_beta,
  35 old_alpha);
- latest release `26.3`, latest snapshot `26.4-snapshot-10`;
- для `26.2`: `javaVersion.majorVersion = 25`, `mainClass =
  net.minecraft.client.main.Main`, asset index `32` (484 606 587 Б),
  131 библиотека, `arguments.jvm` и `arguments.game`;
- современные natives — это обычные библиотеки с `rules` и именами
  `…:natives-windows`, а не legacy `classifiers`. Legacy-версии требуют
  отдельной ветки резолвера.

Следствие: резолвер библиотек обязан поддерживать оба формата, а Java —
брать из метаданных, а не из таблицы «версия → Java».

### 11.6. Загрузчики

| Загрузчик | Факт 21.11.2026 |
|---|---|
| Fabric Meta | 200, версии для 26.2 есть |
| Quilt Meta | 200; для 26.2 первым идёт `0.20.0-beta.9` (beta, не stable) |
| Forge promotions | 200; для 26.2 — `65.1.5` recommended / `65.1.6` latest; для 26.3 — `66.1.9` |
| Forge Maven | 200 |
| NeoForge Maven | **TCP 443 недоступен**, 21 с timeout |
| NeoForge GitHub Releases | пустой список |

NeoForge-инсталлятор официально распространяется через Maven; GitHub его не
заменяет. Без прокси/зеркала модуль NeoForge нельзя ни реализовать, ни
протестировать на этой машине.

### 11.7. Java

- Adoptium API доступен; JRE 25 для Windows x64:
  `OpenJDK25U-jre_x64_windows_hotspot_25.0.5.1_2.zip`, 58 599 439 Б.
- Системно установлена только JRE 8; JRE 21 есть в данных MyLauncher.
- Для MC 26.2 требуется Java 25, значит автозагрузка runtime обязательна.
- Нужно проверять архитектуру (x64), хэш архива, безопасно распаковывать,
  хранить несколько версий и назначать runtime инстансу.

### 11.8. Modrinth

- API 200 без ключа.
- Требуется осмысленный `User-Agent` (по документации Modrinth), иначе
  возможны блокировки.
- Есть rate limit (по документации — порядка 300 запросов/мин на IP);
  нужны кэш, ETag и backoff.
- Формат модпака `.mrpack` (zip + `modrinth.index.json` + overrides).
- Зависимости описываются `project_id`/`version_id`; нужен resolver.

### 11.9. CurseForge

- `api.curseforge.com` без ключа → 403.
- `www.curseforge.com` → 403 (Cloudflare).
- `edge.forgecdn.net` → 200.
- Официальный API требует ключ и соблюдение Terms; часть авторов запрещает
  стороннее распространение.
- **Ключ нельзя вшивать в desktop-приложение**: он извлекается из бинарника.
  Варианты:
  1. пользователь вводит свой ключ (UX-плохо, но честно);
  2. собственный backend-proxy (появляется серверная зависимость);
  3. не поддерживать CurseForge в v1 (рекомендуется, пока нет юридического
     решения).
- Импорт CF-модпака без ключа невозможен: `manifest.json` содержит
  `projectID`/`fileID`, а не прямые URL.

### 11.10. Microsoft Authentication

- Endpoints `login.microsoftonline.com` и `xboxlive.com` доступны.
- Нужен зарегистрированный Azure-клиент (public client, device code flow,
  scopes `XboxLive.signin offline_access`), затем XBL → XSTS → Minecraft
  Services → профиль.
- В референсном клиенте `microsoft_client_id` пуст, то есть поток на этой
  машине **не проверялся**.
- Требуется юридическая проверка соответствия Microsoft/Mojang; нельзя
  использовать обходные методы.
- Токены — только в защищённом хранилище; в логах и ошибках — redaction.
- Offline-профиль обязателен как независимый провайдер, но не должен
  выдаваться за лицензионный.

---

## 12. Архитектурные риски и рекомендуемые решения

### 12.1. Модульность

ТЗ предлагает держать всё в `src-tauri/src/<module>`. Референсный клиент
показывает, чем это заканчивается: `modpack.rs` — 123 КБ, `minecraft.rs` —
118 КБ, `modrinth.rs` — 93 КБ, `server.rs` — 85 КБ. Это трудно тестировать и
рецензировать.

Рекомендация: **Cargo workspace** с тонким `src-tauri` и библиотечными
крейтами. Минимальный разумный набор:

```
crates/
  nexus-core        # ошибки, ID, время, пути, fs-safety, config, DTO
  nexus-platform    # SQLite, net, download, secrets
  nexus-game        # java, minecraft metadata/install, loaders, launch
  nexus-instances   # манифест инстанса, CRUD, clone, import/export
  nexus-content     # modrinth, curseforge, resolver, modpacks
  nexus-ops         # servers, backups, diagnostics
  nexus-nexus       # интеграция NEXUS Workspace
src-tauri/          # команды, события, state, capabilities
frontend/           # React
```

Правила:

- доменные крейты не зависят от `tauri`;
- `reqwest` не протекает в UI;
- каждый крейт имеет свой тип ошибки и тесты;
- `src-tauri` только преобразует DTO и управляет состоянием;
- циклические зависимости запрещены.

### 12.2. Процессы

- Запуск Minecraft/сервера — только `tokio::process::Command` с массивом
  аргументов; никакого `cmd /c`, `powershell`, склейки строк.
- Job Object с `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — чтобы при падении
  лаунчера не оставались зомби-процессы.
- Для сервера нужен **supervisor**: отдельный процесс/поток, который владеет
  stdin/stdout, пишет лог и переживает перезапуск UI. Иначе восстановить
  консоль после перезапуска лаунчера невозможно.
- PID недостаточен для восстановления: проверять start time, путь
  исполняемого файла и командную строку.

### 12.3. Файловая безопасность

Единый модуль `fs-safety`:

- нормализация и канонизация;
- запрет абсолютных путей, `..`, UNC, ADS, букв дисков;
- запрет зарезервированных имён, хвостовых точек/пробелов;
- проверка Unicode NFC и регистронезависимых коллизий;
- проверка, что итоговый путь внутри корня;
- отказ от symlink/junction/reparse-компонентов;
- лимиты zip: число записей, суммарный распакованный размер, коэффициент
  сжатия, глубина пути;
- staging + хэш + атомарный rename.

### 12.4. Секреты

- Токены — Credential Manager (предпочтительно) или DPAPI.
- В SQLite — только идентификатор записи в хранилище, не сам токен.
- Логи: фильтр по именам полей (`access_token`, `refresh_token`,
  `authorization`, `xsts`, `client_secret`), а также по значениям известных
  токенов.
- Ошибки HTTP не должны содержать тело запроса с токеном.
- `zeroize` для буферов с секретами.

### 12.5. Сеть

- TLS verification не отключать.
- Ограничение редиректов (0 или 1), запрет downgrade HTTPS→HTTP.
- SSRF-политика: для внешних каталогов блокировать loopback/link-local/
  private, кроме явно настроенных NEXUS-источников.
- Таймауты: каталог/манифест 30 с, файлы 600 с; ограничение размера ответа.
- Прокси: `system-proxy`/ручная настройка; не игнорировать корпоративный прокси.
- User-Agent с версией лаунчера и контактом (для Modrinth).

### 12.6. Обновления

- Tauri updater требует подписанные артефакты и HTTPS-эндпоинт.
- Ключи и хостинг пока отсутствуют → автообновление нельзя считать частью
  этапа 1.
- Обновление не должно удалять инстансы, Java, аккаунты, бэкапы.
- Нужен канал stable/beta и возможность отката.

---

## 13. Реестр рисков

Шкала: **K** — критический, **B** — высокий, **S** — средний, **N** — низкий.
Вероятность: В — высокая, С — средняя, Н — низкая.

| ID | Риск | Уровень | Вер. | Влияние | Митигация | Этап |
|---|---|---|---|---|---|---|
| R01 | Нет git/VCS/CI | K | В | Нельзя контролировать изменения и откаты | Установить git, init, .gitignore, CI | 0 |
| R02 | Нет кода и тестов | K | В | Нечего собирать и проверять | Этап 1, каркас | 1 |
| R03 | Нет Azure client ID | K | В | Microsoft Auth не реализуема | Регистрация приложения, device code | 5 |
| R04 | CurseForge key нельзя вшить | K | В | CF-функции и импорт CF-модпаков | Решение: user key / proxy / defer | 6 |
| R05 | NeoForge Maven недоступен | B | В | Нельзя установить/проверить NeoForge | Прокси/зеркало или дефер | 4 |
| R06 | NEXUS 500 при исчерпании пула БД | B | С | Обрывы загрузок, ложные ошибки | Retry/backoff, fallback на полный файл | 7 |
| R07 | Публичный NEXUS не отдаёт server-side файлы | B | В | Серверные сборки NEXUS не ставятся | Внутренний API + токен или смена scope | 7 |
| R08 | Quilt запрещён в stock-контракте NEXUS | S | В | Публикация Quilt-сборок блокируется | Локальный Quilt; NEXUS — явно ограничить | 4/7 |
| R09 | TypeScript 7 vs typescript-eslint | B | В | Lint/type-aware правила ломаются | Pin TS 6.0.3 | 1 |
| R10 | sqlx + rusqlite конфликт links | B | В | Cargo не разрешает граф | Один SQLite-слой (SQLx) | 1 |
| R11 | Мажорные апгрейды zip/sqlx/reqwest/keyring | S | С | Скрытые изменения API | Spike + lockfile + ADR | 1/3 |
| R12 | Java 25 для MC 26.x | B | В | Запуск невозможен без runtime | Adoptium downloader + verify | 4 |
| R13 | Forge/NeoForge installer — сторонний код | B | С | Supply chain, запись вне каталога | Хэши, headless, sandbox temp, лимиты | 4 |
| R14 | Восстановление консоли сервера | B | В | После перезапуска нет stdin | Supervisor-процесс или log-tail-only | 8 |
| R15 | Нет ключей updater/хостинга | B | В | Автообновление невозможно | Ключи, HTTPS, staged rollout | 10 |
| R16 | Нет подписи кода | S | В | SmartScreen, недоверие | Сертификат подписи | 10 |
| R17 | Мало места (≈60 ГБ) | S | С | Несколько модпаков + target | Preflight, очистка, лимиты | 2/9 |
| R18 | Zip Slip / zip bomb / junction | K | С | Запись вне инстанса, RCE-риски | fs-safety + security-тесты | 2/6 |
| R19 | Утечка токенов в логи | K | С | Компрометация аккаунта | Redaction, тесты, запрет тел запросов | 5 |
| R20 | Дрейф метаданных Mojang (26.x) | B | С | Ломается установка/запуск | Metadata-driven, версионированные парсеры | 3 |
| R21 | Rate limits Modrinth/Mojang | S | С | 429, блокировки | Кэш, ETag, backoff, UA | 6 |
| R22 | Антивирус блокирует staging | S | С | Случайные ошибки установки | Retry, atomic rename, диагностика | 2 |
| R23 | Кодировки 866/1251 | S | С | Mojibake в логах/скриптах | UTF-8 везде, явные encoding | 1 |
| R24 | Нет E2E-автоматизации Tauri | S | С | Регрессии UI | Playwright (web) + ручной smoke + tauri-driver | 9 |
| R25 | Только x64 | N | С | Нет ARM64 | Решить позже, target + CI | 10 |
| R26 | Сетевые ограничения/прокси | S | С | Часть источников недоступна | Proxy config, диагностика, зеркала | 2/7 |
| R27 | Неполный OpenAPI NEXUS | S | В | Неверная реализация клиента | Контракт + реальные ответы + фикстуры | 7 |
| R28 | Юридические ограничения (EULA/CF/Adoptium) | B | С | Блокировка распространения | Юридическая проверка до релиза | 0/10 |
| R29 | Ошибки в обработке аргументов запуска | B | С | Игра не стартует/падает | Golden-тесты args builder | 3 |
| R30 | Потеря данных при restore/update | K | Н | Уничтожение инстансов | Бэкап перед изменением, атомарность | 9/10 |

---

## 14. Пробелы в ТЗ, которые нужно закрыть до реализации

Ниже — не «придирки», а места, где ТЗ допускает взаимоисключающие трактовки.

1. **Монолитный `src-tauri/src`.** §2.2 предлагает одну папку с 20 модулями.
   Референс уже превратился в файлы по 100+ КБ. Нужно решение о workspace.
2. **Приватные сборки NEXUS.** §10.7 требует внутренний API, но публичный
   клиент не отправляет токен. Нужно явно разделить: публичный режим (без
   токена) и приватный (пользователь вводит токен).
3. **Серверные сборки NEXUS.** §10.6 требует `side=server`, но публичный
   манифест уже отфильтрован. Нужен внутренний API или отдельный эндпоинт.
4. **Quilt.** §5 требует Quilt, а `LAUNCHER_CONTRACT.md` фиксирует, что
   stock-клиент NEXUS отвергает Quilt. Локальный Quilt — да; публикация в
   NEXUS — нет.
5. **CurseForge.** §8.1 требует CF, но не решает вопрос ключа и Terms.
   Встраивание ключа в клиент небезопасно.
6. **Java.** §6.2 верно требует брать версию из метаданных, но fallback-таблица
   в контракте заканчивается на «25.x+». Реальная MC 26.2 требует Java 25;
   нужно полностью доверять `javaVersion.majorVersion` и не хардкодить
   соответствие.
7. **Обновления.** §20 требует подпись и откат, но нет ни ключей, ни хостинга,
   ни решения о канале.
8. **Тестовая стратегия.** §22 перечисляет группы, но нет: offline-политики,
   фикстур, mock-сервера, CI, coverage-гейтов, тестов миграций.
9. **Схема БД.** §16 перечисляет 14 таблиц, но нет: файлов инстанса, версий
   модов, задач, файлов игроков сервера, ссылок на секреты, истории миграций,
   индексов и политики удаления.
10. **Ошибки/IPC.** Нет единого формата ошибки, кодов, request id,
    идемпотентности и семантики отмены.
11. **Расположение данных.** Не решено: Roaming vs LocalAppData, portable mode.
12. **Режим установки.** Не решено: per-user vs per-machine.
13. **Подпись кода.** Не решено, кто и когда получает сертификат.
14. **ARM64.** Не решено, нужен ли.
15. **i18n/a11y/telemetry.** Не решено; для MVP лучше только русский,
    без телеметрии, с клавиатурной навигацией.
16. **Юридические тексты.** Нет упоминания Minecraft EULA, товарных знаков,
    «not affiliated with Mojang/Microsoft», лицензий третьих сторон.
17. **Прокси.** Нет требований к корпоративным прокси и системным настройкам.
18. **Preflight места.** Нет проверки свободного места перед установкой.
19. **Redaction логов.** Нет спецификации, что именно вычищается.
20. **Supervisor сервера.** Нет решения, как переживать перезапуск лаунчера.
21. **Миграции.** Нет политики совместимости `instance.json` и БД между
    версиями лаунчера.
22. **Консистентность бэкапов.** Нет процедуры `save-off/save-all` для
    работающего сервера.
23. **Кэш/лимиты.** Нет политики кэша метаданных и rate limit.
24. **Performance budget.** Нет целевых показателей запуска, RAM, FPS фона.
25. **Dependency policy.** Нет правила обновления и аудита уязвимостей.
26. **«Портирование».** Не определено, что значит «архитектура с
    возможностью портирования»: Linux? macOS? ARM64?
27. **Фон.** Three.js или Canvas — не решено; Three.js добавляет вес и
    WebGL-риски.
28. **Роутинг.** React Router или собственный view-state — не решено.
29. **Offline UUID.** Нет явного правила генерации и предупреждения о
    несовместимости с online-mode серверами.
30. **Лицензия проекта.** Не выбрана.

---

## 15. Минимальные требования к тестированию

### 15.1. Принципы

1. Unit-тесты **не ходят в интернет**. Сеть — только в opt-in интеграционных
   тестах с mock-сервером.
2. Все внешние JSON — фикстуры в репозитории, с указанием источника и даты.
3. Детерминизм: никаких `sleep`; время и случайность инжектируются.
4. Каждый security-кейс — отдельный тест, а не «проверено вручную».
5. Тест миграций: чистая БД + апгрейд с предыдущей версии.
6. Тест не должен зависеть от `%APPDATA%` разработчика; только temp-каталоги.
7. Live-тесты (Mojang, Modrinth, NEXUS) — под feature-флагом и не в CI по
   умолчанию.

### 15.2. Обязательные группы

| Группа | Минимум |
|---|---|
| Path/FS safety | `..`, абсолютные, UNC, ADS, reserved names, trailing dot/space, NFC, case-collision, symlink/junction |
| Archives | zip slip, zip bomb (ratio/size/entries), повреждённый zip, дубликаты, длинные пути |
| Hash/download | SHA-1/SHA-256, size mismatch, Range 206, 416, 500, retry, resume, cancel, pause |
| Manifests | NEXUS schema 1.0, `.mrpack`, CF `manifest.json`, Prism/MultiMC, instance.json |
| Java | парсинг `java -version` (8 и 21/25), архитектура, выбор runtime |
| Launch args | modern `arguments`, legacy `minecraftArguments`, rules, classpath, natives, offline/msa |
| Loaders | version selection, compatibility, install plan, идемпотентность |
| Dependency resolver | missing, conflict, optional, cycle, rollback |
| DB | fresh migrate, upgrade, WAL, busy_timeout, CRUD, удаление |
| Servers | EULA gate, start/stop, stdin, properties, players JSON, auto-restart backoff, PID reuse |
| Backup | excludes, restore, atomicity, retention |
| NEXUS | catalog/versions/manifest parse, 4xx/5xx, content-type, hash/size, client/server filter |
| Secrets | запись/чтение, отсутствие в логах, redaction, удаление |
| UI | stores, critical components, error states, progress, navigation |
| E2E | минимум один сценарий установки и запуска на Windows перед релизом |

### 15.3. Гейты по этапам

- Этап 1: `cargo test --workspace`, `npm run typecheck`, `npm run test`,
  `npm run build`, `cargo clippy -D warnings`, `cargo fmt --check`.
- Этап 3: установка vanilla на реальной машине + offline-тесты резолвера.
- Этап 4: запуск vanilla и Fabric; Forge/NeoForge — по доступности сети.
- Этап 5: Microsoft Auth на реальном аккаунте (ручной сценарий) + тесты
  redaction.
- Этап 7: установка реальной сборки NEXUS с проверкой SHA-256.
- Этап 8: сервер запускается, принимает команду, останавливается,
  восстанавливается после перезапуска лаунчера.
- Этап 10: MSI/NSIS, чистая установка/удаление, обновление, откат.

### 15.4. CI (после появления git)

Windows-runner:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit            # или cargo deny
npm ci
npm run typecheck
npm run lint
npm run test
npm run build
```

Tauri bundle — на теге, с артефактами MSI/NSIS и проверкой подписи.

---

## 16. Выводы

1. **Проект не начат.** Это не «почти готовый каркас», а чистое поле с ТЗ.
2. **Среда готова** к Tauri 2/Rust/Node/SQLite; блокеров сборки нет.
3. **Главные технические риски** — не Rust и не Tauri, а внешние контракты:
   Microsoft Auth, CurseForge, NeoForge, приватные/серверные сборки NEXUS и
   восстановление консоли сервера.
4. **Главный внутренний риск** — монолитная структура и отсутствие
   тестовой/CI-дисциплины. Его нужно закрыть на этапе 1, а не «потом».
5. **Зависимости** нужно зафиксировать сразу: TS 6.0.3, SQLx без rusqlite,
   reqwest 0.13, windows 0.62, zip 9 с урезанными features.
6. **Начинать можно** с этапа 0/1 (инфраструктура и оболочка) параллельно с
   организационными решениями C3–C8.

---

## 17. Открытые решения

| № | Решение | Варианты | Рекомендация | Когда |
|---|---|---|---|---|
| D01 | TypeScript | 6.0.3 / 7.0.2 | 6.0.3 | этап 1 |
| D02 | Rust-зависимости | proven / latest | latest с отдельным spike для sqlx/zip/keyring | этап 1 |
| D03 | Структура | один crate / workspace | workspace, 6–8 крейтов | этап 1 |
| D04 | Microsoft app | свой Azure app / чужой | свой public client, device code | этап 5 |
| D05 | CurseForge | user key / proxy / defer | defer до юридического решения | этап 6 |
| D06 | NeoForge | прокси / зеркало / defer | defer, если сеть не изменится | этап 4 |
| D07 | NEXUS private/server | публичный / внутренний API | публичный MVP; private — отдельно | этап 7 |
| D08 | Quilt | локально / NEXUS | локально; NEXUS не публикует | этап 4 |
| D09 | Консоль сервера | supervisor / log-only | supervisor | этап 8 |
| D10 | Установка | per-user / per-machine | per-user | этап 10 |
| D11 | Данные | LocalAppData / Roaming | LocalAppData для тяжёлого | этап 1 |
| D12 | Подпись/обновления | сейчас / позже | ключи до релиза, updater после MVP | этап 10 |
| D13 | Роутинг | React Router 8 / view-state | view-state на MVP | этап 1 |
| D14 | Фон | Three.js / Canvas | Canvas/CSS на MVP, Three.js — опция | этап 1 |
| D15 | Язык | ru / ru+en | ru на MVP, строки вынести в ресурсы | этап 1 |
| D16 | Телеметрия | есть / нет | нет | — |
| D17 | ARM64 | да / нет | нет в v1 | этап 10 |
| D18 | Лицензия | MIT / proprietary | решить до публикации | этап 0 |

---

## Приложение A. Команды и свидетельства

### A.1. Rust и линковка

```powershell
rustc --version            # rustc 1.100.0 (0ef1bc386 2026-11-09)
cargo --version            # cargo 1.100.0 (3acc79c0c 2026-11-05)
rustup show                # stable-x86_64-pc-windows-msvc
cargo install --list       # wasm-bindgen-cli v0.2.130
```

Временный проект:

```text
[package]
name = "rustcheck"
version = "0.1.0"
edition = "2024"
```

Результат `cargo run --quiet`: `link-ok`.

### A.2. Tauri info

```powershell
npx --yes @tauri-apps/cli@2 info
```

Ключевые строки:

```text
OS: Windows 10.0.26300 x86_64
WebView2: 157.0.4341.60
MSVC: Visual Studio Professional 2026
rustc: 1.100.0
cargo: 1.100.0
rustup: 1.29.3
Rust toolchain: stable-x86_64-pc-windows-msvc (default)
node: 26.13.1
npm: 11.20.0
```

### A.3. Разрешение зависимостей

```powershell
cargo metadata --format-version 1
```

Baseline (606 пакетов):

```text
tauri 2.12.7, reqwest 0.12.28 + 0.13.5, windows 0.57.0 + 0.62.2,
sqlx 0.8.6, zip 2.6.1, keyring 3.6.3, sysinfo 0.33.1
```

Latest (600 пакетов):

```text
tauri 2.12.7, reqwest 0.13.5, windows 0.62.2,
sqlx 0.9.1, zip 9.8.1, keyring 4.2.0, sysinfo 0.40.4
```

Конфликт SQLite:

```text
package `libsqlite3-sys` links to the native library `sqlite3`,
but it conflicts with a previous package which links to `sqlite3` as well
```

### A.4. NEXUS: реальные ответы

```text
GET /api/packs?channel=stable
200 {"packs":[{"id":"vanilla","name":"Vanilla","latestVersion":"1.1", ...}]}

GET /api/packs/vanilla/versions?channel=stable
200 {"versions":[{"version":"1.1","channel":"stable","updatedAt":1795274960}]}

GET /api/packs/vanilla/versions/1.1/manifest?channel=stable
200 manifestVersion=1, mc=26.2, fabric=0.20.1, java=25, files=124

GET /api/packs/vanilla/icon
404

GET /api/v1/launcher/catalog
401 {"error":{"code":"LAUNCHER_TOKEN_REQUIRED", ...}}
```

Range:

```text
Range: bytes=0-15        -> 206, Content-Range: bytes 0-15/135
Range: bytes=999999-     -> 416
Range: bytes=-5          -> 206 (в повторных прогонах)
```

Ошибки сервера при исчерпании пула:

```text
ERROR request: internal server error
  error=pool timed out while waiting for an open connection
  method=GET uri=/api/files/sha256:...
  latency=10005 ms
```

Конфигурация пула в сервере: `max_connections = 10`,
`acquire_timeout = 10s`.

### A.5. Mojang

```text
version_manifest_v2.json: 925 versions
latest: release 26.3, snapshot 26.4-snapshot-10
26.2: javaVersion.majorVersion=25, assets totalSize=484606587,
      libraries=131, mainClass=net.minecraft.client.main.Main
```

### A.6. Java

```text
api.adoptium.net/v3/info/available_releases -> 200
JRE 25 Windows x64:
  OpenJDK25U-jre_x64_windows_hotspot_25.0.5.1_2.zip (58 599 439 B)
```

### A.7. Файлы и хэши

```text
MAIN-DEV-PLAN.md                          8875CFC143CF0A243C87E07C051E016A...
NEXUS-server-docs/LAUNCHER_CONTRACT.md    7051B4632B476634F8B17C229A04E467...
NEXUS-server-docs/MANIFEST.md             3AC6656BBBF84179B03784FA53666F26...
NEXUS-server-docs/README.md               2665428480BC9690F431E69120FD070B...
```

`LAUNCHER_CONTRACT.md` и `MANIFEST.md` побайтово равны серверным;
`README.md` побайтово равен корневому README `MWS-NEXUS`.

---

## Приложение B. Референсный клиент: что можно и что нельзя переиспользовать

**Можно изучить и переиспользовать идеи:**

- device code flow и цепочка XBL/XSTS/Minecraft;
- хранение токенов в keyring;
- парсинг `java -version` (Java 8 vs 21/25);
- генерация offline UUID;
- обработка `.mrpack` и список разрешённых путей;
- ограничения путей и `safe_pack_path`;
- CSP и capabilities;
- структура `launcher.db` (как минимальный пример).

**Нельзя копировать:**

- монолитные файлы по 100+ КБ;
- смешение доменной логики, HTTP и Tauri-команд в одном модуле;
- отсутствие полноценных тестов на критичные сценарии;
- любые «демо»/фиктивные данные.

Референс — источник проверенных версий и подсказок, а не архитектурный шаблон.
