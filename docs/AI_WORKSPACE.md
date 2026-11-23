# NEXUS AI Workspace

Единый документ по AI Chat, AI Agent, NEXUS AI Runtime и административному
управлению. Дополняет [`docs/ARCHITECTURE.md`](ARCHITECTURE.md),
[`docs/SECURITY.md`](SECURITY.md) и [`docs/API.md`](API.md).

## 1. Состав

| Компонент | Где работает | Ответственность |
|---|---|---|
| `backend/src/ai/` | основной сервер NEXUS | диалоги, история, RBAC, tasks, approvals, Skills metadata, политики, аудит, события, оркестрация |
| `runtime/` (`nexus-ai-runtime.exe`) | второй Windows-компьютер | исходящий WSS, локальная PolyStrata, tool execution, workspace isolation, лимиты, процессы |
| PolyStrata | компьютер Runtime, `127.0.0.1:8080/v1` | генерация текста и tool-call предложений; не является источником прав |
| PostgreSQL | основной сервер | источник истины для конversations/messages/tasks/executions/approvals |
| `/api/v1/ws` | браузер | адресные уведомления; не источник истины |

Инварианты:

- Chat request **никогда** не содержит `tools`.
- Agent request содержит только tools, разрешённые одновременно пользователю,
  workspace, Runtime и политике администратора.
- LLM tool call — предложение, а не разрешение.
- Без записи в allowlist, без permission, без workspace assignment, без
  совпавшего approval args_hash — выполнения нет.

## 2. Поток Chat

1. `POST /api/v1/ai/conversations/{id}/messages`.
2. Backend проверяет `ai.chat.use`, ownership, провайдера, Runtime online.
3. Сохраняет user message; assistant placeholder `streaming`.
4. Отправляет `provider.chat.start` (stream=true, tools=[]).
5. Runtime вызывает PolyStrata `/v1/chat/completions` и присылает
   `provider.chat.delta` / `provider.chat.completed`.
6. Backend публикует `ai.message.delta` владельцу, периодически сохраняет
   накопленный текст, финально сохраняет `completed` и usage.
7. `ai.message.completed` / `ai.message.failed`.

Перезагрузка браузера не теряет историю: UI запрашивает сообщения из API, а не
полагается на WebSocket-дельты. `POST /conversations/{id}/stop` отправляет
`provider.chat.cancel` и помечает сообщение `cancelled`.

## 3. Поток Agent

Публичные статусы: `queued`, `running`, `waiting_approval`, `completed`,
`failed`, `cancelled`. Переходы:

```
queued -> running -> completed
                  -> failed
                  -> waiting_approval -> running -> ...
                                      -> cancelled
queued/running/waiting_approval -> cancelled
```

Цикл:

1. `POST /conversations/{id}/messages` в Agent Mode создаёт `ai_tasks` в `queued`
   (контекст сохраняется до dispatch).
2. Orchestrator строит system policy + историю + разрешённые tools/Skills.
3. `provider.chat.start` (stream=false) через Runtime.
4. Для каждого tool call: JSON Schema → workspace allowlist → policy risk →
   approval → execution.
5. Результат возвращается модели как `tool` message; цикл повторяется.
6. Terminal status сохраняется в PostgreSQL и публикуется событием.

Лимиты: `max_iterations=10`, timeout задачи (`ai_user_quotas` + workspace),
policy timeout tools, 40 tool calls (глобальный предел), `max_parallel_tasks`.

Восстановление после рестарта: recovery помечает `queued/running` задачи
`failed` с `AI_TASK_INTERRUPTED` (side effects не повторяются), а
`waiting_approval` сохраняются и продолжаются после approve. Runtime-команды
из outbox при старте помечаются failed.

## 4. Approval

Подтверждение требуется, если policy инструмента:

- `always`; или
- `risk` и risk ≥ medium.

Approval содержит `task_id`, `execution_id`, `requester_id`, `tool_id`,
`args_hash` (SHA-256 канонического JSON), точные аргументы, TTL 15 минут.
`POST /approvals/{id}/approve`:

- доступно владельцу или `ai.admin.manage`;
- не меняет аргументы;
- атомарно переводит `pending → approved`;
- orchestrator при resume повторно проверяет hash, атомарно потребляет
  (`consumed_at`) и только затем dispatch.

Reject/expiry/consumed → выполнение невозможно; повторное approve → `409`.

## 5. Runtime enrollment и протокол

1. Администратор: `POST /api/v1/ai/admin/enrollment-codes` → одноразовый `nxe_…`
   (TTL 60–3600 с).
2. Runtime: `nexus-ai-runtime enroll --code …` → `POST /api/v1/ai/runtime/enroll`
   (без cookie, CSRF не применяется к этой точке).
3. Backend проверяет hash кода, TTL, module state; создаёт node и device token
   `nxa_…`; в БД только SHA-256 token.
4. Runtime: `GET /api/v1/ai/runtime/connect` с `Authorization: Bearer nxa_…`
   (WSS в production).
5. `runtime.hello` → `runtime.hello_ack`; heartbeat 20 с;
   `runtime.capabilities` (модели, tools, programs, PolyStrata status).
6. Revoke/rotate/disable закрывают соединение.

Конверт:

```json
{
  "protocolVersion": 1,
  "messageId": "uuid",
  "type": "provider.chat.start",
  "timestamp": "2026-11-23T12:00:00Z",
  "nodeId": "uuid",
  "correlationId": "uuid",
  "taskId": "uuid",
  "sequence": 42,
  "payload": {}
}
```

Типы: `runtime.hello/hello_ack/heartbeat/heartbeat_ack/capabilities/capabilities_ack`,
`provider.chat.start/accepted/delta/completed/failed/cancel`,
`provider.request/response`, `tool.execute/accepted/completed/failed`,
`task.cancel/cancelled`, `runtime.error`.

`nodeId` всегда сверяется с аутентифицированным node. Команды сохраняются в
`ai_runtime_commands` (outbox); не отправленные после reconnect повторяются,
секреты (API key) в outbox не пишутся. Runtime дедуплицирует `messageId`
(in-memory + `receipts.jsonl`). Разрыв соединения не перезапускает
неидемпотентные операции.

## 6. Модель данных

Миграция `backend/migrations/0019_ai.sql`:

- `ai_provider_configs` — base URL, encrypted API key, model, generation, node;
- `ai_runtime_nodes`, `ai_runtime_enrollment_codes`, `ai_runtime_commands`,
  `ai_runtime_events`;
- `ai_workspaces`, `ai_user_quotas`;
- `ai_conversations`, `ai_messages`, `ai_attachments`;
- `ai_tasks`, `ai_task_events`, `ai_tool_executions`, `ai_tool_approvals`;
- `ai_skills`, `ai_skill_files`, `ai_skill_versions`, `ai_skill_assignments`;
- `ai_tool_policies`, `ai_programs`.

Секреты: API key — AES-256-GCM (ключ производный от `NEXUS_SECRET`), device
token — SHA-256 hash. Абсолютный `root_path` workspace возвращается только
администратору (`ai.admin.manage`).

## 7. HTTP API

Пользовательские:

| Метод | Путь |
|---|---|
| GET | `/api/v1/ai/providers` |
| GET | `/api/v1/ai/providers/{id}` |
| GET/POST | `/api/v1/ai/conversations` |
| GET/PATCH/DELETE | `/api/v1/ai/conversations/{id}` |
| GET/POST | `/api/v1/ai/conversations/{id}/messages` |
| POST | `/api/v1/ai/conversations/{id}/messages/{message_id}/regenerate` |
| PATCH | `/api/v1/ai/conversations/{id}/messages/{message_id}` |
| POST | `/api/v1/ai/conversations/{id}/stop` |
| POST | `/api/v1/ai/conversations/{id}/attachments` (multipart, text only ≤5 МБ) |
| GET/POST | `/api/v1/ai/tasks` |
| GET | `/api/v1/ai/tasks/{id}` |
| GET | `/api/v1/ai/tasks/{id}/events` |
| POST | `/api/v1/ai/tasks/{id}/cancel` |
| GET | `/api/v1/ai/tasks/{id}/executions` |
| GET | `/api/v1/ai/approvals/pending` |
| POST | `/api/v1/ai/approvals/{id}/approve`, `/reject` |
| GET | `/api/v1/ai/workspaces`, `/workspaces/{id}`, `/workspaces/{id}/usage` |
| GET | `/api/v1/ai/skills`, `/skills/{id}` |
| GET | `/api/v1/ai/tools` |
| POST | `/api/v1/ai/runtime/enroll` |
| GET | `/api/v1/ai/runtime/connect` (WebSocket Runtime) |

Административные (`ai.admin.manage`):

- `/api/v1/ai/admin/overview`, `/logs`;
- `/admin/runtime-nodes…` (list/detail/revoke/disable/enable/rotate-token);
- `/admin/enrollment-codes…`;
- `/admin/providers…` (create/update/disable/test/models/status);
- `/admin/workspaces…`;
- `/admin/skills…` (import ZIP, update, enable/disable, delete, assignments);
- `/admin/tools`, `/admin/tools/{id}`;
- `/admin/programs`, `/admin/programs/{id}`;
- `/admin/quotas`, `/admin/quotas/{user_id}`.

Все ответы — `{success,data}` / `{success:false,error,request_id}`. OpenAPI 3.1
доступен на `/api/v1/openapi.json`; Swagger — при `NEXUS_ENABLE_DOCS=true`.

## 8. RBAC

Добавлены права:

`ai.access`, `ai.chat.use`, `ai.agent.use`, `ai.workspace.read`,
`ai.workspace.write`, `ai.skills.use`, `ai.tools.use`, `ai.tasks.cancel`,
`ai.actions.approve`, `ai.admin.manage`.

Администратор получает все; роль User — все, кроме `ai.admin.manage`.
Проверка выполняется в каждом handler, а для tools — повторно перед dispatch и
на Runtime.

## 9. WebSocket события

`ai.message.started/delta/completed/failed`,
`ai.task.queued/started/progress/completed/failed/cancelled`,
`ai.tool.started/progress/waiting_approval/completed/failed`,
`ai.runtime.connected/disconnected`.

Payload содержит минимально необходимые ID/статусы; аргументы инструментов
редактируются (секреты → `***`), абсолютные пути Runtime не передаются.

## 10. Skills

ZIP (`SKILL.md` в корне или единственной папке) → валидация:

- 10 МБ архив / 20 МБ распакованный / 500 файлов / 2 МБ файл;
- запрет `..`, абсолютных путей, symlink, zip-slip;
- checksum, версия, метаданные в PostgreSQL, файлы в `ai_skill_files`.

Skill не меняет system policy/tools/approvals. Агент получает только
метаданные назначенных Skills и загружает содержимое через server-side tool
`skill.load` (результат помечен `untrusted`). Скрипты не исполняются
автоматически.

## 11. Безопасность

- Cookie-сессия браузера не используется Runtime.
- Входящие порты Runtime не открываются.
- Path traversal / absolute / UNC / `..` / symlink / junction / reparse point
  блокируются; containment проверяется непосредственно перед операцией.
- JSON Schema, allowlist, risk, approval, quotas, timeouts.
- Device token / API key не попадают в UI, audit, logs.
- Prompt injection из файлов/Skills/tool output не расширяет права: policy
  отделена от untrusted context, каждое действие проверяется заново.
- Approvals одноразовые, с TTL и args hash.
- Quotas workspace: bytes/files/max file передаются Runtime и проверяются перед
  записью под per-workspace mutex; квота проверяется до операции, но
  параллельные программы могут обойти подсчёт (см. ограничения).

## 12. Ограничения текущей версии

Честно фиксируются, чтобы не выдавать нереализованное за готовое:

1. **MCP** не реализован: нет реестра соединений и tool discovery. Локальные
   tools защищены полностью.
2. **Сетевая изоляция программ** ограничена allowlist аргументов и запуском без
   shell; egress-фильтрация на уровне Windows Firewall автоматически не
   настраивается.
3. **Workspace usage** (`usageBytes/usageFiles`) обновляется только если Runtime
   научится его репортить; текущая версия показывает `checkedAt=null` и не
   подставляет фиктивные нули.
4. **Cancel running tool** инициируется сразу и отправляет `task.cancel`;
   подтверждение Runtime (`task.cancelled`) обрабатывается, но backend может
   зафиксировать `cancelled` локально раньше сетевого ack. Для строгой
   гарантии «остановлено» проверяйте Runtime log/процессы.
5. **Quota concurrency** защищена mutex внутри одного Runtime; между несколькими
   Runtime-инстансами для одного workspace администратор не должен создавать
   такие конфигурации. `fs.apply_patch` и `program.run` не пересчитывают
   итоговую квоту заранее (patch проверяется пофайлово, программа — policy/timeout).
6. **Backend restart** не продолжает запущенные задачи: они переводятся в
   контролируемую ошибку, чтобы не дублировать side effects.
7. **История версий Skill** хранит `SKILL.md`, checksum и метаданные; полный
   snapshot файлов предыдущей версии не сохраняется.
8. **Заголовок диалога** строится детерминированно из первого сообщения (без
   отдельного вызова модели) — безопасный fallback по ТЗ.
9. **PolyStrata** на дев-машине отсутствует; интеграционные тесты используют
   протокольный стенд. На production выполните `runtime check` против реальной
   PolyStrata.

## 13. Запуск

Backend:

```powershell
cd backend
cargo run -- migrate
cargo run -- serve
```

Frontend:

```powershell
cd frontend
npm install
npm run build
```

Runtime — см. [`runtime/README.md`](../runtime/README.md). Кратко:

```powershell
cd runtime
cargo build --release
.\target\release\nexus-ai-runtime.exe check --config C:\NexusAI\runtime.toml
.\target\release\nexus-ai-runtime.exe enroll --config C:\NexusAI\runtime.toml --code nxe_...
.\install-service.ps1
```

## 14. Проверка

```powershell
cd backend
cargo check --all-targets
cargo test -j 2
cd ..\frontend
npm run typecheck
npm run test
npm run build
npm run lint
cd ..\runtime
cargo test --all-targets
```

Дополнительно: `tests/ai_tests.rs` (реальный PostgreSQL + HTTP + WebSocket
Runtime), `runtime` unit-тесты (traversal, patch, schemas, config), smoke/E2E
скрипты NEXUS.
