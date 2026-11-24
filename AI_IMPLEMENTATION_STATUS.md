# NEXUS AI Workspace — статус реализации

Обновлено 23 ноября 2026 после Phase 9 (Production Audit & Finalization).
Документ фиксирует фактическое состояние, проверки и ограничения. Полный аудит:
[`docs/AI_PRODUCTION_AUDIT.md`](docs/AI_PRODUCTION_AUDIT.md).

## 1. Git и резервная копия

- До изменений создана резервная копия:
  `<Desktop>\MWS-NEXUS-backup-20261123` (без `node_modules`,
  `target`, `dist`, логов; `.env` включён в локальную копию).
- Установлен Git 2.56.1, инициализирован репозиторий `main`, baseline-коммит.
- `.gitignore` исключает `.env*`, секреты, `runtime/state`, БД/дампы, `logs`,
  `node_modules`, `target`, `dist`, Playwright auth state, пользовательские
  данные. Существующие данные не удалялись.

## 2. Реализовано и проверено

### Backend

- Модуль `ai`, module gate, RBAC (10 прав `ai.*`), audit, realtime, OpenAPI.
- Providers: CRUD, encrypted API key (AES-256-GCM), test/models/status через
  Runtime, admin list включает отключённые.
- Conversations/messages/attachments, поиск/пагинация, streaming Chat без tools,
  regenerate/edit/stop, заголовок по первому сообщению.
- Agent Orchestrator: последовательные model/tool циклы, JSON Schema, workspace
  allowlist, MCP authorization, risk/approval, execution history, limits,
  recovery, cancel.
- Approvals: TTL, args hash, атомарное потребление, approve/reject/expiry.
- Skills: ZIP import, защита от zip-slip/symlink/oversize, полный snapshot всех
  файлов каждой версии, history/get/restore, assignments, load-on-demand.
- Workspaces: изоляция root, квоты bytes/files/max file, usage reporting,
  refresh endpoint, network policy, memory limit.
- MCP: реестр соединений, discovery через Runtime, snapshots tools, assignments
  user/workspace/role, tool policies, SSRF-проверка, encrypted secret.
- Idempotency: `Idempotency-Key` для send/cancel/approve/reject; task
  idempotency; unique execution keys.
- Recovery: queued → requeue, safe running → resume, in-flight → receipt
  reconciliation, watchdog, waiting_approval сохраняется.
- Cancel: `cancel_status`, `task.cancel_ack`, `task.cancelled{confirmed}`,
  watchdog `AI_CANCEL_UNCONFIRMED`; не выдаёт отмену за успешную до ack.
- Миграции: 20 (`0019` + `0020_ai_hardening.sql`), 26 AI-таблиц.
- OpenAPI: 185 путей / 224 операции.

### Runtime

- Отдельный Rust executable/Windows Service, исходящий WSS, reconnect/backoff,
  heartbeat, capabilities, enrollment CLI, token rotate/revoke.
- PolyStrata OpenAI-compatible: models/health/status/chat, SSE, tool calls,
  usage, cancel, HTTP/JSON errors; SSE timeout исправлен.
- Tools: fs.list/read/write/search/mkdir/move/copy/delete/apply_patch,
  program.run; JSON Schema, deny-by-default, workspace roots.
- Path security: absolute/UNC/`..`, canonical containment, symlink/junction/
  reparse, повторная проверка.
- Atomic write, unified diff с предварительной проверкой всех hunks.
- Processes: registered programs only, no shell, args allowlist, timeout,
  output cap, Job Object (kill-on-close), memory limit, taskkill /T.
- Network isolation: Windows Firewall outbound block для `network_policy=deny`,
  fail-closed без admin, автоматическое удаление правила.
- Quotas: write/copy под mutex, apply_patch delta до записи, program.run до и во
  время выполнения (мониторинг usage + kill tree).
- Usage: `workspace.usage`, usage в `tool.completed`, history.
- MCP client: Streamable HTTP JSON-RPC, initialize/tools.list/tools.call,
  SSE/JSON, session id, schema validation, SSRF.
- Cancel confirmation: ack + ожидание завершения активных операций (до 30 с),
  `confirmed` только при 0 активных.
- Idempotency/replay: sequence/timestamp/messageId, receipts.jsonl, caches.
- Diagnostics: `runtime check` проверяет roots, token, NEXUS `/health`,
  PolyStrata, programs, network isolation status.

### Frontend

- `/workspace/ai`: Chat/Agent UI, streaming, Markdown/code, attachments,
  regenerate/edit/stop, task inspector, approvals (permission-gated), cancel,
  executions/events, reconnect reload, last-message pagination.
- `/system/ai`: Overview, Providers, Runtime, Workspaces (+usage refresh),
  Skills, Tools & Programs, MCP (CRUD/discovery/tools/assign), Quotas,
  Security & Logs.
- Home card показывает реальный статус модуля и `ai.access`.

## 3. Проверки

| Проверка | Результат |
|---|---|
| backend `cargo check --all-targets` | успешно, без warnings |
| backend `cargo test -j 1` | 411 passed, 0 failed (полный прогон Phase 9; лог `logs/backend-full-tests-final.log`) |
| `backend/tests/ai_tests.rs` | 16 passed |
| `ai_runtime_stand` (реальный Runtime process) | 1 passed |
| runtime `cargo test --all-targets` | 28 passed, 1 ignored (`real_polystrata_smoke`) |
| `openapi_tests` | 5 passed (185 путей / 224 операции) |
| `migrations_tests` | 1 passed (20 миграций) |
| frontend `npm run typecheck` | успешно |
| frontend `npm run lint` | успешно (0 warnings) |
| frontend `npm run test` | 92 passed (17 файлов) |
| frontend `npm run build` | успешно |
| Playwright E2E | **26 passed** |

## 4. Реализовано, но требует проверки на реальном втором компьютере

1. Полный сценарий: NEXUS на PC-1, Runtime + PolyStrata на PC-2.
2. Реальный WSS через production reverse proxy/сертификат.
3. Реальная PolyStrata и выбранная модель: tool calling, streaming, лимиты.
4. Windows Service под непривилегированной учётной записью + ACL.
5. Network isolation на production Runtime (нужны права администратора).
6. Реальные MCP-серверы (корпоративные/локальные) и их tool schemas.
7. Поведение при сетевых разрывах WAN, DNS, proxy, antivirus.

## 5. Частично реализовано

- `max_parallel_tasks` и user storage quotas (`max_storage_bytes`, `max_files`,
  `max_file_bytes`) хранятся/отображаются, но admission control их не enforce;
  enforce работают `max_concurrent_tasks`, `max_tasks_per_day`,
  `max_tool_calls_per_task`, `max_task_seconds` и workspace-квоты Runtime.
- Network isolation: firewall-правило на executable, не process-level;
  AppContainer не реализован.
- MCP: только Streamable HTTP; stdio отсутствует.
- Quota concurrency между несколькими Runtime одного workspace не поддержана.
- Frontend: нет browser файлов workspace и выбора старых задач.

## 6. Не реализовано

- Реальная проверка на втором физическом компьютере в этой среде.
- Реальный PolyStrata smoke на машине аудита (сервис отсутствует).
- MCP stdio transport.
- Process-level egress isolation (AppContainer/WFP per-process).
- Полноценный workspace file browser в UI.

## 7. Production-запуск (кратко)

1. Backend: `cargo run -- migrate`, `cargo run -- serve`; задать `NEXUS_SECRET`,
   HTTPS reverse proxy, `NEXUS_COOKIE_SECURE=true`.
2. Frontend: `npm ci && npm run build`.
3. System Management → AI Management: Provider, enrollment code, workspace,
   Skills, programs, MCP.
4. PC-2: build runtime, `runtime.toml`, `enroll`, `install-service.ps1`,
   `runtime check --json`.
5. Проверить §5 `docs/AI_PRODUCTION_AUDIT.md`.
