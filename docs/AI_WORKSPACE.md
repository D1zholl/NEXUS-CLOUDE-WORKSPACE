# NEXUS AI Workspace

Единый документ по AI Chat, AI Agent, NEXUS AI Runtime, MCP и административному
управлению. Дополняет [`docs/ARCHITECTURE.md`](ARCHITECTURE.md),
[`docs/SECURITY.md`](SECURITY.md), [`docs/TESTING.md`](TESTING.md) и
[`docs/API.md`](API.md).

## 1. Состав

| Компонент | Где работает | Ответственность |
|---|---|---|
| `backend/src/ai/` | основной сервер NEXUS | диалоги, история, RBAC, tasks, approvals, Skills, MCP registry, политики, аудит, события, оркестрация |
| `runtime/` (`nexus-ai-runtime.exe`) | второй Windows-компьютер | исходящий WSS, локальная PolyStrata, tool execution, MCP client, workspace isolation, network isolation, лимиты, процессы |
| PolyStrata | компьютер Runtime, обычно `127.0.0.1:8080/v1` | генерация текста и tool-call предложений; не является источником прав |
| PostgreSQL | основной сервер | источник истины для conversations/messages/tasks/executions/approvals/receipts |
| `/api/v1/ws` | браузер | адресные уведомления; не источник истины |

Инварианты:

- Chat request **никогда** не содержит `tools`.
- Agent request содержит tools, разрешённые workspace allowlist и включённые
  политикой; RBAC пользователя, назначение MCP и approval повторно
  проверяются перед каждым dispatch.
- LLM tool call — предложение, а не разрешение.
- Без записи в allowlist, без permission, без workspace assignment, без
  совпавшего approval args_hash — выполнения нет.
- Runtime не доверяет `node_id`/`workspace_root` из payload: node сверяется с
  device token, root — с локальным `allowed_roots`.

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

UI запрашивает последние сообщения (`GET …/messages?tail=true`), поэтому
перезагрузка браузера и reconnect не теряют историю. `POST /conversations/{id}/stop`
отправляет `provider.chat.cancel`; статус `cancelled` фиксируется после
локального прекращения потока, а не как подтверждение остановки Runtime.

## 3. Поток Agent

Публичные статусы: `queued`, `running`, `waiting_approval`, `completed`,
`failed`, `cancelled`. Терминальные состояния не возвращаются в `running`.

```
queued -> running -> completed
                  -> failed
                  -> waiting_approval -> running -> ...
                                      -> cancelled
queued/running/waiting_approval -> cancelling (Runtime ack) -> cancelled
                                                           -> failed (AI_CANCEL_UNCONFIRMED)
```

Цикл:

1. `POST /conversations/{id}/messages` в Agent Mode создаёт `ai_tasks` в `queued`;
   контекст сохраняется до dispatch.
2. Orchestrator строит system policy + историю + разрешённые встроенные tools,
   MCP tools и Skills.
3. `provider.chat.start` (stream=false) через Runtime.
4. Для каждого tool call: JSON Schema → workspace allowlist → MCP assignment →
   risk/approval → quota/limits → execution.
5. Результат возвращается модели как `tool` message; цикл повторяется.
6. Terminal status сохраняется в PostgreSQL и публикуется событием.

Лимиты: `max_iterations` (по умолчанию 10), timeout задачи (`ai_user_quotas` +
workspace), policy timeout tools, `max_tool_calls_per_task` (по умолчанию 40,
настраивается), `max_parallel_tasks` (хранится и отображается; сейчас
применяется только на уровне очереди Runtime, см. ограничения).

### 3.1. Восстановление после рестарта Backend

`recover_interrupted` + `spawn_recovery_tasks` (вызывается в `main.rs::serve`):

- `streaming` сообщения → `failed` (`AI_GENERATION_INTERRUPTED`);
- `queued` задачи → `recovery_state='requeued'` и безопасно запускаются заново;
- `running` без незавершённого tool execution → `resumed`;
- `running` с незавершённым `pending/running` execution → `awaiting_receipt`;
  после reconnect Runtime backend отправляет `execution.status`. Если Runtime
  вернул receipt `completed`, результат восстанавливается и задача продолжается;
  `unknown` → `AI_EXECUTION_UNKNOWN` без повторного side effect; watchdog 5 минут
  завершает задачу контролируемой ошибкой;
- `waiting_approval` сохраняется;
- `cancel_requested` задачи повторно получают `task.cancel`;
- outbox-команды, отправленные до рестарта, помечаются `failed`.

## 4. Approval

Подтверждение требуется, если policy инструмента:

- `always`; или
- `risk` и risk ≥ medium.

Approval содержит `task_id`, `execution_id`, `requester_id`, `tool_id`,
`args_hash` (SHA-256 канонического JSON), точные аргументы, TTL 15 минут.
`POST /approvals/{id}/approve`:

- доступно владельцу или `ai.admin.manage` (требуется `ai.actions.approve`);
- не меняет аргументы;
- атомарно переводит `pending → approved`;
- orchestrator при resume повторно проверяет hash, атомарно потребляет
  (`consumed_at`) и только затем dispatch.

Reject/expiry/consumed → выполнение невозможно; повторное approve → `409`.
Approve/reject поддерживают `Idempotency-Key`.

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
   `runtime.capabilities` (модели, tools, programs, network isolation, PolyStrata).
6. Revoke/rotate/disable закрывают соединение; повторный handshake отклоняется.

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

Типы: `runtime.hello/hello_ack/heartbeat/heartbeat_ack/capabilities/capabilities_ack/error`,
`provider.chat.start/accepted/delta/completed/failed/cancel`, `provider.request/response`,
`tool.execute/accepted/progress/completed/failed`,
`task.cancel/cancel_ack/cancelled`,
`workspace.usage/usage_result`, `execution.status/status_result`,
`mcp.configure/configure_ack/discover/tools`.

Защита от повторов:

- Backend отклоняет кадры Runtime с `sequence` не больше последнего и
  timestamp старше/новее 15 минут (кроме известного `messageId`, для которого
  возвращается receipt).
- Runtime делает то же для команд `sequence`/timestamp; повторный `messageId`
  возвращает сохранённый результат из памяти/`receipts.jsonl` без повторного
  side effect.
- `ai_runtime_commands.message_id` уникален; `ai_tool_executions` уникален по
  `(task_id, tool_call_id)` и `dispatch_message_id`.
- HTTP-операции отправки сообщения, cancel и approve/reject поддерживают
  `Idempotency-Key` через общий `idempotency_keys`.

## 6. Модель данных

Миграции `backend/migrations/0019_ai.sql` и `0020_ai_hardening.sql`:

- `ai_provider_configs` — base URL, encrypted API key, model, generation, node;
- `ai_runtime_nodes`, `ai_runtime_enrollment_codes`, `ai_runtime_commands`,
  `ai_runtime_events`, `ai_runtime_receipts`;
- `ai_workspaces`, `ai_user_quotas`, `ai_workspace_usage_samples`;
- `ai_conversations`, `ai_messages`, `ai_attachments`;
- `ai_tasks`, `ai_task_events`, `ai_tool_executions`, `ai_tool_approvals`;
- `ai_skills`, `ai_skill_files`, `ai_skill_versions`, `ai_skill_version_files`,
  `ai_skill_assignments`;
- `ai_tool_policies`, `ai_programs`;
- `ai_mcp_connections`, `ai_mcp_tools`, `ai_mcp_assignments`.

Секреты: API key и MCP secret — AES-256-GCM (ключ производный от
`NEXUS_SECRET`), device token — SHA-256 hash. Абсолютный `root_path` workspace
возвращается только администратору (`ai.admin.manage`). MCP-секрет не пишется в
outbox БД: сохраняется только безопасная копия без secret.

## 7. HTTP API

Пользовательские:

| Метод | Путь |
|---|---|
| GET | `/api/v1/ai/providers` |
| GET | `/api/v1/ai/providers/{id}` |
| GET/POST | `/api/v1/ai/conversations` |
| GET/PATCH/DELETE | `/api/v1/ai/conversations/{id}` |
| GET/POST | `/api/v1/ai/conversations/{id}/messages` (`?tail=true` — последние) |
| POST | `/api/v1/ai/conversations/{id}/messages/{message_id}/regenerate` |
| PATCH | `/api/v1/ai/conversations/{id}/messages/{message_id}` |
| POST | `/api/v1/ai/conversations/{id}/stop` |
| POST | `/api/v1/ai/conversations/{id}/attachments` (multipart, text ≤5 МБ) |
| GET/POST | `/api/v1/ai/tasks` |
| GET | `/api/v1/ai/tasks/{id}` |
| GET | `/api/v1/ai/tasks/{id}/events` |
| POST | `/api/v1/ai/tasks/{id}/cancel` |
| GET | `/api/v1/ai/tasks/{id}/executions` |
| GET | `/api/v1/ai/approvals/pending` |
| POST | `/api/v1/ai/approvals/{id}/approve`, `/reject` |
| GET | `/api/v1/ai/workspaces`, `/workspaces/{id}`, `/workspaces/{id}/usage` |
| POST | `/api/v1/ai/workspaces/{id}/usage/refresh` |
| GET | `/api/v1/ai/skills`, `/skills/{id}` |
| GET | `/api/v1/ai/tools` |
| POST | `/api/v1/ai/runtime/enroll` |
| GET | `/api/v1/ai/runtime/connect` (WebSocket Runtime) |

Административные (`ai.admin.manage`):

- `/api/v1/ai/admin/overview`, `/logs`;
- `/admin/runtime-nodes…` (list/detail/revoke/disable/enable/rotate-token);
- `/admin/enrollment-codes…`;
- `/admin/providers…` (admin list включает отключённые; create/update/disable/test/models/status);
- `/admin/workspaces…`, `/admin/workspaces/{id}/usage/refresh`;
- `/admin/skills…` (import ZIP, update, enable/disable, delete, assignments,
  `…/versions`, `…/versions/{version}`, `…/versions/{version}/restore`);
- `/admin/tools`, `/admin/tools/{id}`;
- `/admin/programs`, `/admin/programs/{id}` (risk, enabled, networkPolicy, memory);
- `/admin/mcp-connections…` (CRUD, discover, tools, assignments);
- `/admin/mcp-tools/{id}` (risk/approvalPolicy/enabled);
- `/admin/quotas`, `/admin/quotas/{user_id}`.

Все ответы — `{success,data}` / `{success:false,error,request_id}`. OpenAPI 3.1
доступен на `/api/v1/openapi.json`; Swagger — при `NEXUS_ENABLE_DOCS=true`.

## 8. RBAC

Добавлены права:

`ai.access`, `ai.chat.use`, `ai.agent.use`, `ai.workspace.read`,
`ai.workspace.write`, `ai.skills.use`, `ai.tools.use`, `ai.tasks.cancel`,
`ai.actions.approve`, `ai.admin.manage`.

Администратор получает все; роль User — все, кроме `ai.admin.manage`.
Проверка выполняется в каждом handler, а для tools — повторно перед dispatch,
при MCP authorization и на Runtime (allowlist/workspace).

## 9. WebSocket события

`ai.message.started/delta/completed/failed`,
`ai.task.queued/started/progress/completed/failed/cancelled`,
`ai.tool.started/progress/waiting_approval/completed/failed`,
`ai.runtime.connected/disconnected`.

Payload содержит минимально необходимые ID/статусы; аргументы инструментов
редактируются (секреты → `***`), абсолютные пути Runtime не передаются.
Frontend при reconnect перечитывает состояние из HTTP API.

## 10. Skills, версии и восстановление

ZIP (`SKILL.md` в корне или единственной папке) → валидация:

- 10 МБ архив / 20 МБ распакованный / 500 файлов / 2 МБ файл;
- запрет `..`, абсолютных путей, symlink, zip-slip;
- checksum, версия, метаданные в PostgreSQL, файлы в `ai_skill_files`.

Каждый импорт создаёт полный snapshot **всех** файлов в
`ai_skill_version_files` (не только `SKILL.md`). API:

- `GET /admin/skills/{id}/versions` — история;
- `GET /admin/skills/{id}/versions/{version}` — файлы версии (text/Base64);
- `POST /admin/skills/{id}/versions/{version}/restore` — восстановить версию как
  новую (история не перезаписывается).

Skill не меняет system policy/tools/approvals. Агент получает только
метаданные назначенных Skills и загружает содержимое через server-side tool
`skill.load` (результат помечен `untrusted`). Скрипты не исполняются
автоматически.

## 11. MCP

MCP выключен по умолчанию. Поток:

1. Администратор создаёт `ai_mcp_connections` для конкретного Runtime; endpoint
   проверяется на SSRF (loopback/private/link-local/metadata запрещены без
   `allowPrivateNetwork=true`).
2. Backend отправляет `mcp.configure` по WSS; секрет передаётся только по
   защищённому каналу и не сохраняется в outbox.
3. `mcp.discover` → Runtime выполняет JSON-RPC `initialize`/`tools/list`
   (Streamable HTTP, поддержка JSON и SSE-ответов) с повторной SSRF-проверкой.
4. Backend сохраняет snapshot в `ai_mcp_tools`, создаёт `ai_tool_policies`
   (`approval_policy='always'` по умолчанию).
5. Администратор назначает соединение пользователю/workspace/роли
   (`ai_mcp_assignments`) и добавляет `mcp.<connection>.<tool>` в
   `allowed_tools` workspace.
6. Агент вызывает tool через Runtime; результат считается недоверенным и
   ограничивается по размеру.

Ограничение: поддерживается MCP Streamable HTTP (`http`/`https`). stdio-MCP
транспорт не реализован.

## 12. Безопасность

- Cookie-сессия браузера не используется Runtime.
- Входящие порты Runtime не открываются.
- Path traversal / absolute / UNC / `..` / symlink / junction / reparse point
  блокируются; containment проверяется непосредственно перед операцией.
- JSON Schema, allowlist, risk, approval, quotas, timeouts.
- Device token / API key / MCP secret не попадают в UI, audit, logs, outbox.
- Prompt injection из файлов/Skills/tool output/MCP не расширяет права: policy
  отделена от untrusted context, каждое действие проверяется заново.
- Approvals одноразовые, с TTL и args hash.
- Quotas workspace: bytes/files/max file проверяются до записи под
  per-workspace mutex; `fs.apply_patch` считает итоговые размеры всех файлов до
  записи; `program.run` проверяет usage до запуска и мониторит его во время
  выполнения, завершая дерево процессов при превышении.
- `program.run`: только зарегистрированные программы, без shell, allowlist
  аргументов, Job Object (kill-on-close), лимит памяти.
- Сеть программ: `network_policy='deny'` по умолчанию. Runtime временно
  добавляет Windows Firewall outbound block для executable и удаляет после
  завершения. Без прав администратора запуск отклоняется (fail closed), а не
  выполняется без изоляции. Ограничение: правило адресуется executable, поэтому
  на время запуска блокируется сеть всех процессов этого executable.
- MCP: SSRF-проверка на backend и Runtime, redirects запрещены, результаты
  недоверенные, отдельные assignments и approval.

## 13. Ограничения текущей версии

1. **Второй компьютер**: реальный сценарий NEXUS ↔ Runtime ↔ PolyStrata на двух
   физических машинах не выполнен в этой среде. Подготовлен и пройден
   автоматизированный стенд на одной машине (реальный процесс Runtime, WSS,
   streaming, tools, usage, cancel, reconnect, revoke). См.
   `docs/AI_PRODUCTION_AUDIT.md`.
2. **PolyStrata**: на машине аудита отсутствует; `real_polystrata_smoke`
   (`#[ignore]`) и `scripts/ai-polystrata-smoke.ps1` требуют реальный endpoint.
3. **max_parallel_tasks** и пользовательские storage-quota
   (`max_storage_bytes/max_files/max_file_bytes`) хранятся и отображаются, но
   admission control использует `max_concurrent_tasks`, `max_tasks_per_day`,
   `max_tool_calls_per_task`, `max_task_seconds` и workspace-квоты Runtime.
4. **Network isolation** требует прав администратора и блокирует executable
   целиком на время запуска. Полноценный AppContainer/process-level egress не
   реализован.
5. **MCP stdio** не реализован; только Streamable HTTP.
6. **Quota concurrency** между несколькими Runtime-инстансами одного workspace
   не поддерживается; не создавайте такую конфигурацию.
7. **Заголовок диалога** строится детерминированно из первого сообщения (без
   отдельного вызова модели) — безопасный fallback по ТЗ.

## 14. Запуск

Backend:

```powershell
cd backend
cargo run -- migrate
cargo run -- serve
```

Frontend:

```powershell
cd frontend
npm ci
npm run build
```

Runtime — см. [`runtime/README.md`](../runtime/README.md). Кратко:

```powershell
cd runtime
cargo build --release
.\target\release\nexus-ai-runtime.exe --config C:\NexusAI\runtime.toml check --json
.\target\release\nexus-ai-runtime.exe --config C:\NexusAI\runtime.toml enroll --code nxe_...
.\install-service.ps1
```

`runtime check` проверяет: allowed roots, device token, доступность NEXUS
`/api/v1/health`, PolyStrata `/health`+`/status`+`/models`, наличие программ и
доступность сетевой изоляции.

## 15. Проверка и диагностика

```powershell
cd backend
cargo check --all-targets
cargo test -j 1
cd ..\frontend
npm run typecheck
npm run test
npm run lint
npm run build
cd ..\runtime
cargo test --all-targets
```

Автоматизированный стенд Backend+Runtime:

```powershell
.\scripts\ai-runtime-stand.ps1
```

Полный сценарий для второго компьютера:

```powershell
.\scripts\ai-two-machine-check.ps1 -BackendUrl https://nexus.example `
  -EnrollmentCode nxe_... -PolyStrataUrl http://127.0.0.1:8080/v1
```

Troubleshooting:

- `AI_RUNTIME_OFFLINE` — нет WSS: проверьте `server.url`, firewall, revoke/rotate.
- `AI_PROVIDER_UNAVAILABLE` — `/health`/`/models` не отвечают: проверьте
  PolyStrata и base URL.
- `AI_USAGE_TIMEOUT` — Runtime не ответил на `workspace.usage`: проверьте
  доступность root и лимиты обхода.
- `AI_CANCEL_UNCONFIRMED` — Runtime не подтвердил остановку за 75 с: проверьте
  Runtime и дочерние процессы вручную; задача не считается отменённой.
- `AI_EXECUTION_UNKNOWN` — результат операции неизвестен после разрыва/рестарта:
  не повторяйте автоматически, проверьте файл/процесс.
- MCP `error`/SSRF — включите `allowPrivateNetwork` только для доверенного
  локального сервера; проверьте DNS и endpoint.

## 16. Backup и restore

В PostgreSQL входят: AI-таблицы 0019/0020, включая `ai_skill_version_files`,
`ai_runtime_commands`, `ai_runtime_receipts`, `ai_mcp_*`. Для Skills/artifacts
отдельных файлов на диске нет — всё в БД. На Runtime сохраняйте:
`state/device.json` (секрет), `state/receipts.jsonl`, `runtime.toml`.
`NEXUS_SECRET` должен совпадать с моментом сохранения API keys/MCP secrets:
после смены секрета ключи нужно ввести заново. При восстановлении на новом
Runtime устройство регистрируется заново (старый token отзывается).
