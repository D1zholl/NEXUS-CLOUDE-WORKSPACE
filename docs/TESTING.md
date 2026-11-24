# Тестирование NEXUS WORKSPACE

Документ фиксирует **фактические** команды и результаты прогонов, матрицу
покрытия требований и известные пробелы. Результаты ниже получены 20.11.2026
в рабочей копии `C:\Users\max\Desktop\MWS-NEXUS`; всё, что не запускалось,
помечено явно и не выдаётся за пройденное. Раздел I5 добавлен после прогонов
`tests/load.ps1` и `tests/resilience.ps1`, I6 — после локальных проверок
качества, I7 — это сама матрица ниже.

---

## 1. Окружение последнего прогона

| Параметр | Значение |
|---|---|
| Дата | 19–20.11.2026; L3–L5 live-проверки — 20.11.2026, 18:11–18:16 MSK |
| ОС | Windows 11 Pro 10.0.26300, NTFS; рабочая копия `C:\Users\max\Desktop\MWS-NEXUS` |
| CPU / RAM | Intel Core 5 210H, 12 логических ядер / 15,7 ГиБ |
| Rust | rustc/cargo 1.100.0 |
| Node.js / npm | v26.13.1 / 11.20.0 |
| PostgreSQL | 17.12 (по выводу `nexus check`; `show server_version` = 17.12) |
| Defender | real-time/Antivirus/AMService = **выключены** по снимку I5; исключения не создавались и не проверялись |
| База тестов | `nexus_test`, схема пересоздаётся интеграционными тестами |
| Сборка | debug для тестов, release backend для E2E и I5, production frontend |

Финальная приёмка L3–L5 выполнена по сохранённым отчётам и read-only проверкам
службы 20.11.2026. На live-проверке `NexusWorkspace` была `Running/Automatic`,
PostgreSQL 17.12 — `Running`, C: — NTFS, свободно 15,82 GB; все четыре
зарегистрированных тома службы были `online`. RAM стенда — 15,7 ГиБ. Defender
не включался для этой приёмки: все указанные Windows-релизные прогоны сделаны
при выключенной real-time защите. Подробности, ограничения и построчная матрица
§7/§11: [отчёт L3–L5](reports/final-acceptance-20261120-l3-l5.md).


---

## 2. Команды

### 2.1. Backend

```powershell
cd backend
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test -j 2
cargo test -j 2 --test openapi_tests
cargo run -- check --json
```

`-j 2` обязателен в этом окружении: при неограниченном параллелизме компиляция
падает с `os error 1455` (недостаточный файл подкачки Windows).

### 2.2. Frontend

```powershell
cd frontend
npm run typecheck
npm run test
npm run build
```

### 2.3. Дополнительно против живого сервера

```powershell
# Пароль — только через переменную окружения, не аргументом.
$env:NEXUS_ADMIN_PASSWORD = '***'

# I5: список, загрузка, скачивание, Range и private bytes.
powershell -File tests/load.ps1 -FileCount 16 -FileSizeMb 4 `
  -ListRequests 200 -DownloadRounds 2 `
  -ReportDir docs\reports -ReportName load-YYYYMMDD-i5

# I5: VHDX, обрыв клиента, Stop-Process -Force, Restart-Service.
powershell -File tests/resilience.ps1 -DiskFailureMode Vhdx -TestService `
  -ReportDir docs\reports -ReportName resilience-YYYYMMDD-i5

# G2: нагрузочный прогон большого объёма.
powershell -File tests/large-build.ps1 -SizeGb 1 -FileCount 500 `
  -VolumeRoot "C:\Temp\nexus-g2" -ReportDir docs\reports `
  -ReportName large-build-YYYYMMDD-smoke1gb -Cleanup
```

`load.ps1` и `resilience.ps1` требуют запущенного сервера, администратора и
зарегистрированного тома; временный том создаётся и удаляется самим скриптом.
`resilience.ps1` для VHDX и `Stop-Process` требует прав администратора.
`large-build.ps1` прогонялся 19.11.2026: 1 ГиБ — PASS, 25 ГиБ — см. § 3.5.

### 2.4. Качество кода и целевые браузеры (K3/K4)

```powershell
cd backend
cargo fmt --all -- --check
cargo clippy -j 2 --all-targets -- -D warnings
cargo test -j 2 --test migrations_tests

cd ..\frontend
npm run e2e:browsers -- e2e/system.spec.ts   # системные Chrome + Edge

cd ..
Invoke-ScriptAnalyzer -Path deployment -Recurse -Settings ./PSScriptAnalyzerSettings.psd1
Invoke-ScriptAnalyzer -Path tests -Recurse -Settings ./PSScriptAnalyzerSettings.psd1
```

`migrations_tests` создаёт одноразовую БД и не использует общую `nexus_test`.
`e2e:browsers` требует установленных Chrome и Edge (Windows); обычный
`npx playwright test` остаётся на bundled Chromium.

---

## 3. Фактические результаты

### 3.1. Сводка

| Проверка | Команда | Результат |
|---|---|---|
| Форматирование | `cargo fmt --all -- --check` | **exit 0** |
| Clippy | `cargo clippy -j 2 --all-targets -- -D warnings` | **exit 0**, предупреждений нет |
| Backend-тесты | `cargo test -j 2 --no-fail-fast` | **368 passed, 0 failed** (21.11.2026), включая `launcher_repository_tests` (8) и unit-тесты формата MyLauncher; PostgreSQL `nexus_test` |
| OpenAPI-тесты | `cargo test -j 2 --test openapi_tests` | **5 passed**, 117 путей / 142 операции |
| Миграции на пустой БД | `cargo test -j 2 --test migrations_tests` | **1 passed**: одноразовая БД, все 18 миграций, `_sqlx_migrations` сверен с файлами, `cloud_trash` — view |
| Проверка конфигурации | `nexus check --json` | **ok=true**, PostgreSQL 17.12, `migrations = 18`, тома 3, администраторы 1 |
| Live HTTP smoke | `nexus serve` на `127.0.0.1:18080` | `/api/v1/health` → `ok`, `/api/packs?channel=stable` → `200`, `{"packs":[]}` на реальной БД |
| TypeScript | `npm run typecheck` | **exit 0** |
| Frontend-тесты | `npm run test` | **92 passed, 0 failed**, 17 файлов; 23.11.2026 |
| Production-сборка | `npm run build` | успешно: CSS 98,68 кБ (gzip 18,04 кБ); 23.11.2026 |
| E2E с MyLauncher | ручной запуск клиента | **не выполнялся** в среде разработки: нет GUI-сессии и лицензионного запуска Minecraft. Проверены HTTP-контракт и Range интеграционными тестами; см. 3.3 |
| E2E (I1 + K4 + AI) | `npx playwright test` | **26 passed, 0 failed** (5,5 мин), release backend + production frontend + отдельная БД; новые AI Workspace/Management/MCP smoke |
| AI integration (backend) | `cargo test --test ai_tests` | **16 passed**: enrollment/revoke, encrypted provider key, Chat без tools, Agent tool cycle, approvals, Skills/version snapshots, module gate, ownership, MCP discovery/execution, usage, cancel confirmation, idempotency, recovery |
| AI Runtime | `cd runtime; cargo test --all-targets` | **28 passed, 1 ignored** (`real_polystrata_smoke`): WS tool execution/dedup, SSE, traversal, patch/quota, args/schema, MCP JSON-RPC, network helpers |
| AI full regression | `cargo test -j 1` | **411 passed, 0 failed, 1 ignored** (`logs/backend-full-tests-final.log`) |
| E2E Chromium/Chrome/Edge | `npm run e2e:browsers -- e2e/system.spec.ts` | после добавления AI Management не перезапускался; предыдущий прогон 10 passed (14 страниц), 20.11.2026 |
| PowerShell | `Invoke-ScriptAnalyzer -Path deployment,tests -Recurse -Settings ./PSScriptAnalyzerSettings.psd1` | **0 нарушений** (базовые правила K3, PSScriptAnalyzer 1.25) |
| I5 load | `tests/load.ps1 -FileCount 16 -FileSizeMb 4` | **PASS**: 200 списков (avg 1,8 мс, p95 2,5 мс), 16 файлов / 64 МиБ, 32 скачивания с SHA, 24/24 Range, private 13,8 → 17,2 МБ (+3,4); отчёт `docs/reports/load-20261120-i5.md` |
| I5 resilience | `tests/resilience.ps1 -DiskFailureMode Vhdx -TestService` | **PASS** с ограничением по службе: VHDX offline/recovery, обрыв клиента, C2 `running→pending→running→completed`; отчёт `docs/reports/resilience-20261120-i5.md` |
| G2, 1 ГиБ / 500 файлов | `tests/large-build.ps1 -SizeGb 1 -FileCount 500` | **PASS**: 1 073 741 824 байт, 500 уникальных SHA, 500 объектов на диске, 26,72 МБ/с, private +0,2 МБ; отчёт `docs/reports/large-build-20261119-smoke1gb.md` |
| G2, 25 ГиБ / 500 файлов | `tests/large-build.ps1 -SizeGb 25 -FileCount 500` | **PASS**: 26 843 545 600 байт, 500 уникальных SHA, 500 объектов, 38,37 МБ/с, private +2,0 МБ; отчёт `docs/reports/large-build-20261119.md` |
| L2 smoke против службы | `tests/smoke-new-features.ps1` | **PASS** (26 проверок): вход, CSRF, OpenAPI 108 путей, 17 настроек, `UNKNOWN_SETTING` 422, лимиты и квоты, гейт модулей без рестарта, метрики, Cloud/Minecraft; exit 0 |
| L2 WebSocket против службы | `tests/ws-smoke.ps1` | **PASS**: hello-кадр, `job.progress`, анонимное подключение отклонено; exit 0 |
| L2 load против службы 1.0.0 | `tests/load.ps1 -Username smokeadmin -CleanupVolume` | **PASS**: 200 списков (avg 1,5 мс, p95 2,2 мс), 16 × 4 МиБ, 32/32 SHA, 24/24 Range, private 11,1 → 14,6 МБ (+3,5 при лимите +20); отчёт `docs/reports/load-20261120-l2.md` |
| L2 resilience против службы | `tests/resilience.ps1 -ServiceName NexusWorkspace -DatabaseUrl <служба>` | **PASS**: VHDX offline/`NO_VOLUME_AVAILABLE`, восстановление SHA, обрыв 0/1048576 и продолжение, `Restart-Service` 0,79 с с живой сессией, C2 `running→pending→running→completed`, рестарт службы 12,92 с; отчёт `docs/reports/resilience-20261120-l2.md` |

### 3.2. Backend по файлам

| Файл | Тестов | Что покрывает |
|---|---:|---|
| `auth_tests.rs` | 8 | Argon2id, HMAC CSRF, хэширование токена сессии, флаги cookie `HttpOnly`/`SameSite`/`Secure`, объединение ролей и персональных прав, блокировка после неудачных входов, отсутствие утечки внутренних деталей, `403` с требуемым правом |
| `cloud_tests.rs` | 45 | чанковая загрузка и SHA-256, неверное смещение, превышение чанка, конфликт имён и регистронезависимый индекс, NTFS-имена/символы/ADS, лимит полного пути при 100 уровнях, замена, корзина/восстановление/очистка и TTL, дерево папок и traversal, перемещение в корень, хэш и лимиты ссылки, auth-only ссылки, Range, TTL и удаление экспорта, recent/category, batch 200, идемпотентность replace/share/export, retry при занятом файле, MIME по сигнатуре (поддельный `.png`, JPEG, HTML/SVG→скачивание, JS→текст, image/video/audio preview), детали и breadcrumb, rename/star/move и кольцо папок, поиск/сортировка/пагинация, статус и abort загрузки, список/отзыв ссылок, пароль и лимит скачиваний, экспорт через HTTP, purge и пустая корзина, изоляция чужих parentId, выравнивание staging после оборванного PATCH, параллельные `PATCH` (`UPLOAD_BUSY`/offset) и освобождение lock при истечении TTL |
| `idempotency_tests.rs` | 7 | повтор `complete` Cloud и Minecraft, конфликт другого тела, параллельный `begin` — один успех, область ключа по пользователю, очистка старых ключей |
| `jobs_tests.rs` | 19 | планировщик, лимит, пауза/возобновление/отмена, восстановление, сверка SHA, периодическая постановка очистки ключей, фильтры списка, детали и 404, конфликты pause/resume/cancel, валидация maintenance, права и аудит действий |
| `launcher_rate_limit_tests.rs` | 14 | `Retry-After`, изоляция лимитов по токену, восстановление окна, реальный каталог, учёт устойчивого превышения, `X-RateLimit-*` с уменьшением остатка, 121-й запрос → 429, лимитеры из `[launcher]`, заголовки артефакта, независимость корзин каталога и артефактов, 503 параллельного скачивания и восстановление слота, 401/403 не расходуют квоту, изоляция двух HTTP-токенов |
| `launcher_tests.rs` | 18 | хэш и scopes токена, неверный пароль, неизвестный scope, отзыв, каталог с публичными и своими приватными сборками, изоляция чужой приватной сборки на всех методах (404), владелец видит свою, публичная доступна всем, манифест, иконка в манифесте и SHA из `storage_objects`, GET иконки с типом по сигнатуре и 404 чужому токену, артефакт и SHA, скрытие артефакта с небезопасным путём, устаревший релиз, журнал скачивания, 8 параллельных чтений одного объекта, 9-е скачивание → 503 с `Retry-After` и восстановление слотов, истёкший токен, отсутствующий/неверный токен и `X-Launcher-Token`, права на admin-маршруты |
| `launcher_repository_tests.rs` | 9 | контракт MyLauncher без токена: `packs[]`, `versions[]` с `updatedAt`, `manifestVersion: 1`, `sha256:`/`/api/files/...`, побайтовая раздача, Range `206`/`Content-Range`, `416`, независимые каналы stable/beta, приватная сборка с Bearer и `401` после отзыва, неизменяемость снимка, исчезнувшие файлы, блокировка Unicode-пути, `429` на публичном каталоге, архивация текущего релиза не скрывает другой канал |
| `limits_tests.rs` | 4 | HTTP-границы чанка (413 с `limitBytes`/`receivedBytes` и успех 64 МБ), TTL сессии из настроек, порог блокировки входа, эффективный минимум пароля при смене |
| `load_tests.rs` | 5 | 120 мелких файлов и манифест, потоковая передача большого файла, параллельные загрузки, продолжение после обрыва, много вызовов API |
| `manifest_schema_tests.rs` | 7 | сгенерированный манифест по JSON Schema, примеры из документа, отказ некорректным манифестам, пустой релиз, отключённый файл, сохранение category/side/required, относительные уникальные `download_url` |
| `migrations_tests.rs` | 1 | K3: пустая одноразовая БД, все 18 миграций, сверка `_sqlx_migrations` с файлами, `cloud_trash` — view, `storage_volumes.scope`, режим replace в сессиях Minecraft |
| `minecraft_tests.rs` | 22 | валидация путей и slug/версий, уникальность, SHA и размер, дубли путей без учёта регистра, ошибки блокируют публикацию, неизменяемый снимок, стабильный манифест, байтовая сортировка с регистром и независимость от порядка загрузки, аддитивная аудируемая регенерация иконки, откат текущего релиза, удаление объектов, защита опубликованной сборки, клонирование; вложенный config и фильтр `folder`, коллизии create/rename без учёта регистра, запрет переноса config→mods, уникальный и атомарно заменяемый options.txt, отказ не-ASCII/versions/libraries/CON и подпапок mods, явные канонические правила |
| `minecraft_wizard_tests.rs` | 7 | сигнатура и размер иконки, допустимая иконка, замена файла, конфликт переименования, сохранение SHA при переименовании и перемещении папки, определение типа иконки |
| `modules_tests.rs` | 6 | определение модуля по пути (включая `/api/packs` и `/api/files`), отключённый модуль, снимок обоих модулей, аудит переключения, публичные ссылки при отключённом модуле, скрытие репозитория MyLauncher |
| `openapi_tests.rs` | 5 | покрытие всех маршрутов, OpenAPI 3.1, число путей = числу `.route(`, уникальные операции, объявленные path-параметры |
| `module_boundaries_tests.rs` | 2 | в `minecraft`/`launcher` нет `crate::cloud`; общие модули `storage/{http,objects,quota}.rs` существуют |
| `share_security_tests.rs` | 4 | тикет + `Range` не увеличивают счётчик, лимит не обходится без тикета, 5 ошибок пароля → `429` с `Retry-After`, смена `password_hash` и чужой token инвалидируют тикет |
| `session_cache_tests.rs` | 3 | hit/miss, перечитывание после TTL, logout/revoke и смена прав после инвалидации |
| `single_instance_tests.rs` | 1 | advisory lock: второй экземпляр не получает блокировку, после остановки первого — получает |
| `quota_tests.rs` | 5 | превышение квоты с деталями, применение изменения без перезапуска, значение по умолчанию, освобождение места, замена учитывает разницу |
| `realtime_tests.rs` | 25 | адресация событий владельцу, системные события, публикация без подписчиков, лимит соединений, активные соединения в health, реальные байты прогресса, прогресс задачи, upload/download и build-события, отказ без сессии и с недействительной сессией до Upgrade, ping/pong, независимость вкладок |
| `replacement_tests.rs` | 4 | ошибка замены до и после commit сохраняет старый файл/метаданные, API-ошибка не теряет байты, Windows sharing violation на старом объекте переживается retry |
| `resilience_tests.rs` | 7 | offline-том и восстановление, сессия после нового `AppState`, `Content-Length` и Range, полный цикл публичной ссылки, продолжение после обрыва, обнаружение подмены SHA, общий объект при удалении записи |
| `security_content_tests` | 5 | SVG/HTML/XML не отдаются как активные документы, JSON с script остаётся текстом, Range не обходит attachment, Content-Disposition очищает имена |
| `security_headers_tests.rs` | 2 | CSP и защитные заголовки на всех ответах, включая ошибки |
| `settings_tests.rs` | 48 | каталог настроек, типы и границы, кэш и мгновенное применение, CORS/сеть после рестарта, background defaults, квоты, TTL, лимиты, аудит и права |
| `share_security_tests.rs` | 4 | см. выше |
| `storage_tests.rs` | 15 | пути и traversal, уникальные ключи объектов, квоты и выбор тома, offline-том, SHA, Windows-пути, общий системный том |
| `windows_invariants_tests.rs` | 8 | reparse points, длинные пути, ADS/UNC/drive, зарезервированные имена, регистронезависимые конфликты, RFC 5987 для не-ASCII имён |

### 3.3. MyLauncher: что проверено фактически

Проверено интеграционными тестами на реальном PostgreSQL через полный
`nexus::build_router` (те же маршруты, middleware и хранилище, что в
production):

* `GET /api/packs?channel=stable|beta` — форма `{"packs":[...]}` без
  `success/data`, обязательные поля, `iconUrl=null`, каналы независимы.
* `GET /api/packs/{id}/versions` — `{"versions":[...]}`, `updatedAt` числом
  секунд, первый элемент — последний опубликованный.
* Манифест — `manifestVersion: 1`, `packId`, `version`, `minecraft`,
  `java.major`, `files[].hash` = `sha256:` + 64 lower hex,
  `files[].url` = `/api/files/sha256:...`, `required`.
* Скачивание — поток, `Content-Length`, `X-File-SHA256`, `Range` → `206` с
  `Content-Range: bytes 9-15/16`, недостижимый диапазон → `416` c
  `Content-Range: bytes */16`.
* Приватная сборка: анонимно отсутствует в каталоге; с Bearer-токеном
  видна; после отзыва — `401`.
* Публикация с кириллическим путём (`mods/файл.jar`) блокируется
  `VALIDATION_ERROR`: клиент такой путь отвергает.
* Опубликованный манифест не меняется после правки сборки; удалённый файл
  отсутствует в следующем релизе.
* 121-й запрос публичного каталога → `429`.

**Фактически не проверено:** установка и запуск Minecraft настоящим
MyLauncher на Windows (Vanilla/Fabric/Forge/NeoForge), докачка после обрыва
на реальном клиенте, отображение иконки в WebView. Для этого нужны GUI-сессия,
лицензия Minecraft и загрузка сотен мегабайт из Mojang; в текущей среде
разработки они недоступны. Это ограничение, а не подтверждённый результат.

### 3.4. Контракт манифеста

* Unit-тесты `minecraft::launcher_manifest` проверяют парсер клиента,
  Java-правило, пути/расширения, URL и неизменяемость хэша.
* `docs/schemas/launcher-manifest-1.json` — JSON Schema 2020-12; пример
  проходит её через `jsonschema::validator_for`.
* `docs/LAUNCHER_CONTRACT.md` — таблица вызовов и полный разбор полей.

### 3.5. Файловый интерфейс сборки и фокус модальных форм (факт)

Выполнено в текущей среде (Windows, PostgreSQL `nexus_test`):

* `npm run typecheck` — OK.
* `npm run lint` — OK, 0 warnings.
* `npm run test` — 17 файлов, **92/92** теста. В том числе:
  `buildPaths.test.ts` (12) — канонические пути, drop-маршрутизация,
  конфликты, дерево; `BuildFileManager.test.tsx` (3) — скелет, навигация,
  фокус в настройках файла, поиск; `ui.test.tsx` (2) — 12+ символов подряд
  удерживают фокус при re-render родителя, Tab-цикл, Esc и возврат фокуса.
* `npm run build` — OK.
* `cargo fmt --all -- --check` — OK.
* `cargo clippy --all-targets -- -D warnings` — OK.
* `cargo test -j 1` — все 374 теста backend зелёные (unit + 30 интеграционных
  наборов), включая `minecraft_tests` 20/20, `minecraft_wizard_tests` 7/7,
  `load_tests` 5/5, `migrations_tests` 1/1, `quota_tests` 5/5,
  `resilience_tests` 7/7, `launcher_repository_tests` 8/8.
* `nexus check --json` — `ok=true`, `problems=[]`, миграций 16.
* Полный `cargo test` без `-j 1` в этой среде прерывается `os error 1455`
  (недостаточный размер файла подкачки при параллельной компиляции тестов);
  с `-j 1` весь набор прошёл. Это ограничение среды, а не результат тестов.

Playwright-сценарии в `e2e/minecraft.spec.ts` фактически прогнаны на release
backend и PostgreSQL: **2/2**. Сценарий 1 — мастер, ввод 12 символов без потери
фокуса, загрузка `.jar`, проверка, релиз, публикация и оба манифеста
(MyLauncher/internal). Сценарий 2 — каталог `config` с подпапками через
`webkitdirectory`, drop `.jar` на `mods`, drop `options.txt` в слот, внутренний
DnD `mods → config` и длинный ввод в модальной форме релиза.



---

## AI Workspace tests

- `backend/tests/ai_tests.rs` — реальный PostgreSQL, HTTP-сервер и WebSocket
  Runtime: enrollment single-use/revoke, Chat streaming без tools, Agent tool
  cycle/executions, approval binding/consumption/expiry, module gate, ownership,
  MCP registry/discovery/execution, workspace usage reporting, cancel
  confirmation, `Idempotency-Key`, skill version snapshots/restore, restart
  recovery classification. **16 passed**.
- `backend/tests/ai_runtime_stand.rs` — автоматизированный стенд с отдельным
  процессом `nexus-ai-runtime.exe` и локальным OpenAI-compatible stub:
  enrollment CLI, WSS, heartbeat, Chat streaming, Agent `fs_write`, usage,
  cancel с подтверждением, restart/reconnect, revoke. **1 passed** (ignored по
  умолчанию, запуск `scripts/ai-runtime-stand.ps1`). Это не второй компьютер.
- `runtime` unit/integration tests — traversal/absolute/symlink, atomic write,
  search, unified diff и quota pre-check, JSON Schema, config, args, Job
  Object/network helpers, SSE/tool calls, MCP JSON-RPC, sequence/messageId
  dedup: **27 passed, 1 ignored**.
- `real_polystrata_smoke` — `#[ignore]`; требует реальную PolyStrata. Запуск:
  `scripts/ai-polystrata-smoke.ps1`.
- Frontend: `npm run typecheck`, `npm run test` (**92 passed**), `npm run build`,
  `npm run lint`.
- Полный регрессионный прогон Phase 9: `cargo test -j 1` (см.
  `logs/backend-full-tests-phase9.log`), Playwright E2E, существующие
  smoke/ws/resilience/load сценарии. `-j 2` не используется: интеграционные
  тесты делят `nexus_test` и очищают схему.

Двухмашинный сценарий: `scripts/ai-two-machine-check.ps1`. Без его реального
прохождения проект не объявляется Production Ready (см.
`docs/AI_PRODUCTION_AUDIT.md`).
