# NEXUS AI Runtime

Отдельное Windows-приложение (и служба), которое устанавливает **исходящее**
WSS-соединение с NEXUS, вызывает локальную PolyStrata и исполняет разрешённые
инструменты внутри изолированных workspace. Документ дополняет
[`runtime/README.md`](../runtime/README.md) и [`AI_WORKSPACE.md`](AI_WORKSPACE.md).

## 1. Роль и границы

- Runtime не открывает входящих портов и не принимает пользовательскую
  cookie-сессию.
- Устройство аутентифицируется отдельным отзывным device token `nxa_…`
  (в PostgreSQL хранится только SHA-256).
- Runtime — исполнитель, а не центр авторизации: права, workspace allowlist,
  JSON Schema, risk и approval проверяются Backend и повторно на Runtime.
- Backend остаётся источником истины; Runtime не хранит состояние задач.

## 2. Установка и enrollment

```powershell
# Сборка
cd runtime
cargo build --release

# Проверка конфигурации
.\target\release\nexus-ai-runtime.exe --config C:\NexusAI\runtime.toml check --json

# Одноразовый код создаётся в NEXUS: AI Management → Runtime
.\target\release\nexus-ai-runtime.exe --config C:\NexusAI\runtime.toml enroll --code nxe_...
.\target\release\nexus-ai-runtime.exe --config C:\NexusAI\runtime.toml run
.\install-service.ps1 -BinaryPath C:\NexusAI\nexus-ai-runtime.exe -ConfigPath C:\NexusAI\runtime.toml
```

- Код enrollment одноразовый, TTL 60–3600 с; повторное использование → 401.
- Device token показывается один раз и сохраняется в `state\device.json`.
- Ротация: `set-token --node-id <uuid> --token nxa_…`, затем перезапуск службы.
- Revoke делает токен недействительным навсегда; требуется новый enrollment.

## 3. Конфигурация (`runtime.toml`)

Ключевые параметры:

| Секция | Параметр | Назначение |
|---|---|---|
| `[server]` | `url` | `wss://<nexus>/api/v1/ai/runtime/connect` |
| `[server]` | `enroll_url` | база NEXUS для enrollment и `check` |
| `[server]` | `heartbeat_seconds` | heartbeat (по умолчанию 20) |
| `[server]` | `reconnect_min/max_seconds` | backoff переподключения |
| `[storage]` | `allowed_roots` | корни, внутри которых разрешены workspace |
| `[storage]` | `state_dir` | `device.json`, `receipts.jsonl` |
| `[storage]` | `receipts_max_bytes` | размер текущего сегмента receipts (по умолчанию 10 МБ) |
| `[storage]` | `receipts_max_archives` | число хранимых архивных сегментов (по умолчанию 16) |
| `[storage]` | `receipts_load_limit` | сколько receipts читать при старте (по умолчанию 5000) |
| `[provider]` | `base_url` | обычно `http://127.0.0.1:8080/v1` |
| `[provider]` | `api_key` | можно переопределить `NEXUS_AI_PROVIDER_API_KEY` |
| `[limits]` | `max_parallel_tasks` | семафор одновременных операций |
| `[limits]` | `max_file_bytes`, `max_tool_output_bytes` | лимиты файлов и вывода |
| `[limits]` | `env_allowlist` | переменные, передаваемые программам |
| `[[programs]]` | `program_id`, `executable`, `arg_patterns` | реестр разрешённых программ |

Workspace обязан находиться внутри `allowed_roots`; несуществующий корень —
ошибка старта.

## 4. Протокол Backend ↔ Runtime

Конверт:

```json
{
  "protocolVersion": 1,
  "messageId": "uuid",
  "type": "tool.execute",
  "timestamp": "2026-11-24T12:00:00Z",
  "nodeId": "uuid",
  "correlationId": "uuid",
  "taskId": "uuid",
  "sequence": 42,
  "payload": {}
}
```

Типы: `runtime.hello/hello_ack/heartbeat/heartbeat_ack/capabilities/
capabilities_ack/error`, `provider.chat.start/accepted/delta/completed/failed/
cancel`, `provider.request/response`, `tool.execute/accepted/progress/completed/
failed`, `task.cancel/cancel_ack/cancelled`, `workspace.usage/usage_result`,
`execution.status/status_result`, `mcp.configure/configure_ack/discover/tools`,
`workspace.delete/delete_result`.

Защита от повторов:

- Backend отклоняет кадры Runtime с `sequence` не больше последнего и
  timestamp старше/новее 15 минут.
- Runtime делает то же для команд; повторный `messageId` возвращает сохранённый
  результат из памяти/`receipts.jsonl` без повторного side effect.
- `ai_runtime_commands.message_id` уникален; `ai_tool_executions` уникален по
  `(task_id, tool_call_id)` и `dispatch_message_id`.

### 4.1. Возможности (capabilities)

`runtime.hello`/`runtime.capabilities` содержат:

- `runtimeVersion`, `platform`, `protocolVersion`;
- `models` и `polystrata` (reachable/version/modelCount);
- `tools[]` — `toolId`, `version`, `risk`, `inputSchema`, `timeoutSeconds`,
  `requiresPermission`, `idempotent`;
- `programs[]` — `programId`, `executable`, `available`, `networkPolicy`;
- `networkIsolation` — supported/elevated/detail.

Backend сохраняет capabilities и **не предлагает модели инструмент, которого
нет на устройстве или у которого не совпадает версия**. Пустой список `tools`
трактуется как legacy/неизвестный Runtime и не фильтруется.

## 5. Инструменты

| tool_id | Риск | Назначение |
|---|---|---|
| `fs.list` | low | список файлов и каталогов |
| `fs.read` | low | чтение UTF-8 файла с лимитом |
| `fs.stat` | low | метаданные файла/каталога |
| `fs.search` | low | поиск строки в текстовых файлах |
| `fs.write` | medium | создание/перезапись файла |
| `fs.mkdir` | low | создание каталога |
| `fs.move` | high | перемещение/переименование |
| `fs.copy` | medium | копирование файла |
| `fs.delete` | high | удаление |
| `fs.apply_patch` | medium | unified diff с предварительной проверкой |
| `program.run` | high | зарегистрированная программа без shell |

Все пути относительны корня workspace. Runtime повторно проверяет:

- отсутствие абсолютных путей, UNC и `..`;
- containment внутри canonical workspace root;
- symlink/junction/reparse points и их предков;
- лимиты размера, глубины, числа файлов и вывода;
- локальный allowlist программ и аргументов.

`fs.apply_patch` проверяет все hunks в памяти и только затем пишет файлы.
Изменяющие операции возвращают `changedFiles`, а patch — ограниченный `diff`.

## 6. Процессы

- Запускаются только зарегистрированные программы; shell не используется.
- Рабочий каталог — внутри workspace, не symlink/junction.
- Аргументы проверяются по шаблонам; абсолютные пути, UNC, `..` и управляющие
  символы запрещены.
- Ограничиваются время, память (Job Object), stdout/stderr, дерево процессов.
- `task.cancel` отменяет генерацию и завершает дерево процессов
  (`taskkill /T /F`, `child.kill()`, Job Object) и только затем подтверждает
  остановку.
- `network_policy="deny"` временно добавляет outbound-правило Windows Firewall
  для executable; без прав администратора запуск отклоняется (fail closed).
  Правило адресуется executable, поэтому на время запуска блокирует сеть всех
  процессов этого executable.

## 7. Сбои и восстановление

- Backend хранит команды в PostgreSQL и переотправляет незавершённые.
- Runtime дедуплицирует команды по `messageId` (память + журнал receipts),
  включая операции, завершившиеся известной ошибкой: повторная доставка
  возвращает тот же результат и не выполняет side effect заново.
- `receipts.jsonl` ротируется: текущий сегмент ограничен
  `storage.receipts_max_bytes` (по умолчанию 10 МБ), архивные сегменты
  `receipts.NNNNNN.jsonl` читаются при старте (`receipts_load_limit`,
  по умолчанию 5000 записей) и удаляются только самые старые сверх
  `storage.receipts_max_archives` (по умолчанию 16). Запись больше не
  прекращается молча.
- Перед удалением сегмента его `messageId` сжимаются в
  `receipts.index.jsonl`. Индекс не содержит результата, но запрещает
  повторное исполнение: на повторную доставку Runtime отвечает
  `AI_RECEIPT_ROTATED`. Таким образом ротация не открывает путь к повторному
  side effect.
- Удаляются только старые сегменты. Backend не переотправляет команды в
  статусах `completed`/`failed`, поэтому окно хранения покрывает все возможные
  повторные доставки; для неизвестного результата остаётся
  `AI_EXECUTION_UNKNOWN` без автоматического повтора.
- При разрыве соединения неидемпотентные операции не перезапускаются
  автоматически: Backend переводит задачу в контролируемую ошибку/ручную
  проверку.
- Идемпотентные инструменты (`fs.read`, `fs.list`, `fs.stat`, `fs.search`,
  `fs.mkdir`) могут быть повторены Backend максимум один раз и только при
  ошибке до/без side effect (`AI_TOOL_FAILED`, `AI_RUNTIME_OFFLINE`).
  Тайм-аут, обрыв соединения и `AI_EXECUTION_UNKNOWN` не повторяются никогда.
- `execution.status` позволяет Backend сверить результат ранее выданной
  команды после reconnect/рестарта; Runtime отвечает `completed`, `failed`
  (из журнала ошибок) или `unknown`.

### 7.0. Физическое удаление workspace

`workspace.delete` принимает `workspaceId` и сохранённый backend `workspaceRoot`.
Runtime не доверяет пути: каталог обязан быть прямым дочерним каталогом
`storage.provisioning_root` с именем UUID, не быть symlink/junction и совпадать
с маркером `.nexus-workspace.json`, созданным при provisioning. Для каталогов,
созданных до появления маркеров, допускается точное совпадение пути
`<provisioning_root>/<uuid>` (legacy); чужой маркер всегда отклоняется.

Удаление идемпотентно: каталог атомарно переименовывается в
`<provisioning_root>/.nexus-trash/<uuid>` и затем удаляется. Повторный вызов
при отсутствии каталога возвращает `alreadyAbsent=true`. Backend очищает
snapshots и usage-семплы только после `ok=true`; ошибка владения переводит
запись в `blocked` и не повторяется автоматически.

### 7.1. Ограничение: длительные процессы

Жизненный цикл `program.start/poll/stop`, отдельный `tool.progress` и остановка
одного процесса без отмены задачи **не реализованы**. `program.run` остаётся
синхронным, ограничен тайм-аутом задачи и завершается вместе с ней. Это
осознанно вынесено за рамки текущего этапа.

## 8. Безопасность

- Служба работает от непривилегированной учётной записи с доступом только к
  `state`, `logs` и назначенным workspace.
- Не используйте одну папку для Cloud Storage NEXUS и AI workspace.
- Скрипты из Skills не исполняются автоматически.
- Секреты (device token, API key, MCP secret) не пишутся в журналы.
- Входящие порты не открываются; единственный канал — WSS.

## 9. Диагностика

| Симптом | Проверка |
|---|---|
| Runtime offline | служба запущена, `check` видит NEXUS, WSS URL и время синхронизированы |
| `AI_RUNTIME_REVOKED` | выполните новый enrollment |
| PolyStrata недоступна | `curl http://127.0.0.1:8080/v1/models`, `/health` |
| `AI_WORKSPACE_PATH_DENIED` | workspace root внутри `allowed_roots` и существует |
| Программа missing | проверьте путь в `[[programs]]` и права service account |
| Задача зависла | `ai_task_events`/Runtime log; отмените задачу в UI |
| Reconnect loop | TLS-сертификат NEXUS; для локальной отладки только `allow_insecure_ws = true` |

Журналы: `C:\NexusAI\logs\nexus-ai-runtime.<date>` (ротация,
`logging.keep_files`).

## 10. Проверка на двух компьютерах

Обязательный сценарий с реальными ПК, TLS, PolyStrata и моделью описан в
[`TWO_MACHINE_CHECK.md`](TWO_MACHINE_CHECK.md). Автоматизированная часть
запускается на AI-ПК: `scripts/ai-two-machine-verify.ps1`. Локальный стенд
(`scripts/ai-runtime-stand.ps1`) и Playwright не заменяют этот прогон.
