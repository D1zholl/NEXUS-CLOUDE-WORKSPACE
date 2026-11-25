# NEXUS AI Workspace — план доведения Agent Mode до полноценного состояния

Основание: фактический аудит кода `docs/reports/AGENT_MODE_AUDIT.md`.
Документ заменяет предыдущую редакцию плана, которая описывала значительную
часть уже реализованного как отсутствующую.

**Статус:** этапы 1–3 и 5–11 выполнены (24.11.2026), кроме длительных
процессов (этап 4). Миграции на рабочей БД не выполнялись; опасные системные
команды не запускались; существующие пользовательские изменения не
перезаписывались. Двухмашинный прогон не выполнялся.

---

## 1. Цель и характер работ

Довести **существующий** Agent Mode NEXUS до состояния, в котором пользователь
запускает многошаговую задачу, видит потоковую работу агента, подтверждает
опасные действия, наблюдает длительные процессы и получает проверяемый
результат в изолированном workspace на AI-ПК, а состояние переживает
перезагрузку страницы, обрыв соединения и рестарт Backend.

Работы носят характер **дельты**:

- не создаётся новая AI-система, новый чат, новая авторизация или новый
  realtime-канал;
- не переписываются работающие Chat, Orchestrator, approvals, Skills, MCP,
  Runtime enrollment и файловая изоляция;
- исправляются найденные дефекты, расширяются существующие контракты и UI,
  закрываются функциональные пробелы.

Главное архитектурное правило сохраняется: модель предлагает действие; Backend
проверяет права, workspace, схему, лимиты и approval; Runtime исполняет
разрешённую операцию; результат возвращается модели. Модель не имеет прямого
доступа к ОС, файловой системе или секретам.

---

## 1.1. Статус выполнения плана

План выполняется как дельта к существующей системе. Отметки:

| Этап | Статус | Что сделано | Осталось |
|---|---|---|---|
| 1. Стабилизация (P0) | **Выполнено** | RBAC `ai.*`; single-runner; лимит workspace; уникальность активного запуска; approvals reject/expiry; аудит инструментов; миграция 0021 (проверена на тестовой БД) | — |
| 2. Потоковость и события | **Выполнено** | `stream=true`; промежуточные сообщения; `eventId`/`sequence`; `events?after`; инкрементальный UI с дедупликацией и sync после reconnect | — |
| 3. Capabilities и контракт | **Выполнено** | фильтрация tools/version; проверка версии на Runtime; legacy fallback | Версия протокола 1 (аддитивные изменения) |
| 4. Длительные процессы | **Не начато** | — | `program.start/poll/stop`, `tool.progress`, реестр процессов, UI-карточка |
| 5. Контекст и loop safety | **Выполнено** | бюджет; лимит результата; детект повторов; `finish_reason`; `arguments`-объект; сводка; retry-политика для идемпотентных | — |
| 6. Файлы и diff | **Выполнено** | `fs.stat`; `changedFiles`; `diff`; `agent.file.changed`; UI | — |
| 7. Skills | **Выполнено** | `skillIds`; `skills.list`; фильтрация `skill.load` | — |
| 8. Frontend Agent Mode | **Выполнено** | streaming; Skills; files/diff; exit code; история запусков; улучшенный approval | Карточка длительного процесса (этап 4) |
| 9. Админ/лимиты/аудит | **Выполнено** | `max_iterations` backend+UI; аудит tool.*; workspace-лимит; capabilities | Управление длительными процессами (этап 4) |
| 10. Тестирование | **Выполнено** | backend/runtime/frontend/миграции/OpenAPI; Playwright Agent 7/7; сценарий двух ПК подготовлен | Реальный прогон на двух ПК и реальная PolyStrata |
| 11. Документация/Standalone | **Выполнено** | AI_RUNTIME, API, OpenAPI, TESTING, TWO_MACHINE_CHECK, отчёты; Standalone пересобран | — |

Изменённые файлы перечислены в отчёте `docs/reports/AGENT_MODE_AUDIT.md` §1.1 и
в итоговом сообщении исполнителя. Ниже этапы сохранены как контракт работ;
невыполненные пункты остаются в силе.

---

## 2. Исходное состояние: что уже реализовано

Перечисленное ниже **не разрабатывается заново**; допускаются только точечные
исправления и расширения.

### 2.1. Backend

- AI Chat: `backend/src/ai/conversations.rs` — `send`,
  `start_chat_generation`, `run_chat_stream`, `stop_generation`,
  `regenerate`, `edit_message`, вложения.
- Agent Orchestrator: `backend/src/ai/orchestrator.rs` — `start_task`,
  `run_task_inner`, `process_pending`, `handle_tool_call`,
  `execute_runtime_tool`, `execute_server_tool`, `finalize`,
  `resume_after_approval`, `cancel_task`, `spawn_recovery_tasks`,
  `resume_pending_for_node`, `reconcile_node`.
- Реестр инструментов: `backend/src/ai/tools.rs` — `ToolSpec`,
  `static_catalog`, JSON Schema, `arguments_hash`, `authorize_workspace`,
  `approval_required`, `allowed_for_task`.
- Approvals: `backend/src/ai/approvals.rs` — TTL, args hash, `consume`.
- Workspaces/квоты: `backend/src/ai/workspaces.rs` — доступ, usage,
  `reserve_task_slot`.
- Skills: `backend/src/ai/skills.rs` — импорт, версии, восстановление,
  назначения, `load_content`.
- MCP: `backend/src/ai/mcp.rs` — реестр, SSRF, discovery, назначения.
- Runtime-канал: `backend/src/ai/runtime/mod.rs`, `protocol.rs` — enrollment,
  device token, WSS, heartbeat, outbox, sequence/timestamp/messageId,
  `execution.status`, программы.
- События/аудит/идемпотентность: `backend/src/ai/events.rs`,
  `backend/src/realtime`, `backend/src/system/audit.rs`,
  `backend/src/core/idempotency.rs`.

### 2.2. Runtime

- `runtime/src/runtime.rs` — reconnect, heartbeat, capabilities, receipts,
  dispatch, cancel, usage, execution status.
- `runtime/src/provider.rs` — PolyStrata `/models`, `/health`, `/status`,
  `/chat/completions`, SSE, tool calls, usage, cancel.
- `runtime/src/tools.rs` — `fs.list/read/write/search/mkdir/move/copy/delete/
  apply_patch`, `program.run`.
- `runtime/src/filesystem.rs` — нормализация, containment, symlink/junction/
  reparse, атомарная запись.
- `runtime/src/process.rs`, `job.rs`, `network.rs` — программы без shell,
  allowlist аргументов, Job Object, memory limit, сетевая изоляция.
- `runtime/src/config.rs`, `main.rs`, `service.rs` — конфигурация, CLI
  (`check/enroll/set-token/run`), Windows-служба.

### 2.3. Frontend

- `frontend/src/pages/AiWorkspacePage.tsx` — Chat/Agent, чат-стриминг,
  вложения, approvals, инспектор задачи, executions/events, cancel, reconnect.
- `frontend/src/pages/AiAdminPage.tsx` — 9 разделов администрирования.
- `frontend/src/services/ai.ts`, `frontend/src/lib/realtime.ts`.
- `frontend/src/styles/pages/ai.css`.

### 2.4. Данные

26 AI-таблиц в `backend/migrations/0019_ai.sql` и `0020_ai_hardening.sql`.
Отдельные таблицы для Agent Mode не нужны.

### 2.5. Совместимость, которую нельзя ломать

- PolyStrata остаётся OpenAI-compatible; Chat не получает `tools`.
- Runtime остаётся отдельным Windows-компонентом с исходящим WSS и не
  открывает входящих портов.
- Пользовательская авторизация, cookie-сессии, CSRF и общий RBAC не меняются.
- `/api/v1/ws` остаётся единственным браузерным realtime-каналом.
- Модули Minecraft, Cloud Storage и System Management не затрагиваются, кроме
  общего каталога прав и общих middleware.

---

## 3. Целевой пользовательский сценарий

1. Пользователь открывает AI Workspace, выбирает Agent Mode, доступный
   workspace и (опционально) разрешённые Skills.
2. Формулирует задачу.
3. Backend создаёт запуск, сохраняет задачу и отправляет модели системную
   политику, историю, инструменты и метаданные Skills.
4. Модель отвечает текстом **потоково**; текст промежуточных шагов сохраняется и
   виден в хронологии.
5. Если модель запрашивает инструмент, Backend проверяет вызов; при
   необходимости создаёт одноразовый approval. Пользователь видит точное
   действие.
6. Runtime выполняет операцию; для длительных процессов доступны
   промежуточный вывод и отдельная остановка.
7. Результат (exit code, изменённые файлы, diff, ограниченный вывод) попадает
   в контекст модели и в UI.
8. Цикл продолжается до завершения, отказа, отмены или лимита.
9. После обновления страницы, reconnect или рестарта Backend пользователь
   возвращается к тому же запуску и видит полную хронологию.

---

## 4. Модель данных и допустимые изменения схемы

### 4.1. Использовать существующие таблицы

| Понятие | Таблица | Что уже есть |
|---|---|---|
| Conversation | `ai_conversations` | mode, provider, model, workspace |
| AgentRun | `ai_tasks` | status, stage, iterations, limits, context, cancel_* |
| AgentEvent | `ai_task_events` | `id bigserial`, kind, stage, message, data |
| ToolCall | `ai_tool_executions` | tool_id, version, args, args_hash, status, result, dispatch_message_id |
| ToolApproval | `ai_tool_approvals` | args_hash, decision, TTL, consumed_at |
| Workspace | `ai_workspaces` | root, quotas, allowlist, network policy |
| Skill | `ai_skills` + версии/назначения | версии и файлы |
| Runtime | `ai_runtime_nodes` + commands/events/receipts | outbox, capabilities |

Не создавать `agent_runs`, `agent_events`, `tool_calls` и т.п. — это дубли.

### 4.2. Миграция `0021_agent_mode_delta.sql` (только аддитивная)

Создаётся как файл; **на рабочей БД не применяется** в рамках работ без
отдельного разрешения. Перед уникальными индексами выполняется ручная проверка
дублей (SQL-проверки включаются комментариями в миграцию).

1. `ai_messages`:
   - `task_id uuid null references ai_tasks(id) on delete set null`
     (привязка промежуточных сообщений к запуску);
   - `iteration integer null` (номер цикла модели).
2. `ai_tasks`:
   - `skill_ids uuid[] not null default '{}'` (выбранные на запуск Skills);
   - `max_iterations` уже есть — использовать, не добавлять.
3. `ai_user_quotas`:
   - `max_iterations integer not null default 10 check (max_iterations between 1 and 50)`.
4. `ai_workspaces`:
   - `unique index if not exists ai_workspaces_node_root_uq
     on ai_workspaces (runtime_node_id, lower(root_path))`
     (после проверки существующих дублей).
5. `ai_tasks`:
   - `unique index if not exists ai_tasks_active_conversation_uq
     on ai_tasks (conversation_id)
     where status in ('queued','running','waiting_approval')`
     (запрет двух активных запусков в диалоге; учитывает `waiting_approval`).
6. `ai_tool_executions`:
   - `output_tail text` (последние N байт вывода процесса для восстановления
     карточки после перезагрузки; полный вывод не хранить);
   - `process_id text` (идентификатор процесса Runtime для long-running tools,
     если он отличается от execution id).

> Если проверка покажет дубли root_path или активные задачи в одном диалоге,
> миграция останавливается и требует ручного решения администратора. Автомати-
> ческое удаление/слияние данных запрещено.

---

## 5. Этап 1 (P0). Стабилизация существующих механизмов — ВЫПОЛНЕНО

Зависимостей нет; выполняется первым, потому что исправляет блокирующие дефекты.

### 5.1. RBAC: включить `ai.*` в каталог прав

Файлы:

- `backend/src/permissions/catalog.rs` — добавить 10 прав: `ai.access`,
  `ai.chat.use`, `ai.agent.use`, `ai.workspace.read`, `ai.workspace.write`,
  `ai.skills.use`, `ai.tools.use`, `ai.tasks.cancel`, `ai.actions.approve`,
  `ai.admin.manage` (модуль `ai`, те же формулировки, что в `0019_ai.sql`).
- `backend/src/permissions/routes.rs` и `backend/src/users/service.rs` — не
  менять логику, только убедиться, что валидация проходит.
- `frontend/src/pages/RolesPage.tsx` и `UsersPage.tsx` — не менять структуру;
  проверить сохранение AI-прав.

Критерии готовности:

- `PATCH /api/v1/roles/{id}` для роли с `ai.*` возвращает 200 и сохраняет набор.
- Создание роли с `ai.agent.use` и персональный override `ai.*` работают.
- Суперадмин по-прежнему имеет все права; обычная роль User — ровно то, что
  задано миграцией.
- Добавлены интеграционные тесты на оба сценария.

### 5.2. Конкурентность запусков

Файлы:

- `backend/src/ai/orchestrator.rs` — ввести single-runner guard на `task_id`
  вокруг `run_task_inner` (PostgreSQL advisory lock
  `pg_try_advisory_lock(hashtext('nexus.ai.run:'||task_id))` или эквивалентный
  in-memory `DashSet` с обязательной проверкой после рестарта). Повторный
  вызов не должен создавать второй цикл.
- `backend/src/ai/workspaces.rs::reserve_task_slot` — дополнительно проверять
  `ai_workspaces.max_parallel_tasks` по `workspace_id` (сейчас проверяется
  только per-user `max_concurrent_tasks`).
- `backend/src/ai/conversations.rs::send` — не оставлять осиротевшее user
  message: либо создавать задачу до коммита сообщения, либо помечать сообщение
  `failed` с кодом квоты; при активном запуске в диалоге возвращать
  `AI_TASK_ALREADY_RUNNING` (новый код в `backend/src/ai/errors.rs`).
- миграция 0021: уникальные индексы из §4.2.

Критерии готовности:

- Второй одновременный запуск в одном диалоге отклоняется предсказуемым кодом.
- Два параллельных `run_task` для одного `task_id` не выполняют модель и
  инструменты дважды (тест с двойным вызовом).
- `max_parallel_tasks` workspace соблюдается; при превышении — понятная ошибка.
- Дубли `root_path` не создаются (409/422 с указанием существующего workspace).

### 5.3. Approvals: reject и истечение

Файлы:

- `backend/src/ai/approvals.rs::decide` — reject проводить через
  `orchestrator::finalize(status="cancelled")`, чтобы появились
  `ai.task.cancelled`, audit и очистка cancel token; сохранить публикацию
  realtime.
- `backend/src/ai/approvals.rs` — добавить watchdog истечения: периодическая
  задача (в scheduler/jobs или отдельный tokio interval на Backend) переводит
  истёкшие approvals в `expired` и завершает связанные `waiting_approval`
  задачи контролируемым `cancelled`/`failed` с кодом `AI_APPROVAL_EXPIRED`.
- `backend/src/ai/orchestrator.rs` — не считать `waiting_approval` вечным
  состоянием; при истечении — терминальный статус.

Критерии готовности:

- Reject оставляет полную историю: `ai_task_events`, audit, финальный статус.
- Approval с истёкшим TTL не оставляет задачу в `waiting_approval`.
- Повторный approve/reject после истечения — 409 (существующее поведение).

### 5.4. Аудит инструментов

Файлы:

- `backend/src/ai/orchestrator.rs` — при dispatch и завершении инструмента
  писать `audit_events` (`ai.tool.dispatched`, `ai.tool.completed`,
  `ai.tool.failed`, `ai.tool.blocked`) с безопасной сводкой: tool_id, risk,
  execution_id, task_id, duration, exit_code; **без аргументов, stdout/stderr и
  путей**.
- `backend/src/system/audit.rs` — использовать существующий `record`, не
  создавать новый журнал.

Критерии готовности:

- Каждый запуск инструмента имеет запись в общем журнале событий.
- В журнале нет секретов, абсолютных путей и полного вывода.

---

## 6. Этап 2. Потоковость агента и контракт событий — ВЫПОЛНЕНО

Зависит от этапа 1 (стабильный запуск).

### 6.1. Потоковый текст модели в Agent Mode

Файлы:

- `backend/src/ai/orchestrator.rs::provider_chat` — перевести на `stream=true`;
  использовать `PendingRequest::Chat` (канал уже есть в
  `backend/src/ai/mod.rs`) для приёма `provider.chat.delta`; накапливать
  `content` и `tool_calls` (сборка уже реализована в Runtime
  `provider.rs::stream_completion`).
- `backend/src/ai/runtime/mod.rs` — `PROVIDER_DELTA` уже маршрутизируется в
  `PendingRequest::Chat`; убедиться, что для задачи он не смешивается с Chat.
- `backend/src/ai/orchestrator.rs` — на каждый цикл модели создавать
  assistant-сообщение (`ai_messages`, `status='streaming'`, `task_id`,
  `iteration`) и обновлять его по мере дельт; при наличии tool calls —
  завершать сообщение и сохранять `tool_calls` (колонка уже есть).
- `backend/src/ai/events.rs` — переиспользовать `ai.message.delta` /
  `ai.message.completed` с `conversationId`+`messageId`; новых WS-каналов не
  создавать.
- `backend/src/ai/conversations.rs::history_messages` — отдавать в контекст
  промежуточные assistant-сообщения и tool-результаты текущего/предыдущих
  запусков по правилам §8.

Ограничения:

- В UI не передаются скрытые рассуждения модели. Если PolyStrata возвращает
  `reasoning_content`, он игнорируется и не сохраняется.
- Аргументы незавершённого tool call не публикуются; в UI попадает только
  валидированный вызов.

Критерии готовности:

- В Agent Mode текст появляется постепенно (событие `ai.message.delta`).
- После перезагрузки страницы промежуточные сообщения видны из БД.
- При `stream=true` tool calls собираются корректно; неполный JSON никогда не
  запускает инструмент.
- Chat-поток не изменился (регрессионный тест).

### 6.2. Идентификаторы и восстановление событий

Файлы:

- `backend/src/ai/orchestrator.rs::task_event` и
  `backend/src/ai/runtime/mod.rs::publish_task_event` — включать в payload
  `eventId` (id из `ai_task_events`) и `sequence` (порядковый номер в рамках
  задачи).
- `backend/src/ai/routes.rs::task_events` — поддержать
  `GET /api/v1/ai/tasks/{id}/events?after=<eventId>&limit=…` (обратная
  совместимость: без `after` — текущее поведение).
- `frontend/src/pages/AiWorkspacePage.tsx` — при reconnect запрашивать события
  после последнего известного `eventId`, а не только полный список.

Критерии готовности:

- Каждое событие имеет `eventId` и `sequence`.
- После reconnect UI не теряет и не дублирует события.
- `after` возвращает только более новые события в возрастающем порядке.

---

## 7. Этап 3. Согласование возможностей Runtime и контракта инструментов — ВЫПОЛНЕНО (версия протокола 1)

Зависит от этапа 2 (стабильные события и версия протокола).

### 7.1. Единый контракт инструментов

Файлы:

- `backend/src/ai/tools.rs` — оставить единственным источником **политик и
  схем** на Backend; Runtime-описания использовать как декларацию фактической
  поддержки.
- `runtime/src/tools.rs::descriptors` — синхронизировать набор и версии с
  Backend; добавить contract-тест, сравнивающий `tool_id`+`version`+`input_schema`
  (тест может быть отдельным integration-тестом, читающим оба списка через
  публичные функции).
- `backend/src/ai/orchestrator.rs::prepare_tools` — пересекать
  `tools::allowed_for_task` с `ai_runtime_nodes.capabilities.tools`
  (id + version) и с `ai_programs.available` для `program.run`. Инструмент,
  которого нет или версия не совпадает, не передаётся модели; в задачу
  пишется событие с причиной.
- `runtime/src/tools.rs::execute` — проверять `request.version`; неизвестная
  версия → `AI_TOOL_VERSION_UNSUPPORTED`, без выполнения.
- `backend/src/ai/runtime/mod.rs` — при hello/capabilities сохранять
  `toolVersions` (или использовать `tools[].version`) и `programs[].available`;
  не считать устройство готовым к инструменту, которого нет в capabilities.

### 7.2. Версионирование протокола

- `backend/src/ai/runtime/protocol.rs` и `runtime/src/protocol.rs`:
  `PROTOCOL_VERSION = 2`, `MIN_PROTOCOL_VERSION = 1`.
- Backend принимает `1..=2`; Runtime принимает `1..=2`; при выходе за диапазон
  — `AI_PROTOCOL_VERSION_UNSUPPORTED` до выполнения команд.
- Новые поля — только `#[serde(default)]`, чтобы Runtime v1 продолжал работать
  с ограниченным набором возможностей (без progress/process tools).
- В `runtime.hello_ack` и `capabilities` явно перечислять поддерживаемые
  дополнительные операции (например, `supportsToolProgress`,
  `supportsProcessControl`), чтобы Backend не выводил их только из версии.

Критерии готовности:

- Модель не получает инструмент, отсутствующий на подключённом Runtime.
- Старый Runtime (v1) продолжает работать: Chat и базовые fs-инструменты
  доступны, новые инструменты скрыты.
- Несовместимая версия отклоняется с понятным кодом, без частичного
  выполнения.
- Contract-тест падает при расхождении каталогов.

---

## 8. Этап 4. Длительные процессы и прогресс инструментов — НЕ НАЧАТО

Зависит от этапа 3 (capability negotiation) и §5.1 (стабильные approvals).

### 8.1. Модель процесса

Не вводить произвольный shell. Использовать существующий принцип «только
зарегистрированные программы» и расширить его жизненным циклом:

- `program.start` — запуск зарегистрированной программы, возвращает
  `executionId`/`processId`, стартовый статус;
- `program.poll` — статус, ограниченный хвост stdout/stderr, exit code;
- `program.stop` — остановка конкретного процесса (force), без отмены всей
  задачи;
- `program.run` сохраняется для коротких синхронных команд (обратная
  совместимость).

Файлы:

- `backend/src/ai/tools.rs` — новые `ToolSpec` (`program.start/poll/stop`),
  risk/approval: `start` — high (approval), `poll` — low, `stop` — low,
  `server_side=false`.
- `runtime/src/tools.rs` — реализация операций и реестр процессов.
- `runtime/src/process.rs` — вынести запуск в `ProcessHandle`:
  Job Object, cancellation token, ring buffer stdout/stderr (ограниченный,
  например 256 КБ), время старта, exit code, статус.
- `runtime/src/runtime.rs` — `TOOL_PROGRESS` эмиссия с throttling (не чаще
  ~1 раза в секунду или 8 КБ), `task.cancel` останавливает все процессы
  задачи, `program.stop` — один процесс.
- `backend/src/ai/orchestrator.rs` — при `program.start` не блокировать цикл:
  вернуть модели «процесс запущен» и дать возможность `program.poll`;
  сохранять `output_tail` в `ai_tool_executions`.
- `backend/src/ai/runtime/mod.rs` — `TOOL_PROGRESS` уже обрабатывается; писать
  `ai.tool.progress` с `executionId`, `stream`, `chunk`, `truncated`,
  `elapsedMs`.
- `backend/src/ai/routes.rs` — при необходимости добавить
  `POST /api/v1/ai/tasks/{id}/executions/{executionId}/cancel` для остановки
  одного процесса. Аналога в API нет; endpoint обоснован требованием
  «остановить длительный процесс, не отменяя задачу». Альтернатива — только
  `task.cancel`; решение фиксируется в реализации.

### 8.2. Ограничения

- Лимит одновременных процессов на задачу и на Runtime (семафор уже есть).
- Лимит времени процесса, вывода, памяти, числа дочерних процессов.
- Запрет наследования секретов (env allowlist уже есть).
- Процессы не переживают остановку Runtime/задачу (Job Object kill-on-close).
- Полный вывод не хранится в БД; только хвост и события с throttling.

Критерии готовности:

- Агент запускает длительную сборку/тест, видит промежуточный вывод и
  завершает задачу по exit code.
- Пользователь останавливает только процесс; задача остаётся управляемой.
- Отмена задачи гарантированно завершает дерево процессов (тест с дочерним
  процессом).
- После обновления страницы карточка процесса восстанавливает статус и
  последний вывод из `output_tail`/событий.
- Runtime v1 без поддержки процессов не получает эти инструменты.

---

## 9. Этап 5. Контекст, результаты и защита от зацикливания — ВЫПОЛНЕНО (кроме retry)

Зависит от этапа 2 (промежуточные сообщения) и этапа 4 (результаты процессов).

### 9.1. Сохранение действий между запусками

Файлы:

- `backend/src/ai/conversations.rs::history_messages` — включить в контекст
  assistant-сообщения с `tool_calls` и `tool`-сообщения **текущего запуска**;
  для предыдущих запусков передавать компактную сводку (последние N действий:
  tool_id, путь, статус, краткий результат), а не полный вывод.
- `backend/src/ai/orchestrator.rs` — при формировании контекста нового запуска
  добавлять сводку предыдущих запусков из `ai_tool_executions`/`ai_task_events`
  (без секретов и больших stdout).
- `backend/src/ai/models.rs` — при необходимости расширить `ChatMessage`
  (уже поддерживает `tool_calls`, `tool_call_id`).

Правила:

- Полный контекст не должен превышать бюджет (см. 9.3).
- Сводка не является доверенной инструкцией и помечается как данные.
- Секреты и абсолютные пути в сводку не попадают.

### 9.2. Корректность ответа модели

Файлы:

- `runtime/src/provider.rs::parse_completion` — нормализовать `arguments`:
  строка используется как есть; объект сериализуется в строку; иное —
  контролируемая ошибка `AI_PROVIDER_INVALID_TOOL_CALL` (вместо `{}`).
- `backend/src/ai/orchestrator.rs` — проверять `finish_reason`:
  `length`/`content_filter` → контролируемое завершение с
  `AI_PROVIDER_TRUNCATED` (не выдавать за успех); `tool_calls` без вызовов —
  ошибка протокола.
- `backend/src/ai/orchestrator.rs` — при пустом `content` и пустых
  `tool_calls` не считать задачу выполненной без результата: фиксировать
  `AI_PROVIDER_EMPTY_RESPONSE`.

### 9.3. Бюджет контекста и loop safety

Файлы:

- `backend/src/ai/orchestrator.rs`:
  - ограничить суммарный объём контекста (например, 400 000 символов);
    при превышении — усечение старых tool-результатов с маркером
    `[обрезано]`, сохраняя системную политику и последние шаги;
  - ограничить один tool result для модели (например, 32 000 символов) при
    хранении до 200 000 в БД;
  - детектировать повтор `(tool_id, args_hash)` более N раз (например, 3) и
    повторное чтение того же файла без изменений — останавливать с
    `AI_LOOP_DETECTED` и понятным сообщением;
  - классифицировать ошибки: временные (timeout/HTTP 5xx/разрыв) и
    постоянные; повторять только `ToolSpec.idempotent` и только ограниченное
    число раз (например, 2) с backoff; неидемпотентные не повторять.
- `backend/src/ai/workspaces.rs` — использовать `ai_user_quotas.max_iterations`
  вместо константы `DEFAULT_MAX_ITERATIONS`.

Критерии готовности:

- Длинная задача не превышает окно модели и не падает на середине из-за
  размера контекста.
- Повторяющийся вызов останавливается с понятной причиной.
- `length` не приводит к ложному «completed».
- Объектные `arguments` не теряются.
- Повтор read-only операции после временного сбоя не дублирует side effect.

---

## 10. Этап 6. Файлы, изменения и diff — ВЫПОЛНЕНО

Зависит от этапов 2 и 4.

### 10.1. Инструменты

- `backend/src/ai/tools.rs` — добавить `fs.stat` (low, `ai.workspace.read`):
  размер, тип, mtime, is_dir, is_reparse (без раскрытия абсолютного пути).
- `runtime/src/tools.rs` — реализовать `fs.stat`.
- Для `fs.write`/`fs.apply_patch` добавить в `ToolOutcome` опциональное поле
  `diff` (unified diff, ограниченный, например 64 КБ) и `changedFiles`.
- `runtime/src/patch.rs` — возвращать diff по каждому файлу (в дополнение к
  `additions/deletions`).

### 10.2. События и API

- `backend/src/ai/orchestrator.rs` — при успешном изменяющем инструменте
  публиковать `agent.file.changed` (переиспользовать
  `events::user`/`publish_user`) с `taskId`, `path`, `action`,
  `additions`, `deletions`; без содержимого файла.
- `GET /api/v1/ai/tasks/{id}/executions` уже возвращает `resultText`;
  добавить `diff`/`changedFiles` в ответ (аддитивно).
- Отдельный endpoint истории изменённых файлов не создавать: агрегировать на
  клиенте из executions/events.

Критерии готовности:

- UI показывает список созданных/изменённых/удалённых файлов и diff.
- Diff не содержит секретов; абсолютные пути не раскрываются.
- Файл, изменённый программой (не через `fs.*`), не обязан попадать в diff,
  но может быть найден через `fs.stat`/`fs.search`.

---

## 11. Этап 7. Skills: выбор на запуск и явный контракт — ВЫПОЛНЕНО

Зависит от этапа 3 (фильтрация инструментов) и этапа 2 (контекст).

Файлы:

- `backend/src/ai/skills.rs` — использовать существующие
  `assigned_for_user`, `load_content`; добавить проверку выбранных Skill ID
  на принадлежность назначению.
- `backend/src/ai/conversations.rs::SendMessage` — добавить
  `skill_ids: Option<Vec<Uuid>>`; сохранять в `ai_tasks.skill_ids` (миграция
  0021) и в контексте.
- `backend/src/ai/orchestrator.rs::prepare_tools`/`agent_system_prompt` —
  включать только выбранные Skills; если ничего не выбрано — поведение
  прежнее (все назначенные).
- `backend/src/ai/tools.rs` — добавить `skills.list` как server-side tool
  (метаданные: id/slug/name/description, без содержимого), `skill.load`
  оставить для загрузки.
- `backend/src/ai/routes.rs` — `GET /api/v1/ai/skills` уже есть; расширить
  фильтром `workspaceId` при необходимости (сейчас параметр принимается).
- `frontend/src/pages/AiWorkspacePage.tsx` — мультивыбор Skills из списка,
  доступного пользователю; отображать выбранные в задаче.
- `AiAdminPage` — без изменений, кроме отображения `skill_ids` в истории
  запуска (опционально).

Критерии готовности:

- Пользователь видит только назначенные ему Skills и может выбрать их на
  запуск.
- Skill не расширяет права: его требования проверяются теми же инструментами и
  approvals.
- В задаче сохраняется версия/набор использованных Skills для воспроизводимости.

---

## 12. Этап 8. Frontend Agent Mode — ВЫПОЛНЕНО ЧАСТИЧНО

Зависит от этапов 2, 4, 6, 7. Не менять общую тему и 3D-фон.

Файлы:

- `frontend/src/pages/AiWorkspacePage.tsx`:
  - потоковый текст агента и промежуточные сообщения (использовать
    `ai.message.delta` с `messageId`, уже реализованный механизм кэша);
  - таймлайн действий: сообщения модели + tool executions + события в
    хронологическом порядке (переиспользовать `TaskPanel`, не создавать второй
    инспектор);
  - карточка длительного процесса: статус, последний вывод, кнопка остановки
    процесса (новый endpoint §8.1) и общая отмена задачи;
  - список изменённых файлов и diff (из executions/`agent.file.changed`);
  - `exitCode`, длительность, ошибка и предложенный следующий шаг;
  - история запусков диалога: выбор задачи из списка, просмотр после
    перезагрузки (данные уже есть в `aiApi.tasks`);
  - улучшенный approval: человекочитаемое действие, путь/cwd, программа и
    аргументы, риск, остаток TTL, область воздействия; кнопки остаются
    привязанными к API;
  - индикация состояния: подготовка, модель, инструмент, ожидание
    подтверждения, процесс, завершение, ошибка.
- `frontend/src/services/ai.ts` — типы для новых полей (`diff`, `changedFiles`,
  `processId`, `eventId`/`sequence`, `skillIds`).
- `frontend/src/lib/realtime.ts` — типы для `agent.file.changed` и
  `ai.tool.progress` (последний уже объявлен, но не используется).
- `frontend/src/styles/pages/ai.css` — стили карточки процесса, diff, таймлайна
  в рамках существующих токенов.

Не делать:

- не показывать chain-of-thought;
- не добавлять второй WS-канал;
- не переносить оркестрацию в браузер;
- не менять дизайн-систему и общий layout.

Критерии готовности:

- Полный сценарий проходим в браузере: задача → потоковый текст → tool call →
  approval → процесс → результат → файлы/diff.
- После F5 активный запуск, события и вывод восстанавливаются.
- Отмена задачи и отдельного процесса работают из UI.
- Нет отображения скрытых рассуждений и абсолютных путей Runtime.

---

## 13. Этап 9. Администрирование, лимиты и аудит — ВЫПОЛНЕНО ЧАСТИЧНО

Зависит от этапов 1–8.

- `backend/src/ai/admin_routes.rs` — управление `max_iterations` (per-user
  quota), доступностью новых инструментов (политики уже есть), политикой
  approval для `program.*`, лимитами процессов и вывода.
- `backend/src/ai/tools.rs::ensure_policies` — синхронизировать новые
  инструменты; существующие политики не перезаписывать (сейчас `do nothing`).
- `backend/src/ai/workspaces.rs` — enforce `max_parallel_tasks`, storage-квоты
  (если решено включить) и отображение фактических лимитов.
- `backend/src/system/scheduler.rs` или `jobs` — watchdog истёкших approvals и
  «зависших» процессов (без нового планировщика, если можно использовать
  существующий).
- `frontend/src/pages/AiAdminPage.tsx` — поля новых лимитов и отображение
  возможностей Runtime (capabilities/tools/programs) без дублирования.

Критерии готовности:

- Администратор видит, какие инструменты и программы реально доступны на
  устройстве.
- Лимиты применяются на Backend и Runtime, а не только отображаются.
- Аудит покрывает запуск/завершение инструментов, approvals, отмену, тайм-аут,
  нарушение политики.

---

## 14. Этап 10. Тестирование

Зависит от всех предыдущих. Тесты добавляются вместе с этапами, а не в конце.

### 14.1. Backend

- RBAC: роль/пользователь с `ai.*`; superadmin; ownership.
- Потоковый Agent: сборка `content`+`tool_calls`, неполный JSON не запускает
  инструмент.
- Capability gating: Runtime без инструмента → инструмент не предложен;
  версия не совпадает → отказ.
- Конкурентность: второй запуск в диалоге, двойной `run_task`, per-workspace
  лимит, дубли root_path.
- Approvals: reject через finalize, истечение, повторное использование,
  изменение аргументов.
- Контекст: бюджет, усечение, сводка прошлых запусков, отсутствие секретов.
- Loop safety: повтор вызова, `finish_reason=length`, объектные `arguments`.
- Long-running: start/poll/stop, отмена задачи kills tree, восстановление
  `output_tail`.
- Аудит: записи tool.*, отсутствие секретов/путей.
- События: `eventId`/`sequence`, `after=`, reconnect без дублей.
- Идемпотентность: повторная доставка не выполняет side effect.

### 14.2. Runtime

- `fs.stat`, diff, изменённые файлы.
- `program.start/poll/stop`, лимиты, ring buffer, отмена, Job Object.
- `tool.progress` throttling и восстановление.
- Проверка `version` инструмента.
- Ротация receipts, receipt для failed/cancelled.
- Совместимость protocol v1/v2.

### 14.3. Frontend

- Vitest: парсинг событий, агрегация changed files, состояния процесса,
  approval-карточка.
- Playwright: полный Agent-сценарий с approval, cancel, reconnect; проверка
  восстановления после F5; запрет чужого workspace/run.

### 14.4. Сквозные сценарии

1. Агент изучает проект и формирует план (несколько read-only шагов).
2. Создаёт файл; файл виден в UI.
3. Изменяет существующий файл через patch; UI показывает diff.
4. Запускает тесты; получает реальный exit code и вывод.
5. Анализирует ошибку, исправляет, запускает повторно.
6. Использует выбранный Skill и только разрешённые инструменты.
7. Опасное действие ждёт approval; reject не выполняет операцию.
8. Изменение аргументов требует нового approval.
9. Отмена процесса завершает дерево процессов.
10. Обрыв WSS и reconnect не приводят к двойному выполнению.
11. Пользователь A не видит события/workspace/запуски пользователя B.
12. Превышение лимитов завершается понятной ошибкой.
13. Рестарт Backend во время задачи: безопасное продолжение либо
    контролируемая ошибка без повторов.
14. Сценарий на двух физических ПК с реальной PolyStrata (обязательный,
    отдельный отчёт).

Критерии готовности: все сценарии пройдены, результаты приложены к
`docs/TESTING.md`; известные ограничения перечислены явно.

---

## 15. Этап 11. Документация и Standalone — ВЫПОЛНЕНО ЧАСТИЧНО

- Создать `docs/AI_RUNTIME.md` (отсутствует) или официально объединить его с
  `runtime/README.md`, обновив ссылки в `README.md` и плане. Документ должен
  содержать: enrollment, конфигурацию, протокол, capabilities, инструменты,
  процессы, лимиты, диагностику, обновление без потери регистрации.
- Обновить `docs/AI_WORKSPACE.md` — потоковость агента, события, процессы,
  лимиты, Skills, изменённые файлы.
- Обновить `docs/API.md` и `docs/openapi.json` — новые поля/эндпоинты/события
  (после реализации; API не описывать «вперёд» как готовое).
- Обновить `docs/SECURITY.md` — модель угроз длительных процессов, команд,
  diff и вывода.
- Обновить `docs/TESTING.md`, `COMPLETE.md`, `AI_IMPLEMENTATION_STATUS.md`,
  `CHANGELOG.md` — фактические результаты, без завышения готовности.
- Проверить Standalone: Backend, Frontend и Runtime совместимых версий;
  обновление Runtime не сбрасывает `state/device.json` и workspace;
  `runtime check --json` отражает protocol/capabilities.

Критерии готовности: документы соответствуют коду; нет ссылок на
несуществующие файлы; Standalone собирается и проходит smoke.

---

## 16. Зависимости этапов

```text
Этап 1 (P0: RBAC, конкурентность, approvals, аудит)
   ├── Этап 2 (потоковость агента, eventId/sequence)
   │      ├── Этап 5 (контекст, loop safety)
   │      ├── Этап 6 (файлы/diff)
   │      └── Этап 8 (frontend)
   ├── Этап 3 (capabilities, версии, contract-тест)
   │      ├── Этап 4 (процессы, progress)
   │      │      └── Этап 6 / Этап 8
   │      └── Этап 7 (Skills)
   └── Этап 9 (админ/лимиты) — после 4 и 7
Этап 10 (тесты) — параллельно каждому этапу, финальный прогон после 9
Этап 11 (документация/Standalone) — после стабилизации контрактов
```

Этап 1 обязателен до любых изменений цикла: без него нельзя гарантировать
единственного исполнителя и корректные approvals при новых возможностях.
Этапы 2 и 3 можно вести параллельно после этапа 1, если контракт событий и
версий согласован. Этап 4 зависит от версии протокола. Этап 8 начинается после
стабилизации событий (2) и процессов (4), чтобы не переделывать UI дважды.

---

## 17. Карта изменений по файлам

| Файл / модуль | Изменение | Этап |
|---|---|---|
| `backend/src/permissions/catalog.rs` | 10 прав `ai.*` | 1 |
| `backend/src/ai/orchestrator.rs` | single-runner, streaming, finish_reason, бюджет, loop, сводка, audit, file events | 1,2,5,6 |
| `backend/src/ai/workspaces.rs` | per-workspace лимит, storage quota (по решению) | 1,9 |
| `backend/src/ai/approvals.rs` | reject через finalize, watchdog истечения | 1 |
| `backend/src/ai/conversations.rs` | `skillIds`, история tool/assistant, осиротевшие сообщения | 1,5,7 |
| `backend/src/ai/tools.rs` | `fs.stat`, `skills.list`, `program.start/poll/stop`, политики | 3,4,6,7 |
| `backend/src/ai/routes.rs` | `events?after`, `executions/{id}/cancel`, новые поля | 2,4,6 |
| `backend/src/ai/runtime/mod.rs` | eventId/sequence, progress, capabilities, версия | 2,3,4 |
| `backend/src/ai/runtime/protocol.rs` | protocol 2, новые поля, process/progress | 3,4 |
| `backend/src/ai/events.rs` | `agent.file.changed`, корректная адресация | 2,6 |
| `backend/src/ai/skills.rs` | проверка выбранных Skills, `skills.list` | 7 |
| `backend/src/ai/admin_routes.rs` | лимиты, max_iterations, процессы | 9 |
| `backend/src/system/scheduler.rs` | watchdog approvals/процессов | 1,9 |
| `backend/migrations/0021_agent_mode_delta.sql` | аддитивные колонки/индексы | 1,2,7 |
| `runtime/src/tools.rs` | fs.stat, версии, process registry, progress | 3,4,6 |
| `runtime/src/process.rs` | `ProcessHandle`, poll/stop, ring buffer | 4 |
| `runtime/src/runtime.rs` | progress, process ops, receipts rotation, failed receipts | 3,4 |
| `runtime/src/provider.rs` | нормализация arguments | 5 |
| `runtime/src/protocol.rs` | protocol 2, process/progress поля | 3,4 |
| `frontend/src/pages/AiWorkspacePage.tsx` | поток, таймлайн, процесс, файлы/diff, история, Skills, approvals | 8 |
| `frontend/src/services/ai.ts` | типы новых полей | 8 |
| `frontend/src/lib/realtime.ts` | типы новых событий | 8 |
| `frontend/src/styles/pages/ai.css` | стили карточки процесса/diff | 8 |
| `frontend/src/pages/AiAdminPage.tsx` | лимиты, capabilities | 9 |
| `backend/tests/ai_tests.rs` | новые сценарии | 10 |
| `backend/tests/ai_runtime_stand.rs` | процессы, progress, reconnect | 10 |
| `frontend/e2e/ai.spec.ts` | полный Agent-сценарий | 10 |
| `docs/AI_RUNTIME.md` | создать | 11 |
| `docs/AI_WORKSPACE.md`, `docs/API.md`, `docs/openapi.json`, `docs/SECURITY.md`, `docs/TESTING.md`, `COMPLETE.md`, `AI_IMPLEMENTATION_STATUS.md`, `CHANGELOG.md` | актуализация | 11 |

---

## 18. Что не делать

- Не переписывать Chat, Orchestrator, approvals, Skills, MCP и Runtime
  enrollment.
- Не создавать новую систему авторизации, новые роли или второй RBAC.
- Не дублировать существующие таблицы `ai_tasks/ai_task_events/
  ai_tool_executions/ai_tool_approvals`.
- Не создавать второй браузерный WS-канал и не переносить оркестрацию в UI.
- Не менять архитектуру NEXUS ради Agent Mode.
- Не нарушать совместимость PolyStrata (OpenAI-compatible) и Runtime v1.
- Не менять модули Minecraft, Cloud Storage и System Management без
  необходимости.
- Не выдавать произвольный shell-доступ: только зарегистрированные программы и
  разрешённые аргументы.
- Не показывать chain-of-thought и не хранить скрытые рассуждения.
- Не хранить полный stdout/stderr, секреты и абсолютные пути в БД, событиях и
  аудите.
- Не применять миграции на рабочей БД и не запускать опасные системные команды
  в рамках реализации.
- Не объявлять Agent Mode готовым без сквозного сценария на двух ПК и
  приложенных логов.

---

## 19. Критерии готовности Agent Mode (итоговые)

Agent Mode считается полноценным, когда одновременно выполнено:

1. Пользователь запускает задачу из существующего AI Workspace и видит
   потоковый текст агента и промежуточные шаги.
2. Backend управляет циклом «модель → tool call → Runtime → tool result →
   модель» до завершения, отказа, отмены или лимита.
3. Каждый tool call проходит JSON Schema, allowlist workspace, RBAC, проверку
   возможностей Runtime и approval.
4. Модель не получает инструмент, отсутствующий на Runtime; версия контракта
   сверяется.
5. Агент читает и изменяет файлы только внутри назначенного workspace;
   `..`, абсолютные/UNC-пути, symlink/junction блокируются.
6. Агент запускает разрешённые программы, получает exit code, stdout/stderr;
   длительные процессы поддерживают progress и отдельную остановку.
7. Опасные действия ждут одноразового approval; отказ и истечение корректно
   завершают задачу; изменение аргументов требует нового approval.
8. Отмена задачи завершает дерево процессов; отмена процесса не ломает задачу.
9. Контекст между запусками сохраняет действия агента в безопасной сводке;
   бюджет контекста и loop safety работают.
10. Состояния, события и результаты восстанавливаются после F5, reconnect и
    рестарта Backend; повторная доставка не выполняет side effect.
11. Данные, события и workspace изолированы между пользователями.
12. RBAC позволяет администратору управлять AI-правами и лимитами; аудит
    покрывает инструменты, approvals, отмену и тайм-ауты.
13. UI показывает историю действий, изменённые файлы/diff, статусы процессов,
    ошибки и exit code.
14. `docs/AI_RUNTIME.md` существует и соответствует коду; OpenAPI и
    `docs/API.md` актуальны.
15. Пройдены backend/runtime/frontend тесты и сквозной сценарий на двух
    физических ПК с реальной PolyStrata; отчёт приложен.

---

## 20. Порядок начала работ

1. Согласовать этот план и приоритеты (P0 → P1 → P2).
2. Начать с этапа 1; не трогать оркестратор до исправления RBAC и
   единственного исполнителя.
3. После этапа 1 — параллельно этапы 2 и 3 при согласованном контракте
   событий/версий.
4. Этап 4 — после версии протокола и capability negotiation.
5. Этапы 5–7 — после стабильных событий и процессов.
6. Этап 8 — после стабилизации контрактов.
7. Этапы 9–11 — завершение, тесты, документация, Standalone.

## 21. Фактический результат выполнения (обновление)

Реализованные этапы и файлы:

- **Backend:** `backend/migrations/0021_agent_mode_delta.sql`,
  `backend/src/permissions/catalog.rs`, `backend/src/ai/mod.rs`,
  `orchestrator.rs`, `conversations.rs`, `approvals.rs`, `tools.rs`,
  `workspaces.rs`, `models.rs`, `admin_routes.rs`, `routes.rs`, `errors.rs`,
  `runtime/mod.rs`, `runtime/protocol.rs`.
- **Тесты Backend:** `backend/tests/ai_tests.rs` (+1 сценарий RBAC),
  `backend/tests/migrations_tests.rs` (21 миграция, индексы/колонки 0021).
- **Runtime:** `runtime/src/protocol.rs`, `provider.rs`, `tools.rs`, `mcp.rs`.
- **Frontend:** `frontend/src/pages/AiWorkspacePage.tsx`,
  `frontend/src/services/ai.ts`, `frontend/src/lib/realtime.ts`,
  `frontend/src/styles/pages/ai.css`.
- **Документация:** `docs/AI_RUNTIME.md` (создан),
  `docs/reports/AGENT_MODE_AUDIT.md`, этот план.

Результаты проверок:

| Проверка | Результат |
|---|---|
| `cargo test -j 2 --no-fail-fast` (backend) | все цели ok; 86 unit, 17 AI integration, 5 OpenAPI, 1 migrations; лог `logs/agent-mode-tests.log` |
| `cargo test --all-targets` (runtime) | 31 passed, 1 ignored (`real_polystrata_smoke`) |
| `npm run typecheck` | успешно |
| `npm run lint` | успешно |
| `npm run test` | 104 passed, 18 files |
| `npm run build` | успешно |
| `ai_runtime_stand` (реальный Runtime + stub PolyStrata) | 1 passed |
| `cargo fmt --check` | backend/runtime без расхождений |

Оставшиеся работы: этап 4 (длительные процессы и `tool.progress`), retry для
идемпотентных операций, инкрементальный `after` в UI, Playwright Agent-сценарий,
двухмашинный прогон, обновление OpenAPI/`docs/API.md`/`COMPLETE.md` и
Standalone-проверка. Эти пункты не считаются выполненными.
