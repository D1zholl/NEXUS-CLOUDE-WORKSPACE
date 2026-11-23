# NEXUS AI Workspace — статус реализации

Обновлено после фактической реализации и прогонов тестов. Документ фиксирует,
что реально работает, что проверено и что осталось ограничением.

## 1. Исходное состояние

- Каталог `C:\Users\max\Desktop\MWS-NEXUS` **не является git-репозиторием**
  (`.git` отсутствует). Существующий код не перезаписывался без необходимости;
  изменения аддитивные, кроме замены декоративной заглушки `/workspace/ai`.
- Проверено: Rust 1.100, Node 26.13, PostgreSQL 17/19 на `127.0.0.1:5432`.
- PolyStrata на машине разработки не установлена. Реальный чат проверяется на
  втором ПК через `nexus-ai-runtime check` и `real_polystrata_smoke` (ignored
  test); автоматические интеграционные тесты используют реальный HTTP/WS
  протокол NEXUS и тестовый Runtime-стенд.

## 2. Реализовано

### Backend (`backend/src/ai/`)

- `mod.rs` — `AiState`, module key, recovery (`streaming` → failed, активные
  задачи → failed без повторов side effects, Runtime offline).
- `crypto.rs` — AES-256-GCM для API key, ключ из `NEXUS_SECRET`.
- `providers.rs` — CRUD, test connection, models, health/status через Runtime.
- `conversations.rs` — диалоги, сообщения, поиск/пагинация, вложения (текст),
  streaming Chat без tools, stop/regenerate/edit, title.
- `runtime/` — enrollment, одноразовые коды, device token (SHA-256), WSS,
  heartbeat, capabilities, revoke/disable/rotate, outbox/receipts, reconnect,
  дедупликация, события.
- `workspaces.rs` — доступ, квоты, валидация root path, usage summary.
- `tools.rs` — серверный каталог tools, JSON Schema, risk/approval policy,
  allowlist workspace, args hash, `skill.load`.
- `approvals.rs` — pending, TTL, approve/reject, атомарное потребление,
  привязка к args hash.
- `skills.rs` — ZIP import (zip-slip/traversal/symlink/размеры), версии,
  файлы в PostgreSQL, назначения user/workspace/role, load-on-demand.
- `orchestrator.rs` — Agent Task lifecycle, последовательные model/tool циклы,
  execution history, waiting_approval/resume, cancel, timeout, лимиты,
  восстановление в контролируемую ошибку.
- `admin.rs` / `admin_routes.rs` — overview, audit/runtime logs, Runtime nodes,
  enrollment, providers, workspaces, Skills/assignments, tools/programs, quotas.
- `api/openapi/ai.rs` + `document.rs` — 173 пути / 208 операций, AI tags,
  WebSocket-события, multipart.
- Миграция `0019_ai.sql`: 21 таблица, индексы/FK, 10 прав `ai.*`, модуль `ai`.
- Module gate: `/api/v1/ai/*` блокируется при отключённом модуле без рестарта;
  enrollment не требует CSRF (одноразовый код в теле), Runtime WS — Bearer token.
- Recovery вызывается в `main.rs::serve`; политики tools синхронизируются в
  `database::sync_permission_catalog`.

### Runtime (`runtime/`)

- Отдельный Rust executable/Windows Service; исходящий WSS, reconnect
  exponential backoff + jitter, heartbeat, capabilities.
- Enrollment CLI и `set-token`; device credentials в `state/device.json`.
- OpenAI-compatible PolyStrata: `/models`, `/health`, `/status`,
  `/chat/completions`, SSE, tool_calls, usage, cancel, ошибки.
- Tools: `fs.list/read/write/search/mkdir/move/copy/delete/apply_patch`,
  `program.run`; JSON Schema, deny-by-default, workspace allow-roots.
- Path security: абсолютные пути/UNC/`..`, canonical containment, symlink и
  Windows reparse points, повторная проверка перед операцией.
- Atomic write, unified diff (проверка всех hunks до записи), limits.
- Processes: только зарегистрированные программы, прямой spawn без shell,
  проверка args (control chars, absolute/UNC/`..`), timeout, CAP output,
  завершение дерева (`taskkill /T /F` + kill), env allowlist.
- Quotas: max file bytes, bytes/files квоты workspace проверяются под
  per-workspace mutex.
- Idempotency `messageId`: in-memory + `receipts.jsonl`;
  `task.cancel`, provider cancel, provider/tool cache.
- Service scripts: install/update/uninstall, ACL state/log, example конфигурации,
  `runtime check` диагностика, README.

### Frontend

- `/workspace/ai`: полноценный Chat/Agent UI — список/поиск/переименование/
  архивирование диалогов, модели/provider/workspace, streaming с realtime,
  Markdown/code highlight/copy, вложения, regenerate/edit/stop, task inspector,
  executions/events, approvals, cancel, error/empty/loading/offline states,
  responsive layout.
- `/system/ai`: AI Management — Overview, Providers, Runtime, Workspaces, Skills,
  Tools & Programs, Quotas, Security & Logs. Все действия подключены к API.
- Навигация и module gate `ai`; System Overview знает AI-модуль.
- Realtime `ai.*` события добавлены в типизированный клиент.

### Документация

- `docs/AI_WORKSPACE.md` — архитектура, API, протокол, RBAC, Skills, security,
  deployment, ограничения.
- `runtime/README.md` — установка/регистрация/ротация/диагностика/ограничения.
- Обновлены `README.md`, `docs/README.md`, `docs/ARCHITECTURE.md`,
  `docs/API.md`, `docs/SECURITY.md`, `docs/DATABASE.md`, `docs/DEPLOYMENT.md`,
  `docs/ADMIN_GUIDE.md`, `docs/USER_GUIDE.md`, `docs/TESTING.md`, `.env.example`,
  `docs/openapi.json` и сгенерированный список endpoints.

## 3. API и протокол

- Пользовательские: `/api/v1/ai/providers`, `/conversations…`, `/tasks…`,
  `/approvals…`, `/workspaces…`, `/skills…`, `/tools`, `/runtime/enroll`,
  `/runtime/connect`.
- Административные: `/api/v1/ai/admin/overview`, `/logs`, `/runtime-nodes…`,
  `/enrollment-codes…`, `/providers…`, `/workspaces…`, `/skills…`,
  `/tools…`, `/programs…`, `/quotas…`.
- Протокол: versioned envelope (`protocolVersion`, `messageId`, `type`,
  `timestamp`, `nodeId`, `correlationId`, `taskId`, `sequence`, `payload`),
  типы hello/heartbeat/capabilities/provider/tool/task/error.
- Outbox `ai_runtime_commands`; API key не пишется в outbox; при resend
  подставляется из зашифрованной конфигурации.

## 4. Тесты и фактические результаты

Запускались на локальном PostgreSQL (`nexus`, `nexus_test`).

| Проверка | Результат |
|---|---|
| `cargo check --all-targets` (backend) | успешно, без warnings |
| `cargo test --lib` (backend) | **80 passed** |
| `backend/tests/ai_tests.rs` | **10 passed** (single-use enrollment/revoke, Chat без tools, Agent tool cycle, approval binding, reject, expiry, Skills import/assign/load, encrypted API key, module gate, ownership) |
| `backend/tests/migrations_tests.rs` | passed (19 миграций, AI-таблицы/права) |
| `backend/tests/openapi_tests.rs` | passed (173 пути, 208 операций) |
| `backend/tests/modules_tests.rs` | passed |
| полный `cargo test -j 1` (backend) | **402 passed, 0 failed** (все unit + integration suites, включая AI, Cloud, Minecraft, Launcher, jobs, realtime, Windows invariants) |
| `cargo test --all-targets` (runtime) | **20 passed, 1 ignored**, включая реальный WS tool execution + dedup и SSE parser |
| `real_polystrata_smoke` | `#[ignore]`, требует реальную PolyStrata; на дев-машине не запускался |
| `npm run typecheck` | успешно |
| `npm run test` | **92 passed** (17 файлов) |
| `npm run lint` | успешно (0 warnings) |
| `npm run build` | успешно (предупреждение о размере chunk — существующее) |
| `npm run e2e` (Playwright) | **25 passed** (после исправления stale-селекторов Cloud/Launcher и z-index модального окна; AI Management включён в проверку 15 страниц System) |

Security-проверки в тестах: device token не хранится открытым; API key
шифруется и не попадает в ответ/аудит; чужой диалог → 404; отключённый модуль →
403 `MODULE_DISABLED`; approval нельзя использовать повторно; Chat request
содержит 0 tools; Agent request не содержит неразрешённый `fs.delete`;
path traversal/absolute/UNC/symlink блокируются в Runtime.

## 5. Ограничения (честно)

1. **MCP не реализован** — нет реестра соединений и discovery. Локальные tools
   защищены.
2. **Сетевая изоляция программ** ограничена allowlist аргументов и запуском без
   shell; автоматической egress-фильтрации Windows Firewall нет.
3. **Workspace usage bytes/files** не репортится Runtime в БД; UI показывает
   `checkedAt=null`/«нет данных», без фиктивных нулей.
4. **Cancel confirmation**: backend отправляет `task.cancel` и может
   финализировать `cancelled` локально до сетевого ack. Для строгой гарантии
   остановки проверяйте Runtime log/процессы.
5. **Backend restart** не продолжает `running` задачи (контролируемая ошибка),
   чтобы не дублировать side effects; `waiting_approval` сохраняется.
6. **Provider API key** нечитаем после смены `NEXUS_SECRET` — ключ нужно ввести
   заново.
7. **Runtime not installed on second PC in this environment** — проверка
   реального WSS между двумя компьютерами и реальной PolyStrata остаётся
   задачей развёртывания (инструкции и скрипты готовы).
8. **Quota concurrency** между несколькими Runtime-инстансами одного workspace
   не поддерживается; не создавайте такую конфигурацию.
9. **Квоты для `fs.apply_patch` и `program.run`** не пересчитываются по размеру
   заранее (патч проверяется по размеру файлов, программа — по policy/timeout);
   для строгой квоты используйте отдельный диск/лимиты ОС.
10. **История версий Skill** хранит `SKILL.md`/checksum/метаданные, но не полный
    snapshot всех файлов предыдущей версии; текущие файлы заменяются при импорте.
11. **Заголовок диалога** формируется детерминированно из первого сообщения, без
    дополнительного вызова модели (безопасный fallback).

## 6. Статус полного регрессионного прогона

`cargo test -j 1` завершён: **402 passed, 0 failed, 0 ignored**.
Полный лог — `logs/ai-backend-tests-final2.log`.

Почему `-j 1`: интеграционные тесты используют общую БД `nexus_test` и
очищают схему `public`; при параллельном запуске нескольких test-бинарников
(`-j 2`) они начали конкурировать за schema (deadlock/`_sqlx_migrations не
существует`). Это свойство существующего test harness, не регрессия AI.
Последовательный прогон проверяет все 402 теста без пропусков.

Дополнительно после полного прогона выполнены targeted-проверки:
`cargo test --test openapi_tests` (5 passed), `cargo test --test ai_tests`
(10 passed), `cargo check --all-targets` без warnings, runtime
`cargo test --all-targets` (20 passed, 1 ignored), frontend
typecheck/lint/test/build (92 tests).

## 7. Production-запуск (кратко)

1. `backend`: `cargo run -- migrate`, `cargo run -- serve`; задать `NEXUS_SECRET`,
   HTTPS reverse proxy, `NEXUS_COOKIE_SECURE=true`.
2. `frontend`: `npm ci && npm run build`.
3. В **System Management → AI Management**:
   - создать Provider (Base URL PolyStrata на Runtime, модель);
   - создать enrollment code;
   - создать workspace пользователю с absolute root внутри `allowed_roots`;
   - при необходимости импортировать Skills, настроить quotas/tools/programs.
4. На втором ПК: `runtime` build, `runtime.toml`, `enroll`, `install-service.ps1`,
   `runtime check --json`.
5. Проверить Chat, Agent с `fs.read`/`fs.write`, approval для `fs.delete`,
   cancel, reconnect (`Restart-Service`), revoke/rotate.
