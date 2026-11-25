# AI Agent Workspace — план, карта изменений и результат

Документ фиксирует аудит фактической реализации (не только документации),
выявленные пробелы и принятые архитектурные решения. Итоговое состояние
реализации — в [`AI_IMPLEMENTATION_STATUS.md`](../AI_IMPLEMENTATION_STATUS.md).

## 1. Что уже существует (проверено в коде)

| Область | Факт |
|---|---|
| HTTP | `backend/src/ai/routes.rs` + `admin_routes.rs`; всё монтируется в `/api/v1/ai`, body limit 16 МБ, глобальные CSRF/module-gate middleware (`backend/src/lib.rs`) |
| RBAC | `AuthUser::require("ai.*")` в каждом handler; 10 прав `ai.*` |
| Диалоги | `conversations.rs`: streaming, regenerate/edit/stop, attachments, pagination (`tail=true`) |
| Agent | `orchestrator.rs`: последовательные model/tool циклы, JSON Schema, approvals, cancel, recovery, idempotency |
| Runtime | `runtime/` (отдельный Rust процесс): WSS, tool registry, path containment, квоты, Job Object |
| Инструменты | `fs.list/read/write/search/stat/mkdir/move/copy/delete/apply_patch`, `program.run` |
| Workspace | `workspaces.rs`: owner isolation, quota bytes/files/max file, usage через Runtime |
| Frontend | один экран `/workspace/ai` (`AiWorkspacePage.tsx`), `services/ai.ts`, `lib/taskEvents.ts`, `lib/realtime.ts` |
| Квоты | workspace-квоты **enforce** в Runtime; `ai_user_quotas.max_storage_bytes/max_files` только отображаются |

## 2. Реальные пробелы (совпадают с §5 AI_IMPLEMENTATION_STATUS)

1. Нет файлового браузера/редактора workspace — только дерево задач и чат.
2. Нет API для файловых операций от имени пользователя (инструменты вызывал
   только orchestrator).
3. Нет выбора старых задач и восстановления их состояния в UI.
4. Нет diff/снимков изменений агента и безопасного отката.
5. Нет панели execution output (stdout/stderr) как отдельного рабочего блока.
6. Нет пользовательского создания workspace (только админ), нет описания,
   переименования и удаления владельцем.
7. `ai_user_quotas.max_storage_bytes/max_files/max_file_bytes` не enforce.
8. Нет просмотра изображений/бинарных файлов и upload/download.

## 3. Архитектурные решения

### 3.1 Единственный исполнитель — существующий Runtime

Пользовательские файловые операции не читают диск сервера. Backend формирует
`tool.execute` тем же `ToolExecuteRequest`, что и orchestrator, и ждёт
`RuntimeReply::Tool`. Runtime повторно проверяет: root по allowlist, containment,
reparse point, JSON Schema, версию tool, квоты, размер файла. Второго исполнителя
нет.

Разграничение прав:

* **Agent** — дополнительно ограничен `ai_workspaces.allowed_tools` и политиками
  `ai_tool_policies` (решение администратора о том, что может модель).
* **Владелец workspace** — ограничен RBAC (`ai.workspace.read` /
  `ai.workspace.write`), владением workspace, статусом workspace и глобальным
  `enabled` политики инструмента. Allowlist агента не применяется: пользователь
  является принципалом, а allowlist существует для ограничения модели.
* `program.run` через пользовательский API **не** доступен: программы
  запускаются только агентом после approval.
* Служебные инструменты (`fs.read_base64`, `fs.write_base64`) помечены `ui_only`
  и никогда не попадают в tool definitions модели.

### 3.2 Оптимистическая блокировка записи

`fs.read` уже возвращает `sha256` содержимого, `fs.write` — `sha256`
записанного. Добавлено необязательное поле `expectedSha256`:

* Runtime под per-workspace mutex сравнивает текущий хеш файла с ожидаемым
  **до** записи и отклоняет операцию при расхождении;
* проверка и запись атомарны внутри одной tool-операции, гонки check-then-act
  нет;
* backend в ответе `409 AI_FILE_CONFLICT` возвращает актуальные содержимое и
  хеш, чтобы UI показал сравнение, а не молча перезаписал правку пользователя.

Поле аддитивно, схема расширена, версия инструмента не менялась.

### 3.3 Снимки изменений агента (реальный, не фиктивный rollback)

Runtime при изменяющих операциях возвращает `snapshots`:

```json
{ "path": "src/a.ts", "action": "modify",
  "beforeBase64": "...", "afterBase64": "...",
  "beforeSha256": "...", "afterSha256": "...", "truncated": false }
```

Ограничения: 256 КиБ на файл, 2 МиБ суммарно, не более 100 файлов на
операцию. При превышении `truncated=true` и откат для этого файла запрещён.

Backend сохраняет снимки в `ai_file_snapshots` (миграция `0022`) с привязкой к
задаче и execution. Откат:

* проверяет, что текущий `sha256` файла совпадает с `afterSha256` снимка;
* при расхождении возвращает `409` со списком конфликтов и **не трогает файл**;
* восстанавливает `before` (или удаляет файл, если `before` отсутствовал);
* пишет аудит `ai.workspace.snapshot_restored`.

### 3.4 Создание workspace без раскрытия путей

Добавлен протокол `workspace.provision`:

1. Backend просит Runtime создать каталог `<provisioningRoot>/<workspaceId>`.
2. Runtime валидирует UUID, создаёт каталог внутри `storage.provisioning_root`
   (по умолчанию — первый `allowed_roots`) и возвращает абсолютный путь
   **только backend**.
3. Backend сохраняет путь в `ai_workspaces.root_path`; пользователю путь не
   отдаётся (`view(false)`).

Удаление workspace владельцем — мягкое (`deleted_at`): строки, задачи и файлы
сохраняются, workspace исчезает из пользовательских списков. Физическое удаление
файлов не выполняется автоматически (защита пользовательских данных).

### 3.5 Квоты

* workspace `quota_bytes/max_files/max_file_bytes` — передаются в каждый
  `tool.execute` и проверяются Runtime (уже было).
* `ai_user_quotas.max_storage_bytes/max_files` — проверяются backend до
  изменяющей операции по сумме `usage_bytes/usage_files` workspace пользователя
  плюс дельта операции; после операции usage обновляется из ответа Runtime.

## 4. Карта изменений

### Runtime (`runtime/src`)

| Файл | Изменение |
|---|---|
| `protocol.rs` | `FileSnapshot`, `ToolOutcome.snapshots`, `workspace.provision*`, `RuntimeCapabilities.provisioning` |
| `filesystem.rs` | `read_bytes`, `write_bytes_atomic` (тот же atomic write) |
| `patch.rs` | возвращает before/after содержимое для снимков |
| `tools.rs` | `fs.read_base64`, `fs.write_base64`, `expectedSha256`, сбор снимков |
| `config.rs` | `storage.provisioning_root`, лимиты снимков |
| `runtime.rs` | обработчик `workspace.provision` |

### Backend (`backend/src/ai`)

| Файл | Изменение |
|---|---|
| `workspace_files.rs` (новый) | API файлов, upload/download, квоты, аудит, снимки/откат |
| `workspaces.rs` | создание/переименование/удаление владельцем, overview, provision |
| `routes.rs` | монтирование новых маршрутов |
| `tools.rs` | `ui_only` инструменты, `expectedSha256` в схеме |
| `orchestrator.rs` | сохранение снимков после tool execution |
| `models.rs` | `description`, `deleted_at` в `WorkspaceRow` |
| `admin_routes.rs` | совместимость выборок workspace |
| `../migrations/0022_ai_agent_workspace.sql` | `ai_file_snapshots`, `description`, `deleted_at` |

### Frontend (`frontend/src`)

| Путь | Назначение |
|---|---|
| `pages/ai/AIAgentWorkspacePage.tsx` | трёхпанельная IDE, `/workspace/ai/agent` |
| `components/ai/workspace/*` | Explorer, toolbar, switcher, контекстное меню |
| `components/ai/editor/*` | вкладки, редактор (highlight.js + overlay), diff |
| `components/ai/agent/*` | сообщения, composer, activity, статус задачи |
| `components/ai/tasks/*` | список задач, детали, история выполнений |
| `components/ai/terminal/ExecutionOutput.tsx` | stdout/stderr |
| `services/aiWorkspace.ts` | клиент новых endpoint-ов |
| `stores/ai/agentWorkspace.ts` | layout, вкладки, несохранённые буферы |
| `styles/pages/ai-agent.css` | дизайн-система NEXUS |

## 5. Ограничения, которые не скрываются

1. Долгоживущих процессов нет: `program.run` синхронный, `program.start/poll/stop`
   не реализованы. В UI это подписано явно.
2. Терминала/произвольного shell нет и не добавляется.
3. Снимки ограничены по размеру; для больших файлов откат недоступен
   (`truncated=true`).
4. Откат восстанавливает только файлы, изменённые агентом через Runtime tools.
   Внешние изменения (другие инструменты, программы) не отслеживаются.
5. Удаление workspace мягкое.
6. MCP только Streamable HTTP; stdio отсутствует.

## 6. Результат по фазам

| Фаза | Статус | Что сделано |
|---|---|---|
| 1. Architecture audit | готово | этот документ; карта API/таблиц/инвариантов |
| 2. Workspace Explorer | готово | дерево с виртуализацией, CRUD, upload/download, DnD, поиск имён и содержимого, сортировка, контекстное меню, состояния |
| 3. Editor | готово | вкладки, подсветка, нумерация, поиск, Ctrl+S/автосохранение, Markdown/JSON/изображения, diff, конфликты по sha256 |
| 4. Agent Workspace UI | готово | `/workspace/ai/agent`, три панели, resizer-ы, раскладка, streaming, действия, approvals, мобильный режим |
| 5. Task Manager | готово | фильтры, пагинация, детали, события, выполнения, повторное открытие без запуска |
| 6. Diff & Changes | готово | `ai_file_snapshots`, diff, принятие результата, безопасный откат файла/задачи |
| 7. Execution Panel | готово | Output (stdout/stderr), Problems, Execution History, Changed Files, Usage |
| 8. Security & Quotas | готово | ownership/RBAC, path checks, optimistic locking, квоты workspace и пользователя, аудит, idempotency |
| 9. Testing & Polish | готово | backend/unit/frontend/E2E, build, lint, typecheck |

## 7. Проверки (25.11.2026)

| Проверка | Результат |
|---|---|
| backend `cargo test -j 1 --no-fail-fast` | 433 passed, 0 failed, 1 ignored |
| `ai_tests` (+3 Agent IDE) | 21 passed |
| `openapi_tests` | 5 passed, 200 путей / 244 операции |
| `migrations_tests` | 2 passed (22 миграции) |
| runtime `cargo test --all-targets` | 36 passed, 1 ignored |
| frontend `npm run typecheck` / `lint` | успешно, 0 warnings |
| frontend `npm run test` | 126 passed, 22 файла |
| frontend `npm run build` | успешно |
| Playwright `agent-workspace.spec.ts` | 5 passed |
| Playwright (весь набор) | 38 passed, 1 skipped, 0 failed |

## 8. Что осталось за рамками

Перечислено в §5 этого документа и в §13 `AI_WORKSPACE.md`: долгоживущие
процессы/терминал, MCP stdio, process-level egress isolation, второй физический
компьютер, реальная PolyStrata, физическая очистка удалённого workspace,
снимки для файлов >256 КиБ и рекурсивно удалённых каталогов.
