# NEXUS AI Workspace — статус реализации

Обновлено 25 ноября 2026 (вторая итерация): добавлены **полный жизненный
цикл AI Workspace** (мягкое удаление с retention, восстановление, фоновая
физическая очистка через подтверждение Runtime, dry-run, retry/blocked) и
**транзакционные резервы квот** в PostgreSQL. Ранее в этот же день добавлен
**AI Agent Workspace (Agent IDE)** — `/workspace/ai/agent`, файловый API через
Runtime, снимки изменений и безопасный откат, Task Manager, нижняя панель IDE.
Документ фиксирует фактическое состояние, проверки и ограничения.
Аудит: [`docs/reports/AGENT_MODE_AUDIT.md`](docs/reports/AGENT_MODE_AUDIT.md),
план Agent IDE: [`docs/AI_AGENT_WORKSPACE_PLAN.md`](docs/AI_AGENT_WORKSPACE_PLAN.md).

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
- **Agent Workspace API** (`workspace_files.rs`): дерево, чтение, сохранение
  с `expectedSha256`, mkdir/move/copy/delete, поиск, upload/download,
  снимки, diff, откат файла/задачи. Все операции — через Runtime
  `tool.execute`; RBAC и ownership проверяются на каждом запросе.
- **Управление workspace владельцем**: создание через `workspace.provision`
  (путь не раскрывается), переименование, описание, мягкое удаление
  (`deleted_at`), сводка usage/задач/активности.
- **Жизненный цикл workspace**: `ai_workspace_lifecycle`, retention
  (`limits.ai_workspace_retention_days`, 30 дней по умолчанию), восстановление,
  фоновая очистка `workspace.delete` с подтверждением Runtime, dry-run,
  retry/backoff, `blocked` при недоказанном владении, очистка snapshots и
  usage только после подтверждения. Runtime проверяет
  `<provisioning_root>/<workspaceId>`, маркер и reparse points.
- **Транзакционные резервы квот**: `ai_quota_reservations` + advisory lock
  пользователя; все изменяющие file-tools, `program.run` и MCP; подтверждение
  фактическим usage, освобождение при известной ошибке, `uncertain` при
  неизвестном результате и административный release.
- **Task list**: фильтры по набору статусов (`statuses`), фильтр по
  `workspaceId`, `humanInput` в task view.
- **Квоты пользователя**: `max_storage_bytes/max_files/max_file_bytes`
  проверяются транзакционно с учётом активных резервов; факт подтверждается
  замером Runtime после операции.
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
- Agent streaming: `stream=true`, `ai.message.delta`, промежуточные
  assistant-сообщения (`task_id`, `iteration`), сохранение tool_calls.
- События: `eventId`/`sequence` в WS и `GET /tasks/{id}/events?after=`;
  `agent.file.changed`; порядок возрастающий, клиент дедуплицирует.
- Capabilities: фильтрация инструментов по `toolId`+`version` и доступным
  программам; Runtime проверяет версию tool.
- Контекст и loop safety: бюджет 400 000 символов, лимит tool result для
  модели, детект повторов, `finish_reason`, объектные `arguments`.
- Retry: только `idempotent`-инструменты, максимум 2 попытки, только для
  `AI_TOOL_FAILED`/пред-исполнительных ошибок; тайм-аут, разрыв и
  `AI_EXECUTION_UNKNOWN` не повторяются.
- Workspaces: безопасный allowlist для новых, `missingSafeTools` и явное
  административное `tools/grant-defaults` для старых; `fs.stat`, `skills.list`.
- Миграции: 24 (`0019`–`0024`), включая `0023_ai_workspace_lifecycle.sql` и
  `0024_ai_quota_reservations.sql`.
- OpenAPI: 207 путей / 251 операция (перегенерирован из кода).

### Runtime

- Отдельный Rust executable/Windows Service, исходящий WSS, reconnect/backoff,
  heartbeat, capabilities, enrollment CLI, token rotate/revoke.
- PolyStrata OpenAI-compatible: models/health/status/chat, SSE, tool calls,
  usage, cancel, HTTP/JSON errors; SSE timeout исправлен.
- Tools: fs.list/read/write/search/stat/mkdir/move/copy/delete/apply_patch,
  `fs.read_base64`/`fs.write_base64` (бинарные файлы для файлового API
  пользователя), `program.run`; JSON Schema, deny-by-default, workspace roots.
- Снимки файлов (`snapshots`) для изменяющих операций: before/after,
  256 КиБ на файл, 2 МиБ на операцию, до 100 файлов; при превышении
  `truncated=true` и откат запрещён.
- `fs.write`/`fs.write_base64` принимают `expectedSha256`: сравнение и запись
  под per-workspace mutex (оптимистическая блокировка без гонки).
- `workspace.provision`: создание каталога нового workspace под
  `storage.provisioning_root` (по умолчанию первый `allowed_roots`), имя —
  только UUID, маркер `.nexus-workspace.json`; путь возвращается только backend.
- `workspace.delete`: подтверждение владения каталогом, атомарный rename в
  `.nexus-trash`, идемпотентное удаление, отказ при symlink/reparse или чужом
  маркере.
- MCP: после вызова инструмента Runtime измеряет фактический usage workspace
  и возвращает его backend для подтверждения квоты.
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
- Idempotency/replay: sequence/timestamp/messageId, receipts.jsonl, caches;
  receipts для успешных и неуспешных операций.
- Receipts rotation: текущий сегмент 10 МБ, архивы `receipts.NNNNNN.jsonl`,
  компактный `receipts.index.jsonl` с `messageId` перед удалением; повторная
  доставка после ротации получает `AI_RECEIPT_ROTATED` без side effect.
- Diagnostics: `runtime check` проверяет roots, token, NEXUS `/health`,
  PolyStrata, programs, network isolation status.

### Frontend

- `/workspace/ai`: Chat/Agent UI, streaming, Markdown/code, attachments,
  regenerate/edit/stop, task inspector, approvals (permission-gated), cancel,
  executions/events, reconnect reload, last-message pagination.
- `/workspace/ai/agent`: самостоятельная IDE.
  - Explorer: ленивое дерево с виртуализацией, создание/переименование/перемещение/
    копирование/удаление, upload/download, drag & drop, поиск по именам и
    содержимому, контекстное меню, индикаторы «изменён агентом» и «AI работает».
  - Editor: вкладки, dirty-индикатор, подсветка (highlight.js), нумерация,
    поиск по файлу, Ctrl+S и автосохранение, Markdown/JSON/изображения,
    diff «мои изменения», обработка конфликта версий (сравнение и выбор).
  - Agent: история с пагинацией, streaming, действия, approvals, стоп,
    повтор, редактирование, выбор модели и Skills.
  - Нижняя панель: Problems, Output, Execution History, Changed Files,
    Workspace Usage; изменение высоты и скрытие.
  - Раскладка: resizer-ы, скрытие панелей, полноэкранный редактор,
    сохранение в localStorage (только раскладка), мобильные переключатели.
  - Task Manager: фильтры, пагинация, детали, события, выполнения, снимки,
    «Открыть в панели» без повторного запуска.
- Инкрементальные события: `after=lastEventId`, дедупликация по `eventId`,
  восстановление порядка и полная синхронизация после reconnect/F5.
- `/system/ai`: предупреждение `missingSafeTools` и управляемое добавление
  безопасных инструментов в workspace.
- `/system/ai`: Overview, Providers, Runtime, Workspaces (+usage refresh),
  Skills, Tools & Programs, MCP (CRUD/discovery/tools/assign), Quotas,
  Security & Logs.
- Home card показывает реальный статус модуля и `ai.access`.

## 3. Проверки

| Проверка | Результат |
|---|---|
| backend `cargo check --all-targets` | успешно |
| backend `cargo test -j 2 --no-fail-fast` | **443 passed, 0 failed, 1 ignored** (`build/phase2-backend-tests.log`) |
| `backend/tests/ai_tests.rs` | **28 passed**, включая lifecycle, purge worker и конкурентные квоты |
| backend unit Agent IDE | 9 тестов: пути/upload/line stats/снимки, `ui_only`, `expectedSha256`, фильтры статусов |
| runtime `cargo test --all-targets` | **41 passed**, 1 ignored (`real_polystrata_smoke`) |
| `openapi_tests` | 5 passed (207 путей / 251 операция) |
| `migrations_tests` | **2 passed** (24 миграции, включая 0023/0024) |
| `nexus migrate` на существующей БД | PASS, 24 миграции, новые таблицы созданы |
| frontend `npm run typecheck` | успешно |
| frontend `npm run lint` | успешно (0 warnings) |
| frontend `npm run test` | **126 passed** (22 файла) |
| frontend `npm run build` | успешно |
| Playwright Agent Mode E2E | исторический прогон: **7 passed** (реальный Runtime + stub PolyStrata); в финальном прогоне 25.11 не перезапускался |
| Playwright Agent Workspace E2E | исторический прогон: **5 passed**: Explorer/Editor/save/preview, конфликт, diff+откат+Task Manager, мобильные панели |
| Playwright E2E (весь набор) | исторический прогон: **38 passed, 1 skipped**; для 1.1 требуется повторный прогон на чистой установке |
| Standalone | собран ранее; после изменений требуется пересборка (`build-standalone.ps1`), в этом прогоне не выполнялась |

## 4. Реализовано, но требует проверки на реальном втором компьютере

1. Полный сценарий: NEXUS на PC-1, Runtime + PolyStrata на PC-2.
2. Реальный WSS через production reverse proxy/сертификат.
3. Реальная PolyStrata и выбранная модель: tool calling, streaming, лимиты.
4. Windows Service под непривилегированной учётной записью + ACL.
5. Network isolation на production Runtime (нужны права администратора).
6. Реальные MCP-серверы (корпоративные/локальные) и их tool schemas.
7. Поведение при сетевых разрывах WAN, DNS, proxy, antivirus.

Сценарий и скрипт подготовлены: [`docs/TWO_MACHINE_CHECK.md`](docs/TWO_MACHINE_CHECK.md),
`scripts/ai-two-machine-verify.ps1`. **Прогон на двух ПК не выполнялся** в этой
среде и не может считаться пройденным.

## 5. Частично реализовано

- `max_parallel_tasks` workspace enforce на уровне очереди задач backend.
- Network isolation: firewall-правило на executable, не process-level;
  AppContainer/WFP per-process не реализован.
- MCP: только Streamable HTTP; stdio отсутствует.
- Quota concurrency: транзакционные резервы в PostgreSQL закрывают
  параллельные операции и несколько Runtime-соединений; фактический usage
  по-прежнему зависит от замера Runtime, но резерв не освобождается вслепую.
- Снимки ограничены 256 КиБ/файл и 2 МиБ/операцию: для больших файлов и
  рекурсивного удаления каталогов откат недоступен (`truncated=true`).
  Откат восстанавливает только файлы, изменённые через Runtime tools.
- Frontend: поиск по именам использует дерево до глубины 6 и лимит Runtime
  (1000 записей), поэтому для очень больших workspace результаты усечены.
- Реальная PolyStrata: сценарий `agent-real.spec.ts` включается переменными
  окружения и в этом прогоне был skipped (сервис отсутствует).

## 6. Не реализовано

- Длительные процессы: `program.start/poll/stop`, `tool.progress`, отдельная
  остановка процесса. `program.run` синхронный. Явно вне текущего этапа.
  Нижняя панель Output показывает реальные stdout/stderr завершённых запусков
  и не выдаётся за терминал.
- Реальная проверка на втором физическом компьютере в этой среде.
- Реальный PolyStrata smoke на машине аудита (сервис отсутствует).
- MCP stdio transport.
- Process-level egress isolation (AppContainer/WFP per-process).
- Полноценный streaming snapshot для файлов > 256 КиБ и рекурсивного удаления
  каталогов; журнал компенсации multi-file restore.
- Терминал (ConPTY) и `program.start/poll/stop`.

## 7. Production-запуск (кратко)

1. Backend: `cargo run -- migrate`, `cargo run -- serve`; задать `NEXUS_SECRET`,
   HTTPS reverse proxy, `NEXUS_COOKIE_SECURE=true`.
2. Frontend: `npm ci && npm run build`.
3. System Management → AI Management: Provider, enrollment code, workspace,
   Skills, programs, MCP.
4. PC-2: build runtime, `runtime.toml`, `enroll`, `install-service.ps1`,
   `runtime check --json`.
5. Проверить §5 `docs/AI_PRODUCTION_AUDIT.md`.
