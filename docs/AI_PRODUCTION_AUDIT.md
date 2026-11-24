# NEXUS AI Workspace — Production Audit (Phase 9)

Дата: 23 ноября 2026. Аудит проведён по фактическому коду, миграциям,
конфигурации и тестам, а не по предыдущему отчёту. Документ фиксирует, что
проверено, что исправлено, что осталось и при каких условиях система может
считаться Production Ready.

## 0. Итоговый вердикт

**Production Ready не объявляется.** Причина: обязательный сценарий
«NEXUS Backend на компьютере №1 + Runtime и PolyStrata на компьютере №2» не
выполнен на двух физических машинах в этой среде. Всё остальное, что можно
проверить автоматически на одной машине, проверено реальными процессами и
PostgreSQL.

Подготовлен и успешно пройден автоматизированный стенд: отдельный процесс
`nexus-ai-runtime.exe` подключается к реальному backend по WebSocket, проходит
enrollment/heartbeat, получает задания, вызывает OpenAI-compatible PolyStrata
stub, выполняет реальные файловые операции, репортит usage, подтверждает
отмену, переподключается после перезапуска и отзывается. Это **не** замена
проверке второго компьютера.

## 1. Методика

- Прочитаны все файлы `backend/src/ai/**`, `runtime/src/**`, миграции
  0019/0020, frontend AI-страницы/сервисы/события, тесты и документация.
- Сверка с `NEXUS_AI_Workspace_Implementation_Prompt.md` (разделы 5–22).
- Проверки: `cargo check --all-targets`, `cargo test -j 1`, runtime tests,
  `npm run typecheck|lint|test|build`, OpenAPI/migrations tests, automated
  runtime stand, Playwright E2E (см. §8).
- Проверялись реальные механизмы; моки использовались только для PolyStrata
  stub и для эмуляции MCP HTTP-сервера в unit-тесте клиента.

## 2. Состояние по компонентам

### 2.1. AI Backend

| Область | Состояние | Доказательство |
|---|---|---|
| Модуль `ai`, module gate | Реализовано | `backend/src/ai/mod.rs`, `middleware::module_guard`, тест `disabled_ai_module_blocks_api_immediately` |
| Провайдеры, шифрование API key | Реализовано | `providers.rs`, `crypto.rs`, тест `provider_api_key_is_encrypted_and_never_returned` |
| Диалоги, сообщения, поиск, вложения | Реализовано | `conversations.rs`; добавлена выдача последних сообщений `?tail=true` |
| Chat streaming без tools | Реализовано | `conversations.rs::start_chat_generation`; тест `chat_streams_without_tools_and_persists_history` |
| Agent orchestrator | Реализовано и усилено | `orchestrator.rs`; 16 AI integration tests |
| Approvals | Реализовано | `approvals.rs`; approve/reject/expiry/args-hash тесты |
| Skills | Реализовано, расширено | полный snapshot версий, restore, API |
| Workspace/квоты | Реализовано, расширено | usage reporting, quota pre-check patch/program |
| MCP | **Реализовано в Phase 9** | `mcp.rs`, миграция 0020, API, тест discovery/execution |
| Аудит, события, OpenAPI | Реализовано | audit events, realtime, 185 путей / 224 операции |

### 2.2. AI Runtime

| Область | Состояние | Доказательство |
|---|---|---|
| Отдельный Windows executable/service | Реализовано | `runtime/src`, `service.rs`, PowerShell scripts |
| Исходящий WSS, reconnect, heartbeat | Реализовано | `runtime.rs`; automated stand |
| Enrollment/revoke/rotate | Реализовано | `runtime/mod.rs`; тест enrollment single-use/revoke |
| PolyStrata adapter | Реализовано | `/models`, `/health`, `/status`, SSE, tool_calls; runtime tests |
| Filesystem tools | Реализовано | traversal/symlink/reparse containment; `fs.*` |
| Program execution | Усилено | Job Object, memory limit, firewall network isolation |
| MCP client | **Реализовано в Phase 9** | JSON-RPC Streamable HTTP, discovery/call, SSRF |
| Usage reporting | **Реализовано в Phase 9** | `workspace.usage` + usage в `tool.completed` |
| Cancel confirmation | **Исправлено** | `task.cancel_ack` + `task.cancelled{confirmed}` только после остановки |
| Idempotency/replay | Усилено | sequence/timestamp/messageId, receipts |
| Recovery | Усилено | backend recovery + `execution.status` reconciliation |

### 2.3. PolyStrata integration

- Реальный клиент поддерживает `/models`, `/health`, `/status`,
  `/chat/completions`, SSE, usage, несколько `tool_calls`, `tool_call_id`,
  отмену, ошибки HTTP/JSON/timeout.
- В Phase 9 исправлено: общий timeout больше не обрывает длинный SSE-поток;
  тело HTTP-ошибки возвращается в безопасном усечённом виде; отмена
  прерывает и нестриминговый запрос.
- Реальная PolyStrata в среде аудита не обнаружена (порт 8080 занят самим
  NEXUS). `real_polystrata_smoke` остаётся `#[ignore]`.
- Добавлен сценарий `scripts/ai-polystrata-smoke.ps1`.

### 2.4. AI Chat frontend

- Реальный streaming через `ai.message.delta`, fallback polling, Markdown,
  code highlight/copy, вложения, regenerate/edit/stop, approvals, task inspector,
  executions/events, cancel.
- Исправлено: выборка последних сообщений (`tail=true`), сброс вложений при
  смене диалога, reload состояния при reconnect, gate approve/reject по
  `ai.actions.approve`, отображение `cancelStatus`.
- Остаётся ограничение UX: нет выбора старых задач и файлового browser
  workspace (см. §7).

### 2.5. Agent Orchestrator / Tool Registry

- Backend строит список tools; Chat tools не передаёт.
- MCP tools добавляются динамически и повторно авторизуются перед dispatch.
- JSON Schema, risk, approval binding, quota/timeouts, execution history.
- Исправлено: неизвестный tool теперь ищется по model name среди разрешённых;
  MCP tool не может обойти workspace allowlist/assignment.

### 2.6. Skills

- Импорт ZIP, защита от zip-slip/symlink/oversize, checksum, версии.
- **Исправлено:** каждая версия хранит полный snapshot всех файлов
  (`ai_skill_version_files`), доступны history/get/restore.
- Скрипты не исполняются автоматически.

### 2.7. User Workspaces / квоты

- Изоляция root, allowlist tools/programs, ACL-инструкции, usage.
- **Исправлено:** Runtime репортит bytes/files; backend сохраняет usage и
  историю samples; UI показывает источник и время.
- **Исправлено:** `fs.apply_patch` проверяет итоговый размер/число файлов до
  записи; `program.run` проверяет usage до запуска и во время выполнения
  (мониторинг + kill tree при превышении).
- `max_parallel_tasks` и user storage quotas пока не enforce (см. §7).

### 2.8. RBAC

- 10 прав `ai.*`, проверки в handlers/service/orchestrator/runtime.
- Ownership: чужие диалоги/задачи/workspace → 404.
- MCP assignment проверяется на discovery-скоупе и повторно перед вызовом.

### 2.9. WebSocket

- Пользовательский `/api/v1/ws` и Runtime `/api/v1/ai/runtime/connect`
  разделены.
- Heartbeat, online/offline/degraded, reconnect, revoke closes connection.
- Sequence/timestamp/messageId защита от replay.
- События AI адресные; секреты/абсолютные пути не передаются.

### 2.10. PostgreSQL

- 20 миграций; 26 AI-таблиц после 0020.
- FK/индексы/уникальность sequence, idempotency keys, dispatch message id.
- Outbox команд, approvals, executions, receipts, usage samples, skill version
  files, MCP registry/assignments.
- Секреты — ciphertext/hash.

### 2.11. System Management

- Overview, Providers (admin list теперь включает отключённые), Runtime,
  Workspaces (+ refresh usage), Skills, Tools & Programs, MCP, Quotas,
  Security & Logs.
- UI показывает реальные ошибки/status; фиктивные данные не добавлялись.

## 3. Что исправлено в Phase 9

1. **PolyStrata:** SSE timeout, error body, cancel non-stream, diagnostics.
2. **MCP:** реестр соединений, discovery, tool snapshots, assignments, SSRF,
   approval policy, выполнение через Runtime.
3. **Network isolation:** Windows Firewall outbound block с fail-closed,
   Job Object kill-on-close, memory limit.
4. **Usage accounting:** реальные bytes/files, история, refresh endpoint.
5. **Cancel confirmation:** `cancel_status`, ack, подтверждение, watchdog,
   отсутствие локального «cancelled» до ack.
6. **Skills versioning:** полный snapshot всех файлов + restore.
7. **Quota pre-checks:** `fs.apply_patch` (delta до записи) и `program.run`
   (до и во время выполнения).
8. **Backend restart recovery:** requeue безопасных задач, receipt
   reconciliation, watchdog, сохранение `waiting_approval`.
9. **Idempotency/replay:** HTTP Idempotency-Key для send/cancel/approve/reject,
   sequence/timestamp/messageId, outbox unique, execution dispatch id.
10. **Frontend:** tail-сообщения, reconnect reload, attachments reset,
    permission-gated approvals, MCP UI, usage refresh, stale Home card fix.

## 4. Аудит безопасности

| Риск | Статус | Мера |
|---|---|---|
| Изоляция пользователей | Закрыто | ownership + 404, workspace owner/admin |
| Path Traversal | Закрыто | `normalize_relative`, canonical containment, повторная проверка |
| Symlink/junction/reparse | Закрыто | `is_reparse_point`, обход всех компонентов, race re-check |
| Prompt Injection | Снижено | policy отделена от untrusted; tool результат/skill/MCP помечены untrusted; approvals серверные |
| Секреты | Закрыто | AES-256-GCM, SHA-256 token, redaction, secret не в outbox/log/UI |
| Права Runtime | Закрыто | отдельный device token, revoke/rotate, disabled/revoked блокируют команды |
| Дочерние процессы | Закрыто/усилено | без shell, allowlist args, Job Object, taskkill /T |
| Сетевые подключения | Частично | SSRF backend+Runtime; egress firewall для программ; MCP redirects off |
| Обход approvals | Закрыто | args_hash, one-time consume, TTL, повторная проверка |
| Повторная доставка команд | Закрыто | sequence/timestamp/messageId, receipts, idempotency |
| Чужие tasks/approvals/usage | Закрыто | owner checks; admin override только с правом |
| MCP SSRF | Закрыто | literal/DNS private/loopback/link-local блок, opt-in |
| Quota race | Частично | mutex в одном Runtime; несколько Runtime на один workspace не поддерживаются |

Критические/высокие риски, найденные в аудите: локальное «cancelled» без
подтверждения Runtime, отсутствие MCP registry, отсутствие network isolation,
отсутствие usage, неполные snapshots Skills, quota bypass для patch/program,
дублирование при retry. Все перечисленные исправлены.

## 5. Сценарий production-проверки на двух компьютерах

Подготовлен `scripts/ai-two-machine-check.ps1`. Порядок:

1. На PC-1: backend с `NEXUS_SECRET`, HTTPS/WSS reverse proxy, миграции.
2. На PC-2: PolyStrata на `127.0.0.1:8080/v1`; `runtime.toml` с
   `server.url=wss://nexus.example/api/v1/ai/runtime/connect`,
   `allowed_roots` внутри выделенного диска.
3. В System Management → AI Management: enrollment code, provider,
   workspace, Skills, programs, MCP.
4. На PC-2: `runtime check --json`, `enroll`, `install-service.ps1`.
5. Проверить: online/heartbeat, Chat streaming, Agent с `fs.read`/`fs.write`,
   approval для `fs.delete`, cancel с `cancelStatus=confirmed`, reconnect после
   `Restart-Service`, revoke/rotate, usage, MCP discovery/call.

Результаты этого сценария должны быть приложены к отчёту с указанием дат,
версий PolyStrata/моделей и логов. Без этого пункт «Production Ready» не
закрывается.

## 6. Автоматизированный стенд (одна машина)

`backend/tests/ai_runtime_stand.rs` (игнорируется по умолчанию):

- запускает настоящее `nexus-ai-runtime.exe` с отдельным `runtime.toml`;
- enrollment через CLI → WSS к реальному Axum-серверу;
- heartbeat/online;
- Chat streaming через локальный OpenAI-compatible stub;
- Agent `fs_write` → реальный файл в workspace;
- usage, измеренный Runtime;
- cancel длинной генерации с подтверждением;
- kill/restart Runtime → повторное online;
- revoke узла.

Результат: **1 passed**. Это стенд, а **не** второй компьютер.

## 7. Оставшиеся ограничения (честно)

1. Реальный сценарий с PolyStrata и Runtime на **втором компьютере** не
   выполнен: нет физического доступа/второй машины в среде аудита.
2. Реальная PolyStrata не установлена на машине аудита; smoke-тест ignored.
3. `max_parallel_tasks` и user storage quotas не enforce в admission control.
4. Network isolation: Windows Firewall правило на executable, требует admin,
   блокирует все процессы этого executable на время запуска. AppContainer/
   process-level egress не реализован.
5. MCP: только Streamable HTTP; stdio-транспорт отсутствует.
6. Quota concurrency между несколькими Runtime одного workspace не поддержана.
7. Frontend: нет browser файлов workspace и выбора старых задач; история
   задач доступна через API/инспектор текущей.
8. Реальный PolyStrata tool calling зависит от модели и версии PolyStrata;
   адаптер валидирует структуру, но не гарантирует поддержку конкретной моделью.
9. Смена `NEXUS_SECRET` делает сохранённые API keys/MCP secrets нечитаемыми —
   их нужно ввести заново.

## 8. Фактические результаты тестов

| Проверка | Результат |
|---|---|
| `cargo check --all-targets` (backend) | успешно, без warnings |
| `cargo test -j 1` (backend, полный) | **411 passed, 0 failed, 1 ignored** (`logs/backend-full-tests-final.log`) |
| `cargo test --all-targets` (runtime) | **28 passed, 1 ignored** |
| `backend/tests/ai_tests.rs` | **16 passed** |
| `ai_runtime_stand` (ignored, реальный процесс Runtime) | **1 passed** |
| `openapi_tests` | 5 passed (185 путей / 224 операции) |
| `migrations_tests` | 1 passed (20 миграций) |
| `npm run typecheck` | успешно |
| `npm run lint` | успешно, 0 warnings |
| `npm run test` | **92 passed** (17 files) |
| `npm run build` | успешно |
| Playwright E2E | **26 passed** |

## 9. Playwright E2E и регрессии

E2E: **26 passed** на release backend и отдельной БД `nexus_e2e` (включая новый
AI smoke `frontend/e2e/ai.spec.ts`: `/workspace/ai` и `/system/ai` → MCP tab).
После финальных UI-правок AI smoke повторён отдельно: **1 passed**.

Регрессии Minecraft/Cloud/System Management проверяются полным backend-прогоном
и E2E; изменения AI не затрагивают их таблицы/маршруты, кроме общих middleware.

## 10. Критерии снятия статуса «не Production Ready»

- Пройти §5 на двух физических компьютерах с реальными PolyStrata и моделью.
- Приложить логи `runtime check --json`, enrollment, Chat, Agent, approval,
  cancel, reconnect, revoke, usage, MCP.
- Убедиться, что network isolation либо включена (Runtime elevated), либо
  администратор осознанно использует `network_policy="allow"`.
- Подтвердить, что выбранная модель PolyStrata корректно возвращает
  `tool_calls` и `tool_call_id`.
