# NEXUS Agent Mode — технический аудит существующей реализации

Дата аудита: 24 ноября 2026.
Предмет: соответствие фактической реализации NEXUS AI Workspace требованиям
`NEXUS_AGENT_MODE_PLAN.md` и определение точного объёма доработок для
полноценного Agent Mode.

Документ составлен по исходному коду (`backend/src/ai/**`, `runtime/src/**`,
`frontend/src/**`, миграции 0019/0020), а не по README. Все утверждения ниже
сопровождаются ссылкой на файл и функцию/структуру.

---

## 1. Методика и границы аудита

Прочитано и сверено с кодом:

- `README.md`, `COMPLETE.md`, `AI_IMPLEMENTATION_STATUS.md`;
- `docs/AI_WORKSPACE.md`, `docs/AI_PRODUCTION_AUDIT.md`, `docs/ARCHITECTURE.md`,
  `docs/SECURITY.md`, `docs/API.md`, `docs/TESTING.md`, `docs/README.md`;
- `runtime/README.md`, `runtime/runtime.toml.example`;
- весь `backend/src/ai/**` (mod, orchestrator, tools, approvals, conversations,
  providers, workspaces, skills, mcp, events, runtime/*, routes, admin_routes);
- `backend/migrations/0019_ai.sql`, `0020_ai_hardening.sql`;
- `backend/tests/ai_tests.rs`, `backend/tests/ai_runtime_stand.rs` (структура и
  набор сценариев);
- весь `runtime/src/**` (runtime, provider, tools, filesystem, process, patch,
  job, network, protocol, config, mcp, main, service);
- `frontend/src/pages/AiWorkspacePage.tsx`, `AiAdminPage.tsx`,
  `services/ai.ts`, `lib/realtime.ts`, `e2e/ai.spec.ts`;
- `backend/src/permissions/catalog.rs`, `permissions/routes.rs`,
  `users/service.rs`, `auth/session.rs`, `database.rs`, `middleware.rs`.

Ограничения аудита:

- Тесты и сборки в рамках этого аудита **не перезапускались**. Использованы
  результаты, зафиксированные в `docs/AI_PRODUCTION_AUDIT.md` и
  `AI_IMPLEMENTATION_STATUS.md`, и проверка кода.
- Реальная PolyStrata и второй физический компьютер недоступны; вывод о
  совместимости сделан по адаптеру и протоколу, а не по живому прогону.
- Документ `docs/AI_RUNTIME.md`, на который ссылаются `README.md` (строка 45) и
  исходный план, **отсутствует в репозитории**. Это зафиксировано как дефект
  документации.
- Документ `docs/reports/AGENT_MODE_AUDIT.md` на момент начала аудита
  отсутствовал и создан этим отчётом.

---

## 1.1. Статус после реализации (обновление)

После аудита выполнена часть плана. Ниже — фактически реализованные изменения;
исходные разделы отчёта описывают **базовое состояние до доработок** и служат
обоснованием.

Реализовано и проверено тестами:

1. **RBAC:** 10 прав `ai.*` добавлены в `backend/src/permissions/catalog.rs`.
   Кастомные роли и персональные overrides с AI-правами снова валидируются
   (тест `custom_role_can_hold_ai_permissions`, unit-тест каталога).
2. **Конкурентность:** single-runner guard `AiState::begin_run/end_run`;
   проверка `max_parallel_tasks` workspace; частичный уникальный индекс
   активного запуска в диалоге; ответ `AI_TASK_ALREADY_RUNNING`; user message
   не остаётся осиротевшим при отказе квоты.
3. **Approvals:** reject проходит через `finalize` (событие, аудит, очистка
   cancel token); периодический `sweep_expired_approvals` завершает задачи с
   истёкшим approval; ошибка `AI_APPROVAL_EXPIRED`.
4. **Аудит инструментов:** `ai.tool.proposed/completed/failed/cancelled/blocked`
   в `audit_events` с безопасной сводкой (без аргументов и вывода).
5. **Потоковость агента:** Agent Mode переведён на `stream=true`;
   `provider_chat_stream` публикует `ai.message.delta`, сохраняет
   промежуточные assistant-сообщения (`task_id`, `iteration`) и tool_calls;
   пустые tool-call заглушки не создаются.
6. **События:** `eventId`/`sequence` в payload; `GET /ai/tasks/{id}/events?after=`
   для инкрементального восстановления; порядок событий возрастающий.
7. **Capabilities Runtime:** `prepare_tools` пересекает каталог с
   `ai_runtime_nodes.capabilities.tools` (id+version) и доступными программами;
   Runtime проверяет `request.version`; legacy с пустым списком tools не
   фильтруется.
8. **Корректность модели:** нормализация `arguments`-объекта; проверка
   `finish_reason` (`length`/`content_filter` → ошибка), пустой ответ → ошибка.
9. **Контекст и loop safety:** бюджет 400 000 символов с усечением старых
   tool/assistant-сообщений; результат для модели ограничен 32 000 символов;
   повтор одного `(tool, args_hash)` более 3 раз блокируется; сводка предыдущих
   действий диалога добавляется в system prompt.
10. **Файлы и diff:** инструмент `fs.stat`; Runtime возвращает `changedFiles`
    и ограниченный `diff`; Backend публикует `agent.file.changed`; API
    executions отдаёт `result`, `changedFiles`, `diff`; UI показывает файлы,
    diff и exit code.
11. **Skills:** пользовательский выбор Skills на запуск (`skillIds`), серверный
    инструмент `skills.list`, фильтрация `skill.load` по выбранным Skills.
12. **Лимиты:** `ai_user_quotas.max_iterations` (административно), миграция
    `0021_agent_mode_delta.sql` (аддитивная, на рабочей БД не применялась).
13. **Документация:** создан `docs/AI_RUNTIME.md` (ранее отсутствовал).

Результаты прогонов после изменений:

- Backend: `cargo test -j 2 --no-fail-fast` — все цели успешно, включая
  86 unit, 17 AI integration, 5 OpenAPI, 1 migrations (21 миграция), остальные
  модульные наборы; лог `logs/agent-mode-tests.log`.
- Runtime: `cargo test --all-targets` — 31 passed, 1 ignored
  (`real_polystrata_smoke`); лог `logs/agent-mode-runtime-tests.log`.
- Frontend: `npm run typecheck`, `npm run lint`, `npm run test` (104 passed,
  18 files), `npm run build` — успешно.
- Runtime: `cargo build --release`; стенд с реальным
  `nexus-ai-runtime.exe` (`NEXUS_RUNTIME_BIN=… cargo test --test ai_runtime_stand
  -- --ignored`) — **1 passed**: enrollment, Chat streaming, Agent streaming
  tool call, реальная запись файла, usage, cancel с подтверждением, reconnect,
  revoke. Лог `logs/agent-mode-stand.log`. PolyStrata заменена локальным
  OpenAI-compatible stub (как и раньше).
- `cargo fmt --check` — backend и runtime без расхождений.

**Ещё не реализовано (см. §8.3 и §12):** полноценный жизненный цикл длительных
процессов (`program.start/poll/stop`, `tool.progress`), повышение версии
протокола (изменения аддитивные, поэтому версия оставлена 1), E2E-сценарий
Agent с реальным Runtime и сценарий на двух ПК.

> Обновление 24.11.2026: retry-политика, инкрементальное восстановление
> событий в UI, Playwright Agent E2E и двухмашинный сценарий (подготовка)
> закрыты — см. §1.2. Открытыми остаются только длительные процессы и
> фактический прогон на двух ПК.

---

## 1.2. Статус после стабилизации (24 ноября 2026)

Данный раздел дополняет §1.1 и фиксирует завершение оставшихся задач этапов
1–3 и 5–11 **кроме длительных процессов**.

### 1.2.1. Совместимость существующих workspace

- Новый workspace получает безопасный allowlist автоматически
  (`fs.stat`, `skills.list`, low/medium и `skill.load`).
- Для старых workspace права **не расширяются автоматически**:
  `GET /ai/admin/workspaces` возвращает `missingSafeTools`;
  `POST /ai/admin/workspaces/{id}/tools/grant-defaults` добавляет только
  безопасные инструменты; высокорисковые (`fs.delete`, `fs.move`,
  `program.run`) остаются под явным контролем администратора.
- Операция идемпотентна и аудируется (`ai.workspace.tools_granted`).
- UI AI Management показывает предупреждение и кнопку «Добавить безопасные».
- Проверено интеграционным тестом `legacy_workspace_tools_upgrade_is_explicit_and_additive`
  (старый workspace, новый workspace, сохранение `fs.delete`, повторный вызов,
  audit) и E2E-созданием workspace без `allowedTools`.

### 1.2.2. Инкрементальная загрузка событий

- Frontend хранит максимальный `eventId`, запрашивает `?after=<id>` и не
  перечитывает историю при каждом обновлении.
- Слияние HTTP и WS: дедупликация по `eventId`, сортировка по возрастанию,
  обработка запоздавших событий и повторной доставки.
- После reconnect/F5 выполняется одна полная синхронизация последней страницы,
  затем снова инкрементальный режим.
- Ошибка синхронизации не очищает уже показанные события.
- Покрыто unit-тестами `frontend/src/lib/taskEvents.test.ts` и backend-тестом
  `events?after` (уникальность, порядок, ровно более новые события).

### 1.2.3. Миграция 0021

- Отдельный тест на одноразовой БД применяет миграции ≤ 0020, создаёт дубли
  `root_path` (регистронезависимо) и две активные задачи в одном диалоге.
- Миграция обязана упасть на дублях и **не удалять данные**; после ручного
  устранения — примениться; повторный запуск идемпотентен.
- Проверяются колонки `ai_messages.task_id/iteration`, `ai_tasks.skill_ids`,
  `ai_user_quotas.max_iterations`,
  `ai_tool_executions.output_tail/process_id`, индексы
  `ai_workspaces_node_root_uq`, `ai_tasks_active_conversation_uq`, значение по
  умолчанию `max_iterations = 10` и check-ограничение `1..50`.
- На рабочей БД миграция **не применялась**.

### 1.2.4. receipts.jsonl

- Вместо тихого прекращения записи после 10 МБ — ротация в
  `receipts.NNNNNN.jsonl`.
- Неуспешные операции тоже получают receipt: повторная доставка возвращает
  тот же результат/ошибку и не выполняет side effect.
- Перед удалением старого сегмента его `messageId` сохраняются в
  `receipts.index.jsonl`; повторная доставка после ротации получает
  `AI_RECEIPT_ROTATED` без исполнения.
- Настройки: `receipts_max_bytes`, `receipts_max_archives`,
  `receipts_load_limit`.
- Покрыто 4 unit-тестами ротации, индекса, failure-receipt и битых строк.

### 1.2.5. Retry-политика

- Повтор только для `idempotent=true`, не server-side и не MCP.
- Максимум 2 попытки, только `AI_TOOL_FAILED`/пред-исполнительная ошибка.
- Тайм-аут, обрыв соединения, `AI_EXECUTION_UNKNOWN` и
  `AI_WORKSPACE_PATH_DENIED` не повторяются.
- Каждая попытка фиксируется в `attempts`, событии `ai.tool.retrying` и
  audit; операция идентифицируется execution id + idempotency key.
- Покрыто unit-тестом `retry_only_for_idempotent_non_server_tools`.

### 1.2.6. E2E и двухмашинный сценарий

- Добавлены Playwright-сценарии `frontend/e2e/agent.spec.ts` (7 тестов) на
  реальном Runtime и реальном backend с контролируемым PolyStrata stub:
  создание диалога/Agent Mode, streaming, tool call, approval approve/reject,
  результат инструмента, продолжение цикла, файлы/diff, cancel, F5,
  восстановление событий, offline Runtime и reconnect.
- `frontend/e2e/agent-real.spec.ts` — сценарий с реальной PolyStrata,
  включается `E2E_POLYSTRATA_URL`/`E2E_POLYSTRATA_MODEL`; в среде аудита
  **skipped**, mock-подмена запрещена.
- Подготовлены `docs/TWO_MACHINE_CHECK.md` и
  `scripts/ai-two-machine-verify.ps1` (PC-1 HTTPS + PC-2 Runtime/PolyStrata,
  enrollment, задачи, approvals, cancel, restart/reconnect, skills,
  многошаговый прогон). Прогон на двух физических ПК **не выполнялся**.

### 1.2.7. Что осталось не реализовано

- Длительные процессы: `program.start/poll/stop`, `tool.progress`,
  отдельная остановка процесса (`program.run` синхронный).
- Двухмашинный прогон и реальная PolyStrata в среде аудита.
- Проверка на чистой Windows-машине без dev-инструментов (Standalone
  проверяется запуском с очищенным PATH, но не на отдельной ОС).

### 1.2.8. Результаты проверок

| Проверка | Результат |
|---|---|
| backend `cargo test -j 2 --no-fail-fast` | 421 passed, 0 failed |
| `ai_runtime_stand` (реальный Runtime) | 1 passed |
| runtime `cargo test --all-targets` | 36 passed, 1 ignored |
| `migrations_tests` | 2 passed |
| `openapi_tests` | 5 passed (186 путей / 225 операций) |
| frontend typecheck/lint/test/build | успешно, 112 unit-тестов |
| Playwright Agent Mode E2E | 7 passed |
| Playwright полный набор | **33 passed, 1 skipped** (реальная PolyStrata), 0 failed |
| Playwright после финальной правки UI | **8 passed** (`agent.spec.ts` + `ai.spec.ts`) |
| Standalone integrity | 1638 файлов проверено, 0 ошибок; SHA256SUMS 1639 строк, 0 ошибок |
| Standalone с очищенным PATH | launcher/backend/PostgreSQL/Runtime запускаются, dev-инструменты не нужны |

### 1.2.9. Standalone

- `build-standalone.ps1 -SkipTests` (тесты выполнены отдельно) собрал
  `NEXUS-Standalone-release`: 1640 файлов, ~181,5 МБ. Пересборка после
  финальной правки frontend подтвердила побайтное совпадение `frontend`,
  `backend\nexus.exe` и `runtime\nexus-ai-runtime.exe` с текущими сборками
  (`logs/standalone-build-final-2.log`).
- `manifest.json` (1638 файлов) и `SHA256SUMS` (1639 строк) проверены:
  расхождений нет.
- Бинарники Runtime и Backend в релизе побайтно совпадают с текущей
  release-сборкой.
- `tools/Verify-Integrity.ps1` — 0 ошибок.
- `NEXUS.exe --version`, `backend\nexus.exe --version`,
  `database\bin\postgres.exe --version`,
  `runtime\nexus-ai-runtime.exe --version` работают с очищенным PATH
  (`System32`, `Windows`, PowerShell); Rust, Node.js и системный PostgreSQL
  не требуются.
- Проверка на отдельной чистой Windows-машине не выполнялась.

---

## 2. Краткий вердикт

Agent Mode в NEXUS **уже реализован как серверный многошаговый цикл**, а не как
заготовка:

- есть серверный Agent Orchestrator с последовательными циклами
  «модель → tool call → Runtime → tool result → модель»;
- есть реестр инструментов с JSON Schema, риском, allowlist workspace,
  permissions и approval-политикой;
- есть отдельный AI Runtime с исходящим WSS, enrollment, дедупликацией,
  изоляцией workspace, файловыми инструментами, запуском зарегистрированных
  программ, квотами и подтверждённой отменой;
- есть таблицы `ai_tasks` / `ai_task_events` / `ai_tool_executions` /
  `ai_tool_approvals`, восстановление после рестарта Backend и сверка receipt;
- есть UI Agent Mode с approvals, инспектором задач, executions и events.

Исходный `NEXUS_AGENT_MODE_PLAN.md` описывает систему, которая в значительной
части уже существует. План требует не «реализации с нуля», а **дельты**:
устранения найденных дефектов, потокового отображения работы агента,
управления длительными процессами, согласования возможностей Runtime,
сохранения контекста между запусками и административных исправлений.

Найдены реальные дефекты, которые нельзя игнорировать:

1. **Критический:** каталог прав `permissions/catalog.rs` не содержит `ai.*`.
   Из-за этого изменение ролей и персональных прав через API/UI для любых
   AI-прав отклоняется валидацией. Это ломает существующий RBAC-контур, а не
   только Agent Mode.
2. **Высокий:** Backend не фильтрует инструменты по фактическим capabilities
   Runtime и не сверяет версию контракта инструмента; несовместимый Runtime
   обнаруживается только ошибкой в момент выполнения.
3. **Высокий:** в Agent Mode текст модели не передаётся потоково, а
   промежуточные сообщения модели между tool call теряются (не сохраняются ни в
   сообщения, ни в события).
4. **Высокий:** длительные процессы не поддерживаются как отдельная сущность:
   нет `tool.progress`, нет start/poll/terminate, `program.run` блокирует цикл
   до завершения (до 300 с и более).
5. **Высокий:** результаты инструментов предыдущих запусков не попадают в
   историю диалога; новый запуск видит только финальный текст, а не фактические
   изменения и вывод команд.

---

## 3. Фактическая архитектура AI Workspace

### 3.1. Компоненты и ответственность

| Компонент | Реальное расположение | Ответственность |
|---|---|---|
| AI Backend | `backend/src/ai/` | диалоги, история, оркестрация, RBAC, approvals, Skills, MCP, квоты, аудит, realtime, OpenAPI |
| AI Runtime | `runtime/src/` (`nexus-ai-runtime.exe`) | исходящий WSS, клиент PolyStrata, исполнение инструментов, изоляция workspace, процессы, лимиты, receipts |
| PolyStrata | внешний OpenAI-compatible endpoint на AI-ПК | генерация текста и `tool_calls`; не источник прав |
| PostgreSQL | сервер NEXUS | источник истины: `ai_conversations`, `ai_messages`, `ai_tasks`, `ai_task_events`, `ai_tool_executions`, `ai_tool_approvals`, `ai_runtime_*`, `ai_workspaces`, `ai_skills`, `ai_mcp_*` |
| Browser WS | `/api/v1/ws` (`backend/src/realtime/mod.rs`) | адресные уведомления; не источник истины |
| Runtime WS | `/api/v1/ai/runtime/connect` (`backend/src/ai/runtime/mod.rs::ws_handler`) | единственный управляющий канал Runtime, Bearer device token |

Инварианты, реально закреплённые в коде:

- Chat никогда не передаёт `tools`: `conversations::start_chat_generation`
  формирует `tools: Vec::new()` (backend/src/ai/conversations.rs:756–767).
- Agent передаёт только разрешённые инструменты:
  `orchestrator::prepare_tools` → `tools::allowed_for_task` +
  `mcp::allowed_for_task` (backend/src/ai/orchestrator.rs:1392–1404).
- Tool call модели — предложение: `handle_tool_call` проверяет allowlist,
  JSON Schema, workspace, RBAC, программу, approval, затем dispatch
  (backend/src/ai/orchestrator.rs:575–819).
- Runtime не доверяет payload: `Workspace::new` сверяет root с локальным
  `allowed_roots` (runtime/src/filesystem.rs:24–53), `node_id` сверяется с
  device token (runtime/src/runtime.rs:335–342).
- Секреты: API key и MCP secret — AES-256-GCM, device token — SHA-256
  (backend/src/ai/crypto.rs, backend/src/ai/runtime/mod.rs::hash_token).

### 3.2. Схема взаимодействия

```text
Браузер
  │ cookie-сессия + CSRF
  ▼
POST /api/v1/ai/conversations/{id}/messages
  │
  ├─ Chat:  ai_messages(assistant, streaming)
  │        → provider.chat.start (stream=true, tools=[])
  │        → Runtime → PolyStrata SSE
  │        ← provider.chat.delta → ai.message.delta (WS)
  │        ← provider.chat.completed → ai_messages(completed)
  │
  └─ Agent: ai_tasks(queued) + ai_task_events
           → tokio::spawn(orchestrator::run_task)
           → provider.chat.start (stream=false, tools=[...])
           → Runtime → PolyStrata /chat/completions
           ← provider.chat.completed{content, tool_calls}
           → для каждого tool call:
                schema → workspace allowlist → RBAC → MCP assignment
                → risk/approval → ai_tool_executions
                → tool.execute (WSS) → Runtime → ai_tool_executions(result)
                → tool message в context
           → следующий цикл модели
           → finalize: ai_messages(assistant, completed), ai_tasks(completed)

PostgreSQL ← все состояния; /api/v1/ws ← только уведомления
```

### 3.3. Соответствие сущностей ТЗ и фактических таблиц

| Сущность ТЗ | Фактическая таблица | Комментарий |
|---|---|---|
| Conversation | `ai_conversations` | режим `chat`/`agent`, provider, model, workspace |
| AgentRun | `ai_tasks` | статусы, stage, iterations, limits, context, cancel_* |
| AgentEvent | `ai_task_events` | `id bigserial`, kind, stage, message, data |
| ToolCall | `ai_tool_executions` | tool_id, version, args, args_hash, status, result, dispatch_message_id |
| ToolApproval | `ai_tool_approvals` | args_hash, TTL, decision, consumed_at |
| Workspace | `ai_workspaces` | owner, runtime_node, root_path, quotas, allowed_tools/programs, network policy |
| Skill | `ai_skills` + `ai_skill_files` + `ai_skill_versions` + `ai_skill_version_files` | версии и назначения |
| Runtime device | `ai_runtime_nodes` + `ai_runtime_commands` + `ai_runtime_events` + `ai_runtime_receipts` | outbox и журнал |

Вывод: **новые таблицы для Agent Mode не нужны**. Все сущности ТЗ уже имеют
эквиваленты; корректировки требуют только отдельные колонки/индексы (см. §10).

---

## 4. Аудит Backend

### 4.1. AI Chat

Реализация: `backend/src/ai/conversations.rs`.

- `send` (строка 517) проверяет владельца, состояние диалога, `ai.chat.use` или
  `ai.agent.use`, провайдера; отдельно защищает от второго параллельного чата в
  одном диалоге (`state.ai.chats`, `begin_chat`).
- `start_chat_generation` (731) отправляет `provider.chat.start` с
  `stream=true` и пустым `tools`.
- `run_chat_stream` (823) принимает `ChatChunk::Delta/Done/Failed/Cancelled`,
  публикует `ai.message.delta`, сохраняет накопленный текст не чаще ~900 мс,
  финализирует сообщение.
- `stop_generation` (974) отправляет `provider.chat.cancel` и помечает
  сообщение `cancelled`.
- Регенерация (`regenerate`, 1023) и редактирование (`edit_message`, 1123)
  реализованы; история ограничена `MAX_HISTORY_MESSAGES = 60`
  (`history_messages`, 687), вложения — 5 файлов по 5 МБ.

Чат соответствует ТЗ и **не требует переписывания**.

### 4.2. Agent Orchestrator

Реализация: `backend/src/ai/orchestrator.rs`.

- `start_task` (90): проверка диалога/провайдера/workspace, совпадение Runtime
  провайдера и workspace, `reserve_task_slot`, запись `ai_tasks` в `queued`,
  событие `ai.task.queued`, spawn цикла, audit `ai.task.created`.
- `run_task_inner` (349): deadline, отмена, проверка лимитов
  (`iterations >= max_iterations`, `tool_calls_count >= max_tool_calls`),
  обработка pending tool calls, затем новый запрос модели.
- `process_pending` (512): последовательное выполнение tool calls, сохранение
  каждого результата как `role: "tool"` с `tool_call_id`, персист контекста.
- `handle_tool_call` (575): поиск инструмента по model_name, парсинг аргументов,
  JSON Schema (`tools::validate_arguments`), workspace allowlist
  (`tools::authorize_workspace`), RBAC (`owner_can` по `spec.permission`),
  MCP-назначение (`mcp::authorize_tool`), проверка программы, approval,
  dispatch.
- `execute_runtime_tool` (1032) и `execute_server_tool` (935): запись execution,
  `tool.execute`, ожидание через oneshot, сохранение результата и usage.
- `finalize` (1655): создаёт финальное assistant-сообщение, обновляет задачу,
  пишет `ai.task.*` и audit.
- `provider_chat` (1444): отправляет нестриминговый `provider.chat.start` с
  `tools` и `stream=false`.

Фактически это полноценный **последовательный** многошаговый цикл. Параллельные
tool calls (OpenAI `parallel_tool_calls`) не выполняются параллельно — это
осознанное ограничение, соответствующее ТЗ.

### 4.3. Интеграция с PolyStrata и формат OpenAI-compatible

Реализация: `runtime/src/provider.rs`.

- `ProviderClient::chat` (113) формирует `/chat/completions` с `messages`,
  `stream`, `tools`, `temperature`, `top_p`, `max_tokens`.
- `parse_completion` (320) разбирает нестриминговый ответ: `content`,
  `tool_calls[].id/function.name/function.arguments`, `usage`, `finish_reason`.
- `stream_completion` (370) разбирает SSE, `data: [DONE]`, `delta.content`,
  `delta.tool_calls` с накоплением по `index` (id, name, arguments), `usage`.
- `collected_calls` (483) собирает частичные tool calls в целые; если id
  отсутствует — синтезирует `call_{index}`.

**Может ли текущая интеграция поддерживать полный цикл Tool Calling?**

Да, структурно — да: формат запроса/ответа, `tool_call_id`, множественные
`tool_calls`, сборка аргументов из потока и передача результата как `tool`
сообщения реализованы и покрыты unit-тестами
(`provider.rs::tests::parses_sse_stream_with_tool_calls`). Ограничения:

1. Agent использует `stream=false`; сборка потоковых tool calls в реальном
   Agent-цикле не задействована (только unit-тест).
2. `arguments` ожидаются строкой. Если PolyStrata вернёт `arguments` объектом
   (встречается в OpenAI-совместимых серверах), значение молча заменяется на
   `{}` (`parse_completion`, 343–347) — вызов уйдёт с пустыми аргументами и
   будет отклонён схемой.
3. `finish_reason` разбирается, но Orchestrator его не анализирует: `length` и
   `content_filter` трактуются как обычное завершение.
4. Реальный smoke против установленной PolyStrata помечен `#[ignore]`
   (`real_polystrata_smoke`, 635) и в среде аудита не выполнялся.

### 4.4. Потоковая передача и tool calls

- Chat: потоковая передача реализована полностью (`ai.message.delta`).
- Agent: **отсутствует** потоковая передача текста модели. Запрос идёт с
  `stream=false`, поэтому UI не видит ни «печатающегося» ответа, ни
  промежуточных пояснений модели перед tool call. Промежуточный `content`
  сохраняется только в `ai_tasks.context` и теряется для пользователя.
- Потоковые tool calls: Runtime умеет собирать их, но в Agent-режиме они
  приходят целиком в одном ответе. Backend не имеет кода сборки частичных
  tool calls, потому что не запрашивает поток.
- `tool.progress`: константа и обработчик есть
  (`protocol.rs:34`, `runtime/mod.rs:1051`), но Runtime **никогда не отправляет**
  это событие. Прогресс длительных операций в UI отсутствует.

### 4.5. Реестр инструментов и политика

Реализация: `backend/src/ai/tools.rs`.

- `ToolSpec` (20): id, model_name, version, risk, approval_policy, permission,
  idempotent, timeout, max_output_bytes, JSON Schema, server_side, mcp.
- Каталог (`static_catalog`, 102): `fs.list`, `fs.read`, `fs.write`, `fs.search`,
  `fs.mkdir`, `fs.move`, `fs.copy`, `fs.delete`, `fs.apply_patch`, `program.run`,
  `skill.load`.
- `validate_arguments` (445) — `jsonschema`; `arguments_hash` (462) — SHA-256;
  `authorize_workspace` (470) — статус workspace + allowlist;
  `approval_required` (437) — `always` либо `risk >= medium`;
  `allowed_for_task` (507) — пересечение каталога, политик и allowlist.
- Политики хранятся в `ai_tool_policies` и синхронизируются
  `ensure_policies` при миграции/старте (`database.rs::sync_permission_catalog`).
- MCP-tools добавляются динамически и повторно авторизуются перед вызовом
  (`mcp::allowed_for_task`, `mcp::authorize_tool`).

Расхождения с ТЗ:

- нет `fs.stat`;
- нет `terminal.execute`/`terminal.start/poll/terminate` (есть только
  `program.run` по реестру зарегистрированных программ);
- нет отдельного инструмента `skills.list` (метаданные Skills подаются в
  system prompt, содержимое — через `skill.load`);
- каталог Backend и каталог Runtime (`runtime/src/tools.rs::descriptors`)
  продублированы вручную и не сверяются автоматически;
- Backend не сверяет `version` инструмента с Runtime;
- флаг `idempotent` нигде не используется для решения о повторе.

### 4.6. Подтверждения (approvals)

Реализация: `backend/src/ai/approvals.rs`, `orchestrator.rs`.

- `create` (39): TTL 15 минут, `args_hash`, точные аргументы.
- `decide` (140): owner или `ai.admin.manage`; атомарный
  `pending → approved/rejected`; audit `ai.approval.approved/rejected`.
- `consume` (245): атомарно, только `approved`, не использованный, hash
  совпадает, не истёк.
- `resume_after_approval` (orchestrator 1784) повторно проверяет hash и
  запускает цикл; `handle_tool_call` потребляет approval непосредственно перед
  dispatch.
- Route `approve/reject` требует `ai.actions.approve`
  (backend/src/ai/routes.rs:567, 594).
- Reject переводит задачу в `cancelled` (approvals.rs:210–233).

Дефекты:

- reject-ветка не вызывает `orchestrator::finalize`: нет `ai.task.cancelled`
  события/аудита, не очищается cancel token; UI получает только realtime.
- истёкший approval не завершает задачу автоматически: `expire_stale`
  вызывается только из `pending_for_user`/`decide`, поэтому задача может
  остаться в `waiting_approval` навсегда (до ручной отмены).
- аргументы в UI показываются как редактированный JSON без человекочитаемого
  описания эффекта/команды.

### 4.7. Хранение и восстановление

- Схема: 26 AI-таблиц (0019 + 0020), см. §3.3.
- `ai_task_events.id bigserial` даёт монотонный идентификатор события.
- `ai_tool_executions` уникален по `(task_id, tool_call_id)` и
  `dispatch_message_id`; `ai_runtime_commands.message_id` уникален.
- `recover_interrupted` (backend/src/ai/mod.rs:160) обрабатывает:
  `streaming → failed`, `queued → requeue`, `running` без in-flight execution →
  resume, `running` с in-flight execution → `awaiting_receipt`,
  `waiting_approval` сохраняется, `cancel_requested` повторно отменяется.
- `spawn_recovery_tasks` (orchestrator 1943) запускает восстановление и watchdog
  на 5 минут; `resume_pending_for_node` (2012) подхватывает задачи после
  подключения Runtime; `reconcile_node`/`reconcile_execution` (2038/2074)
  запрашивает `execution.status` и восстанавливает результат либо фиксирует
  `AI_EXECUTION_UNKNOWN`.
- Runtime хранит `receipts.jsonl` и кэши завершённых provider/tool операций
  (runtime/src/runtime.rs:71–102), поэтому повторный `messageId` не выполняет
  side effect.

Пробелы:

- восстановление не покрывает in-flight `provider.chat.start`: при обрыве
  соединения `fail_pending_for_node` отдаёт `AI_EXECUTION_UNKNOWN`, задача
  завершается ошибкой, даже если запрос был идемпотентным чтением.
- нет гарантии единственного исполнителя задачи: `run_task` может быть
  запущен одновременно из `start_task`, `resume_after_approval`,
  `reconcile_execution`, `resume_pending_for_node`; блокировки на task_id нет.
- `receipts.jsonl` при размере > 10 МБ перестаёт пополняться (runtime.rs:85–90)
  без ротации — дедупликация после рестарта деградирует.

### 4.8. RBAC и аудит

Права объявлены в миграции `0019_ai.sql` (10 прав `ai.*`) и назначены ролям.
Проверки выполняются в handlers (`actor.require`), в orchestrator перед
dispatch (`owner_can`), в approvals и на Runtime (allowlist/workspace).

**Дефект RBAC (исправлен):** `backend/src/permissions/catalog.rs` (единый
каталог прав, используемый `permissions::routes::validate_permissions:103`,
`users::service` (строка 391), `database::sync_permission_catalog` и
`AuthUser` для суперадмина) содержал только 20 прав Minecraft/Cloud/System и
**не содержал ни одного `ai.*`**. Последствия до исправления:

- нельзя было создать/изменить кастомную роль с `ai.*` — `422 Неизвестное
  право: ai.access` (системные роли, включая User и Administrator, всё равно
  защищены от изменения, поэтому встроенные роли это не затрагивало);
- нельзя было выдать/запретить `ai.*` персонально пользователю;
- суперадмин получал `catalog::all_keys()`, то есть без `ai.*` (спасала только
  проверка `is_superadmin` в `AuthUser::can`).

После исправления права добавлены в каталог; поведение проверено тестами.

Аудит: `system::audit::record` вызывается для создания задачи, завершения,
запроса отмены, решений по approval, enrollment/revoke Runtime. **Запуск и
завершение каждого инструмента в `audit_events` не пишутся** — только в
`ai_task_events` и `ai_tool_executions`.

### 4.9. Лимиты и конкурентность

- Per-user: `max_concurrent_tasks` (по умолчанию 1), `max_tasks_per_day` (50),
  `max_tool_calls_per_task` (40), `max_task_seconds` (900) —
  `workspaces::reserve_task_slot` (142).
- Задача: `max_iterations` жёстко `DEFAULT_MAX_ITERATIONS = 10`
  (orchestrator:32,175) без административной настройки.
- Инструмент: min(policy.max_timeout_seconds, spec.timeout_seconds, 600).
- Runtime: семафор `max_parallel_tasks` (глобально на процесс), лимиты вывода,
  файлов, глубины, времени программ.
- **Не enforce:** `ai_workspaces.max_parallel_tasks` (читается/отображается, но
  нигде не проверяется), пользовательские storage-квоты
  (`max_storage_bytes/max_files/max_file_bytes`).
- **Нет уникальности** `(runtime_node_id, root_path)` для workspace: два
  workspace могут указывать на один каталог. Mutex в Runtime и квоты привязаны к
  `workspace_id`, поэтому такая конфигурация обходит сериализацию и удваивает
  квоту.
- **Нет запрета** второго активного запуска в одном диалоге: при
  `max_concurrent_tasks > 1` пользователь может отправить второе сообщение, и
  два цикла будут писать в один диалог/workspace. При дефолте 1 второй запуск
  отклоняется по квоте, но user message уже сохранён (осиротевшее сообщение).

### 4.10. WebSocket и события

- Пользовательские события: `ai.message.started/delta/completed/failed`,
  `ai.task.queued/started/progress/completed/failed/cancelled`,
  `ai.tool.started/progress/waiting_approval/completed/failed`,
  `ai.runtime.connected/disconnected` (`backend/src/ai/events.rs`,
  `orchestrator::task_event`, `runtime::publish_task_event`).
- Runtime-канал: конверт `protocolVersion/messageId/type/timestamp/nodeId/
  correlationId/taskId/sequence/payload`; защита от replay по sequence и
  timestamp ±15 минут; `messageId`-дедупликация.
- Payload не содержит `eventId`/`sequence` для пользовательских событий, хотя
  `ai_task_events.id` существует. Инкрементальное восстановление «с
  последнего события» невозможно — UI перечитывает состояние целиком.
- `ai.runtime.connected` публикуется ещё и в `publish_user(node_id)`, где
  `node_id` — не пользователь; адресация бессмысленна.
- `agent.file.changed` и отдельного события об изменённых файлах нет.

---

## 5. Аудит AI Runtime

### 5.1. Enrollment и идентификация

`backend/src/ai/runtime/mod.rs`:

- `create_enrollment_code` (239) — одноразовый `nxe_…`, TTL 60–3600 с, hash в
  БД, audit.
- `enroll` (292) — проверка hash/TTL/использования, создание
  `ai_runtime_nodes`, device token `nxa_…` (в БД SHA-256), audit.
- `ws_handler` (658) — Bearer token, проверка `revoked/disabled`, upgrade.
- `rotate_token`, `revoke_node`, `set_status` — реализованы; закрытие
  соединения через `RuntimeHub::close`.

Соответствует ТЗ. `docs/AI_RUNTIME.md` отсутствует, но `runtime/README.md`
описывает процедуру.

### 5.2. Канал, heartbeat, reconnect

`runtime/src/runtime.rs`:

- `run` (104): цикл reconnect с экспоненциальной задержкой и jitter,
  `max_parallel_tasks` → семафор.
- `connect_once` (190): Bearer + `user-agent`, writer task, `runtime.hello`,
  `runtime.capabilities`, heartbeat каждые `heartbeat_seconds`, probe
  capabilities каждые 300 с, таймаут чтения 120 с.
- Backend: `run_socket` (716) — heartbeat 20 с, `HEARTBEAT_TIMEOUT = 90 с`,
  закрытие при отзыве, публикация online/offline.
- `resend_pending` (594) переотправляет только `pending` команды; `sent`
  обрабатываются через reconcile.

Сильные стороны: устойчивый backoff, явная дедупликация, проверка timestamp.

Ограничения: reconnect не восстанавливает незавершённую генерацию модели;
in-flight команда с потерей соединения превращается в контролируемую ошибку.

### 5.3. Инструменты Runtime

`runtime/src/tools.rs`:

- `descriptors` (46): `fs.list/read/write/search/mkdir/move/copy/delete/
  apply_patch`, `program.run`.
- `execute` (232): поиск descriptor, повторная JSON Schema, создание
  `Workspace`, mutex на `workspace_id` для изменяющих операций, диспетчеризация.
- `fs_*` (300–527): лимиты, квоты (`ensure_quota`), атомарная запись,
  `apply_patch` с двухфазной проверкой.
- `program_run` (572): проверка квоты до и во время выполнения (монитор каждые
  750 мс), отмена, usage после.

### 5.4. Изоляция workspace и путей

`runtime/src/filesystem.rs`:

- `Workspace::new` (24): абсолютный root, существование, канонизация, запрет
  reparse point, вхождение в `allowed_roots`, проверка reparse-предков.
- `resolve` (64): `normalize_relative`, поиск ближайшего существующего предка,
  канонизация, containment, повторная проверка reparse по компонентам.
- `normalize_relative` (124): запрет `/`, `\`, `C:`, `..`, NUL, `:`,
  завершающих точку/пробел, длины компонента.
- `resolve_existing` (105): запрет корня, существование, reparse, канонизация.

Это соответствует ТЗ и покрыто unit-тестами. Остаточный TOCTOU-риск между
проверкой и операцией снижен повторными проверками, но не устранён полностью
(Windows не даёт простого `openat`-эквивалента).

### 5.5. Процессы

`runtime/src/process.rs`:

- `validate_args` (106): allowlist/шаблоны, запрет control chars, абсолютных
  путей, UNC, `..`, лимит 32 аргумента/500 символов.
- `run` (155): только зарегистрированный executable, cwd внутри workspace и не
  reparse, `env_clear` + allowlist, Job Object, memory limit, timeout,
  stdout/stderr с ограничением, `kill_tree` при timeout/cancel.
- `job.rs`: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `PROCESS_MEMORY`.
- `network.rs`: временный outbound firewall block для `network_policy=deny`,
  fail-closed без прав администратора.

Сильные стороны: нет shell, нет произвольной командной строки, дерево процессов
гарантированно завершается.

Ограничения для длительных задач:

- процесс синхронный: цикл агента ждёт завершения; промежуточный вывод
  недоступен;
- нет идентификатора процесса для UI, нет poll/terminate отдельного процесса;
- нет списка активных процессов и их состояния между командами;
- отмена возможна только всей задачей (`task.cancel`), не отдельным процессом.

### 5.6. Отмена

- Backend: `request_runtime_cancel` (orchestrator 1563) ставит
  `cancel_status='requested'`, отправляет `task.cancel`, watchdog 75 с →
  `AI_CANCEL_UNCONFIRMED`.
- Runtime: `cancel_task` (runtime.rs 903) отправляет `task.cancel_ack`, отменяет
  токены активных сообщений, ждёт до 30 с завершения, затем
  `task.cancelled{confirmed}` только при 0 активных.
- Backend принимает `task.cancelled` и переводит задачу в `cancelled`
  (runtime/mod.rs:1151–1243).
- Отмена в `waiting_approval` завершается локально без Runtime (side effect не
  отправлялся) — корректно.

Соответствует ТЗ. Нюанс: при отмене задачи, где активен только provider-запрос,
backend удаляет локальный pending и прерывает ожидание (`AI_TASK_CANCELLED`);
поздний `provider.chat.failed` от Runtime уже игнорируется, а терминальный
статус разруливает подтверждение `task.cancelled`/watchdog. Это корректно, но
означает, что причина отмены генерации не сохраняется в истории провайдера.

### 5.7. Capabilities и версионирование

- `RuntimeCapabilities` (protocol.rs:101): runtime_version, platform,
  protocol_version, models, polystrata, tools[], programs[],
  network_isolation.
- Backend сохраняет capabilities в `ai_runtime_nodes.capabilities`
  (`store_capabilities`), программы — в `ai_programs` (`store_programs`).
- `protocol_version_supported` требует **точного** равенства `1`
  (runtime/mod.rs:195); несовместимая версия отклоняется.
- Backend **не использует** список tools/programs из capabilities при
  формировании набора инструментов: `prepare_tools` смотрит только каталог и
  workspace. Инструмент, которого нет на Runtime, будет предложен модели и
  упадёт с `AI_TOOL_NOT_ALLOWED` при выполнении.
- `ToolExecuteRequest.version` передаётся, но Runtime его не проверяет
  (`runtime/src/tools.rs::execute` ищет только `tool_id`).

### 5.8. Пригодность для длительных многошаговых задач

| Аспект | Оценка |
|---|---|
| Многошаговый цикл до 10 итераций | пригоден |
| Долгая генерация модели (до 180 с на шаг) | пригоден, но без стриминга в UI |
| Долгая команда/сборка (до 300 с) | частично: блокирует цикл, нет прогресса, нет отдельного стопа |
| Процесс дольше task timeout | будет отменён вместе с задачей; отдельного жизненного цикла нет |
| Несколько параллельных задач | ограничено per-user и семафором Runtime; per-workspace лимит не работает |
| Восстановление после разрыва | частично: receipt reconciliation, но in-flight provider → ошибка |
| Долгий контекст | нет бюджета контекста; риск переполнения |

---

## 6. Аудит Frontend

Реализация: `frontend/src/pages/AiWorkspacePage.tsx` (1246 строк),
`frontend/src/services/ai.ts`, `frontend/src/lib/realtime.ts`.

Реализовано:

- переключение Chat/Agent (`Segmented`, строки 625–637), gate по `ai.agent.use`;
- выбор провайдера/модели/workspace, отображение usage;
- отправка сообщения, вложения, остановка (chat) и отмена задачи (agent);
- потоковый чат: `ai.message.delta` дописывает контент в кэш TanStack Query
  (266–285), fallback-опрос при `streaming`;
- approvals: карточки с риском, аргументами, TTL, кнопками (1090–1127);
- инспектор задачи: статус, progress, итерации, tools count, cancelStatus,
  executions с аргументами/результатом, события (1129–1243);
- восстановление после reconnect: `realtime.onStatus` инвалидирует все AI-запросы
  (247–261);
- активный диалог хранится в URL `?c=`, поэтому перезагрузка возвращает диалог;
- задача выбирается из списка (приоритет активной), опрос 2–4 с;
- ошибки задач и сообщений отображаются.

Чего нет:

- потокового текста агента и промежуточных сообщений модели;
- отдельного списка изменённых файлов и diff;
- карточки длительного процесса с промежуточным выводом и стопом;
- выбора Skills пользователем при запуске (Skills назначаются администратором,
  UI показывает только количество);
- просмотра файлов workspace (задокументированное ограничение);
- выбора старых задач из истории (только текущая/последняя);
- отображения `exitCode` выполнения (поле есть в API, но не рендерится);
- инкрементального восстановления событий (перечитывается весь список).

E2E `frontend/e2e/ai.spec.ts` — только smoke: открытие страниц и вкладки MCP;
реальный Agent-сценарий в Playwright не проверяется.

---

## 7. Таблица соответствия техническому заданию

| Функциональность | Реализовано | Частично | Отсутствует | Файл / модуль (подтверждение) |
|---|---|---|---|---|
| Agent Orchestrator | ✔ | | | `backend/src/ai/orchestrator.rs::start_task/run_task_inner/process_pending/handle_tool_call` |
| Tool Calling | ✔ | | | `backend/src/ai/tools.rs::ToolSpec/validate_arguments`; `runtime/src/provider.rs::parse_completion/stream_completion`; `runtime/src/tools.rs::execute` |
| Многошаговый цикл агента | ✔ | | | `orchestrator.rs::run_task_inner` (цикл 387–504), `process_pending` (512) |
| Реестр инструментов | | ✔ | | `tools.rs::catalog` (11 инструментов), `ai_tool_policies`; нет `fs.stat`, `terminal.*`, нет фильтра по capabilities Runtime |
| Файловые инструменты | | ✔ | | `runtime/src/tools.rs::fs_*`; `filesystem.rs`; отсутствует `fs.stat` |
| Terminal execution | | | ✔ | только `program.run` (`runtime/src/process.rs`); произвольного терминала нет |
| Skills | | ✔ | | `backend/src/ai/skills.rs` (import/version/assign/load), `tools.rs::skill.load`; нет `skills.list` как tool и пользовательского выбора |
| Runtime capabilities | | ✔ | | `runtime/src/runtime.rs::base_capabilities/probe_capabilities`; `backend/.../runtime/mod.rs::store_capabilities`; не используются для фильтрации tools, version не сверяется |
| Approval system | ✔ | | | `backend/src/ai/approvals.rs::create/decide/consume`; `orchestrator.rs::resume_after_approval`; TTL 15 мин, args_hash |
| Отмена процессов | ✔ | | | `orchestrator.rs::request_runtime_cancel`; `runtime/src/runtime.rs::cancel_task`; `process.rs::kill_tree`; Job Object |
| Управление состоянием задач | ✔ | | | `ai_tasks`, `ai_task_events`, `orchestrator.rs::finalize`; нет single-runner lock и уникальности активного запуска в диалоге |
| WebSocket-события | | ✔ | | `events.rs`, `task_event`, `publish_task_event`; нет eventId/sequence в payload, нет agent-стриминга, нет `agent.file.changed` |
| RBAC | | ✔ | | 10 прав в `0019_ai.sql`, проверки в handlers/orchestrator/approvals; **`permissions/catalog.rs` не содержит `ai.*`** |
| История действий | | ✔ | | `ai_task_events`, `ai_tool_executions`, `audit_events`; tool start/finish не аудируются |
| Восстановление после reconnect | | ✔ | | `ai/mod.rs::recover_interrupted`, `orchestrator.rs::spawn_recovery_tasks/resume_pending_for_node/reconcile_execution`, `runtime.rs::ReceiptStore`; in-flight provider → ошибка |
| Изоляция workspace | ✔ | | | `runtime/src/filesystem.rs::Workspace/normalize_relative/resolve`; `allowed_roots`; нет уникальности root_path и лимита параллельных задач workspace |

Дополнительные строки, важные для полноты:

| Функциональность | Реализовано | Частично | Отсутствует | Файл / модуль |
|---|---|---|---|---|
| Потоковый ответ Agent | | | ✔ | `orchestrator.rs::provider_chat` (`stream:false`) |
| Промежуточные сообщения модели | | | ✔ | content пишется только в `ai_tasks.context`, не в `ai_messages`/events |
| Прогресс инструмента | | | ✔ | `kind::TOOL_PROGRESS` объявлен и обрабатывается, Runtime не отправляет |
| Длительные процессы (start/poll/stop) | | | ✔ | `runtime/src/process.rs::run` синхронный |
| Список изменённых файлов / diff | | ✔ | | `runtime/src/patch.rs` возвращает `files/action/additions/deletions`; агрегации и UI нет |
| Идемпотентный повтор | | ✔ | | флаг `ToolSpec.idempotent` не используется; receipts есть |
| Аудит инструментов | | | ✔ | `audit_events` только task/approval; executions — в `ai_task_events` |
| Выбор Skills пользователем | | | ✔ | Skills назначаются админом (`ai_skill_assignments`), UI не выбирает |
| Проверка finish_reason | | | ✔ | `ProviderChatCompleted.finish_reason` не анализируется |
| Бюджет контекста | | | ✔ | `push_message` ограничивает 200 сообщений, но не объём; результат tool до 200 000 символов |
| Защита от повторяющихся вызовов | | | ✔ | только `max_iterations` и `max_tool_calls_per_task` |

---

## 8. Проблемы и архитектурные риски

Критичность: **критическая**, **высокая**, **средняя**, **низкая**.

### 8.1. Критические (статус на момент аудита; K-1 исправлен)

**K-1. Каталог RBAC не содержал `ai.*` (исправлено).**
`backend/src/permissions/catalog.rs:4-41` против `backend/migrations/0019_ai.sql:7-35`.
`validate_permissions` (`permissions/routes.rs:103`) и проверка в
`users/service.rs:388-398` отклоняли любое `ai.*`, поэтому нельзя было создать
кастомную роль или персональный override с AI-правами. Встроенные роли
(`User`, `Administrator`) — системные и не редактируются, поэтому их это не
затрагивало. Исправлено добавлением 10 прав в каталог; тесты
`custom_role_can_hold_ai_permissions` и `ai_permissions_are_registered`
проходят.

### 8.2. Высокие

**H-1. Backend не согласует инструменты с capabilities Runtime.**
`prepare_tools` (`orchestrator.rs:1392`) не смотрит `ai_runtime_nodes.capabilities`
и `ai_programs.available`; `ToolExecuteRequest.version` не проверяется Runtime.
Модель получает инструмент, которого нет на устройстве; ошибка возникает только
при выполнении, после расхода токенов и времени.
Направление: пересекать каталог с `capabilities.tools` (id+version) при
подготовке задачи, отклонять несовместимый protocol/version с понятным кодом,
не показывать `program.run`, если программа недоступна.

**H-2. Agent Mode не передаёт текст модели потоково и теряет промежуточные
сообщения.** `provider_chat` использует `stream=false` (`orchestrator.rs:1461`);
`content` промежуточного assistant-сообщения только добавляется в `context`
(460–471) и нигде не сохраняется. Пользователь видит только «Модель запросила
выполнение инструментов», без пояснений агента; после перезагрузки они
отсутствуют. Нарушает требование потокового отображения и «истории действий».

**H-3. Нет модели длительного процесса.** `program.run` синхронен
(`runtime/src/process.rs:155`), `tool.progress` не отправляется, нет
start/poll/terminate. Сборка/тесты/dev-server блокируют цикл, промежуточный
вывод недоступен, отдельный процесс нельзя остановить, не отменяя всю задачу.

**H-4. Контекст между запусками теряет действия агента.** `history_messages`
(`conversations.rs:687`) выбирает только `user/assistant` и только `content`;
`tool_calls`/`tool_call_id`/результаты инструментов в `ai_messages` не
сохраняются (колонки в схеме есть, но не используются). Новый запуск в том же
диалоге не знает, какие файлы изменены и какие команды выполнялись; возможны
повторные и противоречивые действия.

**H-5. Один workspace root может быть назначен нескольким workspace.**
Нет уникального индекса `(runtime_node_id, root_path)`
(`0019_ai.sql`, `ai_workspaces`). Mutex в Runtime и квоты ключуются
`workspace_id` (`runtime/src/tools.rs:25,248`), поэтому два workspace на одном
каталоге обходят сериализацию и удваивают лимиты. Плюс
`ai_workspaces.max_parallel_tasks` не проверяется (`workspaces.rs` — только
чтение/запись поля).

**H-6. Нет гарантии единственного исполнителя AgentRun.**
`run_task` вызывается из `start_task`, `resume_after_approval`,
`reconcile_execution`, `resume_pending_for_node` без блокировки на `task_id`.
Два цикла могут одновременно звать модель и писать финальное сообщение
(`finalize` проверяет терминальный статус до транзакции, но не сериализует
исполнителей). Риск дублирования сообщений, гонок контекста и повторных
dispatch (БД спасает только от дублей `(task_id, tool_call_id)`).

### 8.3. Средние

**M-1. Дублирование каталога инструментов Backend и Runtime.** Два ручных
списка (`backend/src/ai/tools.rs::static_catalog`,
`runtime/src/tools.rs::descriptors`) без автоматической сверки; расхождение
проявляется в runtime-ошибке. Нужен контрактный тест/генерация или сравнение
capabilities при hello.

**M-2. `arguments` объектом теряются.** `runtime/src/provider.rs:343-347`
подставляет `{}` вместо объекта. OpenAI-совместимые серверы иногда так делают.
Нужна нормализация (объект → строка) и явная ошибка вместо тихой подмены.

**M-3. `finish_reason` игнорируется.** Обрыв по `length`/`content_filter`
считается успешным завершением (`orchestrator.rs:473-487`); задача может быть
объявлена выполненной при усечённом ответе.

**M-4. Нет защиты от зацикливания по смыслу.** Ограничены только число
итераций и вызовов; повтор одного и того же `(tool, args)` или повторное чтение
файла без прогресса не детектируется. Нет retry-политики для временных ошибок
провайдера.

**M-5. Нет бюджета контекста.** `push_message` ограничивает 200 сообщений
(`orchestrator.rs:1360`), но каждый tool result — до 200 000 символов
(`tool_result_text`, 1285), итог может превысить окно модели и привести к
`AI_PROVIDER_HTTP_ERROR`/обрыву. Нужны лимит суммарного контекста и усечение
середины с пометкой.

**M-6. Истёкший approval не завершает задачу.** `expire_stale` вызывается
только из `pending_for_user`/`decide`; задача в `waiting_approval` может висеть
бесконечно. Нужен фоновый watchdog/периодическая проверка.

**M-7. Reject-ветка обходит `finalize`.** `approvals.rs:210-233` не пишет
`ai.task.cancelled` в `ai_task_events` и audit, не очищает cancel token; история
задачи неполная.

**M-8. Аудит не покрывает инструменты.** Требование ТЗ «запуск/завершение
каждого инструмента» не выполнено в `audit_events`; данные есть только в
`ai_tool_executions` (может быть очищено/не входит в общий журнал).

**M-9. Нет eventId/sequence в пользовательских событиях.** `ai_task_events.id`
не передаётся в WS; инкрементальное восстановление невозможно, UI перечитывает
всё. Нет `agent.file.changed`.

**M-10. Receipts Runtime деградируют.** При `receipts.jsonl > 10 МБ` запись
молча прекращается (`runtime.rs:85-90`), новых receipt нет → после рестарта
возможен повтор неидемпотентной операции по тому же `messageId`.

**M-11. In-flight provider при обрыве считается неизвестным.** Нет
классификации «повтор безопасен» по `idempotent`; задача падает, даже если это
был read-only вызов. Нужна политика: идемпотентные операции можно безопасно
повторить/продолжить, неидемпотентные — нет.

**M-12. Осиротевшее user message при отказе в квоте.** `conversations::send`
сохраняет сообщение до `start_task` (`conversations.rs:571-620`); при
`AI_TASK_LIMIT_REACHED` сообщение остаётся без ответа.

**M-13. Один диалог — несколько запусков.** Нет partial unique index на
активный `ai_tasks(conversation_id)`; при `max_concurrent_tasks > 1` два цикла
пишут в один диалог и workspace.

### 8.4. Низкие

- **L-1.** `ai.runtime.connected` публикуется `publish_user(node_id)` — id
  устройства не является id пользователя (адресация в пустоту).
- **L-2.** `fs.stat` отсутствует; `fs.list` рекурсивен и не заменяет
  метаданные одного файла.
- **L-3.** `apply_patch` не возвращает unified diff, только статистику; UI не
  агрегирует изменённые файлы.
- **L-4.** `exitCode` не отображается в TaskPanel, хотя API его возвращает.
- **L-5.** `program.run` по умолчанию `approval_policy='always'` — автономный
  цикл «тест → исправление → тест» требует подтверждения на каждом запуске
  (настраивается администратором, но политика не описана в UI).
- **L-6.** `docs/AI_RUNTIME.md` отсутствует, хотя на него ссылаются README и
  план; README/COMPLETE содержат устаревшие числа тестов.
- **L-7.** E2E AI — только smoke; нет сценария Agent с реальным Runtime.
- **L-8.** `max_iterations` захардкожен (10) и не управляется администратором.
- **L-9.** `ai_workspaces` не поддерживает список пользователей — только
  владелец; «назначение workspace группе» из ТЗ не реализовано.
- **L-10.** `ToolOutcome.result` может содержать абсолютные пути в
  `result_text`; Runtime вычищает свои roots (`sanitize`), но гарантия зависит
  от корректной настройки roots.
- **L-11.** Таблица `ai_runtime_receipts` создана миграцией `0020`, но не
  используется кодом Backend (receipt хранится в
  `ai_tool_executions.receipt`); `docs/AI_WORKSPACE.md` описывает её как
  рабочую. Это расхождение схемы и документации (не функциональный дефект).
- **L-12.** `docs/AI_WORKSPACE.md` упоминает `ai_runtime_receipts` и
  `recovery_state` без указания, что receipt reconciliation работает только для
  tool executions, но не для provider-запросов.

---

## 9. Повторно используемые компоненты (не переписывать)

Backend:

- `backend/src/ai/mod.rs` — состояние, recovery.
- `backend/src/ai/orchestrator.rs` — цикл, guard-проверки, approval-resume,
  cancel, reconcile.
- `backend/src/ai/tools.rs` — каталог, схемы, политики, hash.
- `backend/src/ai/approvals.rs` — TTL/one-time.
- `backend/src/ai/conversations.rs` — чат, история, вложения, regenerate/edit.
- `backend/src/ai/workspaces.rs` — доступ, квоты, usage.
- `backend/src/ai/skills.rs` — импорт/версии/назначения/загрузка.
- `backend/src/ai/mcp.rs` — реестр/SSRF/назначения.
- `backend/src/ai/runtime/*` — hub, outbox, protocol, enrollment.
- `backend/src/ai/events.rs`, `crate::realtime`, `crate::system::audit`,
  `crate::core::idempotency`, `crate::middleware`.

Runtime:

- `runtime/src/runtime.rs` — reconnect, heartbeat, receipts, dispatch, cancel.
- `runtime/src/provider.rs` — PolyStrata, SSE, tool calls.
- `runtime/src/tools.rs`, `filesystem.rs`, `patch.rs`, `process.rs`, `job.rs`,
  `network.rs`, `config.rs`.

Frontend:

- `frontend/src/pages/AiWorkspacePage.tsx` — каркас, Chat/Agent, approvals,
  инспектор, reconnect.
- `frontend/src/services/ai.ts` — типизированный API-клиент.
- `frontend/src/lib/realtime.ts` — канал событий.
- `frontend/src/pages/AiAdminPage.tsx` — администрирование.
- `frontend/src/styles/pages/ai.css` — дизайн-система.

Инфраструктура:

- миграции 0019/0020 и все 26 AI-таблиц;
- `scripts/ai-runtime-stand.ps1`, `scripts/ai-two-machine-check.ps1`,
  `scripts/ai-polystrata-smoke.ps1`;
- `backend/tests/ai_tests.rs`, `backend/tests/ai_runtime_stand.rs`,
  `frontend/e2e/ai.spec.ts`, `build-standalone.ps1`.

---

## 10. Необходимые изменения (сводка, без реализации)

### 10.1. Исправления существующего (P0)

1. `backend/src/permissions/catalog.rs` — добавить `ai.*` (10 прав).
2. `backend/src/ai/workspaces.rs::reserve_task_slot` — проверять
   `ai_workspaces.max_parallel_tasks` по `workspace_id`; миграция 0021:
   `unique index on ai_workspaces (runtime_node_id, lower(root_path))`.
3. `backend/src/ai/approvals.rs::decide` — reject через `finalize`, очистка
   cancel token; периодическая проверка истёкших approvals (watchdog).
4. `backend/src/ai/orchestrator.rs` — single-runner guard на `task_id`
   (pg advisory lock или in-memory DashSet) вокруг `run_task_inner`.
5. `backend/src/ai/conversations.rs::send` — не создавать user message при
   отказе в квоте (или помечать её failed) и запрещать второй активный запуск
   в диалоге (partial unique index `ai_tasks(conversation_id)` для активных).
6. `backend/src/ai/orchestrator.rs::finalize` и audit — записывать
   `ai.task.cancelled` при reject; добавить audit `ai.tool.*` с безопасной
   сводкой.

### 10.2. Новые возможности (P1–P2)

7. Agent streaming:
   - `orchestrator.rs::provider_chat` → `stream=true` + обработка
     `provider.chat.delta` через существующий `PendingRequest::Chat`;
   - персист промежуточных assistant-сообщений (`ai_messages` уже имеет
     `tool_calls`, `tool_call_id`; добавить `task_id`/`iteration` при
     необходимости);
   - события `ai.message.delta/completed` для Agent без новых WS-механизмов.
8. Capability negotiation:
   - пересечение `tools::allowed_for_task` с `capabilities.tools` (id+version);
   - проверка `version` в `runtime/src/tools.rs::execute`;
   - поддержка protocol `1..=2` с явным набором возможностей; фильтрация
     `program.run` по `available`.
9. Long-running processes:
   - новые инструменты `program.start/poll/stop` (или `terminal.*` поверх
     реестра программ) в `backend/src/ai/tools.rs` и `runtime/src/tools.rs`;
   - реестр активных процессов в Runtime, ring buffer вывода, `tool.progress`
     с throttling;
   - отмена отдельного процесса и всей задачи.
10. Context & results:
   - бюджет контекста и усечение tool result перед отправкой модели;
   - детект повторяющихся вызовов;
   - классификация retry по `ToolSpec.idempotent`;
   - проверка `finish_reason`;
   - нормализация `arguments`-объекта.
11. Files/diff:
   - `fs.stat`;
   - агрегированный список изменённых файлов и `agent.file.changed`;
   - diff в результате `fs.apply_patch`/`fs.write` (с лимитом).
12. Skills UX:
   - `skills.list` (метаданные) и выбор Skills на запуск
     (`POST .../messages` → `skillIds`), фильтрация в `prepare_tools`.
13. Frontend:
   - таймлайн действий и потоковый текст агента;
   - карточка процесса, список изменённых файлов/diff, exit code;
   - история запусков диалога;
   - улучшенные approvals (эффект/область/таймер).
14. Events/recovery:
   - `eventId`/`sequence` в WS payload; `GET /tasks/{id}/events?after=`;
   - `agent.file.changed`;
   - корректная адресация `ai.runtime.connected`.
15. Runtime reliability:
   - ротация `receipts.jsonl` вместо тихого отключения;
   - receipt для failed/cancelled операций;
   - heartbeat/health для `program`-процессов.
16. Документация: создать `docs/AI_RUNTIME.md` (или объединить с
   `runtime/README.md` и обновить ссылки), обновить `docs/API.md`,
   `docs/openapi.json`, `docs/TESTING.md`, `COMPLETE.md`,
   `AI_IMPLEMENTATION_STATUS.md`.

### 10.3. Совместимость

- Все перечисленные поля протокола добавляются как `#[serde(default)]`, чтобы
  старый Runtime продолжал работать в режиме ограниченных возможностей.
- `protocolVersion` повышается только при несовместимом изменении; Backend
  принимает известный диапазон, Runtime отклоняет неизвестный.
- Миграция 0021 — только `add column if not exists` и `create unique index if
  not exists`; на рабочей БД не применяется в рамках аудита.

---

## 11. Итог: что работает, что требует доработки, что отсутствует

### Уже работает (не переписывать)

- AI Chat со streaming, историей, вложениями, regenerate/edit/stop.
- Серверный Agent Orchestrator с последовательным многошаговым циклом.
- JSON Schema, allowlist workspace, RBAC-проверки, risk/approval, TTL и
  args_hash, одноразовое потребление.
- Runtime enrollment/revoke/rotate, исходящий WSS, heartbeat, reconnect,
  sequence/timestamp/messageId-защита.
- Файловые инструменты и изоляция workspace (traversal/symlink/junction/
  reparse, canonical containment).
- Запуск зарегистрированных программ без shell, Job Object, лимиты, сетевая
  изоляция.
- Отмена с подтверждением Runtime и watchdog.
- Восстановление задач после рестарта Backend, receipt reconciliation.
- Skills (импорт, версии, назначения, load-on-demand), MCP Streamable HTTP.
- UI Agent Mode: approvals, инспектор, executions/events, reconnect.
- 26 AI-таблиц, idempotency, audit, module gate, Standalone-сборка Runtime.

### Требует доработки

- RBAC-каталог (критично).
- Потоковый текст агента и сохранение промежуточных сообщений.
- Прогресс инструментов и жизненный цикл длительных процессов.
- Согласование tools/version с capabilities Runtime.
- Контекст между запусками (tool results в истории/сводке).
- Конкурентность: single-runner, уникальность root_path, per-workspace лимит,
  запрет второго запуска в диалоге.
- Аудит инструментов, reject-ветка, истёкшие approvals.
- Бюджет контекста, loop detection, retry-политика, `finish_reason`,
  `arguments`-объект.
- Event id/sequence и инкрементальное восстановление.
- UI: файлы/diff, процесс, история запусков, выбор Skills.
- Документация `docs/AI_RUNTIME.md` и актуализация отчётов.

### Отсутствует и требует разработки

- Полноценное управление длительными процессами (start/poll/stop, отдельный
  стоп, промежуточный вывод).
- Список изменённых файлов и diff в UI (частичные данные есть в результатах
  инструментов).
- Пользовательский выбор Skills на запуск.
- Инкрементальное восстановление событий по sequence.
- Контрактный тест соответствия каталогов инструментов Backend и Runtime.
- E2E-сценарий Agent с реальным Runtime и второй физический ПК.

---

## 12. Заключение

Agent Mode в NEXUS — не прототип: серверный цикл, Runtime, approvals, Skills,
MCP, изоляция и восстановление уже реализованы и покрыты интеграционными
тестами. Аудит выявил дефекты и пробелы, часть которых **уже устранена** в
рамках выполнения плана: RBAC-каталог, конкурентность запусков, approvals,
аудит инструментов, потоковость агента, capabilities Runtime, контекст и loop
safety, `fs.stat`/diff, выбор Skills, миграция 0021 и документация Runtime.

Оставшиеся разрывы с ТЗ на момент первичного аудита (историческая часть):

- длительные процессы не имеют отдельного жизненного цикла
  (`program.start/poll/stop`) и `tool.progress`;
- нет автоматической retry-политики для идемпотентных операций;
- версия протокола оставлена `1`, так как изменения аддитивные и обратно
  совместимые; отдельный bump потребуется только при несовместимом изменении;
- UI получает `eventId`/`sequence`, но пока перечитывает события целиком
  (API `after` готов к использованию);
- E2E-сценарий Agent с реальным Runtime и проверка на двух физических ПК
  остаются обязательными для объявления Production Ready.

### 12.1. Обновление после стабилизации (24.11.2026)

Перечисленные ниже пункты **закрыты** и подтверждены прогонами из §1.2:

- retry-политика: только идемпотентные инструменты, 2 попытки, без повтора
  при тайм-ауте/разрыве/`AI_EXECUTION_UNKNOWN`;
- инкрементальное восстановление событий в UI: `after`, дедупликация,
  порядок, полная синхронизация после reconnect;
- миграция `0021` проверена на отдельной БД (дубли и идемпотентность);
- `receipts.jsonl` ротируется; перед удалением сегмента `messageId`
  сохраняются в индекс, повторное исполнение запрещено;
- Playwright Agent E2E: 7 сценариев на реальном Runtime; полный набор —
  33 passed, 1 skipped;
- Standalone собран; manifest/SHA256SUMS и запуск с очищенным PATH проверены;
- документация OpenAPI/API/COMPLETE/STATUS/TESTING обновлена.

Остаются открытыми и не выдаются за готовые:

1. **Длительные процессы** (`program.start/poll/stop`, `tool.progress`,
   отдельная остановка процесса) — этап 4 не начинался.
2. **Двухмашинный прогон** с реальной PolyStrata: сценарий
   `docs/TWO_MACHINE_CHECK.md` и `scripts/ai-two-machine-verify.ps1`
   подготовлены, но не выполнялись. `agent-real.spec.ts` в этой среде skipped.
3. **Чистая Windows-машина без dev-инструментов**: Standalone проверен с
   очищенным PATH, но не на отдельной ОС/ВМ.
4. Версия протокола остаётся `1`; повышение требуется только при
   несовместимом изменении контракта.

Agent Mode можно считать стабильным для эксплуатации после закрытия пункта 2;
до этого Production Ready не заявляется.
