# NEXUS AI Runtime

Отдельное Windows-приложение (и служба), которое устанавливает **исходящее**
WSS-соединение с NEXUS, вызывает локальную PolyStrata и исполняет разрешённые
инструменты внутри изолированных workspace.

Runtime **не открывает входящих портов** и не принимает пользовательскую
cookie-сессию. Устройство аутентифицируется отдельным device token.

## 1. Требования

- Windows 10/11 или Windows Server 2019+;
- PolyStrata на этом компьютере, OpenAI-compatible API;
- NEXUS 1.x с включённым AI-модулем;
- отдельная непривилегированная учётная запись для службы (рекомендуется);
- Rust 1.85+ только для сборки (готовый exe не требует toolchain).

## 2. Сборка

```powershell
cd runtime
cargo build --release
# результат: target\release\nexus-ai-runtime.exe
```

## 3. Установка файлов

Рекомендуемая раскладка:

```text
C:\NexusAI\
  nexus-ai-runtime.exe
  runtime.toml
  state\            # device.json, receipts.jsonl (ACL: только service account)
  logs\
  workspaces\
    user-1\
```

Скопируйте `runtime.toml.example` в `C:\NexusAI\runtime.toml` и заполните:

- `server.url` — `wss://<nexus>/api/v1/ai/runtime/connect`;
- `server.enroll_url` — `https://<nexus>`;
- `storage.allowed_roots` — корни, внутри которых разрешены workspace;
- `provider.base_url` — обычно `http://127.0.0.1:8080/v1`;
  `NEXUS_AI_PROVIDER_API_KEY` (env) переопределяет `provider.api_key`, чтобы не
  хранить секрет в файле;
- `[[programs]]` — только реально установленные программы.

Проверка конфигурации:

```powershell
.\nexus-ai-runtime.exe check --config C:\NexusAI\runtime.toml
.\nexus-ai-runtime.exe check --config C:\NexusAI\runtime.toml --json
```

`check` проверяет доступность NEXUS `/api/v1/health`, PolyStrata `/health`,
`/status`, `/models`, наличие device token, allowed roots, программы и статус
сетевой изоляции. Код возврата `2` — есть проблемы.

## 4. Enrollment

1. В NEXUS откройте **System Management → AI Management → Runtime** и создайте
   одноразовый enrollment code (TTL по умолчанию 15 минут).
2. На компьютере Runtime выполните:

```powershell
.\nexus-ai-runtime.exe enroll `
  --config C:\NexusAI\runtime.toml `
  --code nxe_...
```

3. Устройство появится в списке Runtime. Запустите проверку и службу:

```powershell
.\nexus-ai-runtime.exe run --config C:\NexusAI\runtime.toml
.\install-service.ps1 -BinaryPath C:\NexusAI\nexus-ai-runtime.exe -ConfigPath C:\NexusAI\runtime.toml
```

Код одноразовый: повторный enrollment с ним вернёт `401`.
Device token показывается один раз при enrollment/ротации и хранится только в
`state\device.json` (на сервере — SHA-256 hash).

## 5. Ротация и отзыв

- **Rotate token** в AI Management выдаёт новый токен и закрывает текущее
  соединение. На Runtime выполните:

```powershell
.\nexus-ai-runtime.exe set-token --config C:\NexusAI\runtime.toml `
  --node-id <uuid> --token nxa_...
Restart-Service NexusAIRuntime
```

- **Disable** блокирует новые команды и закрывает канал.
- **Revoke** делает токен недействительным навсегда; требуется новый enrollment.

## 6. PolyStrata

Ожидаемые endpoints:

| Endpoint | Назначение |
|---|---|
| `GET /v1/models` | список моделей |
| `GET /health` | health |
| `GET /status` | статус/версия |
| `POST /v1/chat/completions` | Chat Completions (stream/non-stream) |

Runtime поддерживает SSE (`data: {...}`, `[DONE]`), несколько `tool_calls` за
ответ, `finish_reason`, `usage` (если PolyStrata возвращает). Параметры
`temperature`, `top_p`, `max_tokens` передаются только если заданы в NEXUS и
поддерживаются моделью. При HTTP-ошибке, таймауте или некорректном JSON задача
получает контролируемую ошибку, а не «успех».

## 7. Инструменты

| tool_id | Риск | Назначение |
|---|---|---|
| `fs.list` | low | список файлов |
| `fs.read` | low | чтение UTF-8 |
| `fs.write` | medium | запись/создание |
| `fs.search` | low | поиск по файлам |
| `fs.mkdir` | low | создание каталога |
| `fs.move` | high | перемещение |
| `fs.copy` | medium | копирование |
| `fs.delete` | high | удаление |
| `fs.apply_patch` | medium | unified diff |
| `program.run` | high | зарегистрированная программа |

Все пути относительны workspace root. Runtime повторно проверяет:

- отсутствие абсолютных путей, UNC и `..`;
- containment внутри canonical workspace root;
- symlink/junction/reparse points;
- лимиты размера, глубины и числа файлов;
- локальный allowlist программ и аргументов.

Программы запускаются напрямую (без shell). Аргументы с абсолютными путями,
UNC и `..` отклоняются. `program.run` всегда требует подтверждения пользователя
в NEXUS (policy `always` по умолчанию).

`fs.apply_patch` сначала проверяет все hunks в памяти и только затем пишет файлы.

## 8. Повторная доставка и сбои

- Backend хранит команды в PostgreSQL и повторно отправляет незавершённые
  команды после reconnect.
- Runtime дедуплицирует команды по `messageId` (in-memory + `receipts.jsonl`).
- При разрыве соединения неидемпотентные операции не перезапускаются
  автоматически: NEXUS переводит задачу в контролируемую ошибку/ручную проверку.
- `task.cancel` отменяет HTTP-генерацию и завершает дерево процессов
  (`taskkill /T /F` + `child.kill()` + Job Object). Runtime присылает
  `task.cancel_ack`, ждёт завершения активных операций и только затем
  `task.cancelled{confirmed=true}`.
- Runtime не хранит состояние задач как источник истины: истина — PostgreSQL NEXUS.

## 9. Ограничения и безопасность

- Runtime работает без входящего HTTP-сервера; единственный канал — WSS.
- Служба должна работать от непривилегированной учётной записи с доступом
  только к `state`, `logs` и назначенным workspace.
- Не используйте одну и ту же папку для Cloud Storage NEXUS и AI workspace.
- Скрипты из Skills не исполняются автоматически: только зарегистрированная
  программа с approval.
- Программы: allowlist аргументов, без shell, Job Object (kill-on-close),
  лимит памяти. `network_policy="deny"` (по умолчанию) временно добавляет
  Windows Firewall outbound block для executable; без прав администратора
  запуск отклоняется (fail closed). Правило адресуется executable, поэтому на
  время запуска блокирует сеть всех процессов этого executable.
- MCP включён через реестр соединений (`mcp.configure`/`mcp.discover`), только
  Streamable HTTP; SSRF-проверка выполняется и на Runtime.
- Секреты (device token, API key, MCP secret) не пишутся в журналы; journald/stderr
  службы не содержит токенов. Outbox NEXUS не хранит MCP secret.

## 10. Диагностика

| Симптом | Проверка |
|---|---|
| Runtime offline | служба запущена, `check` видит NEXUS, WSS URL корректен, системное время синхронизировано |
| `AI_RUNTIME_REVOKED` | выполните новый enrollment |
| PolyStrata недоступна | `curl http://127.0.0.1:8080/v1/models`, `curl http://127.0.0.1:8080/health` |
| `AI_WORKSPACE_PATH_DENIED` | workspace root в NEXUS должен находиться внутри `storage.allowed_roots` и существовать на диске |
| Программа missing | проверьте путь в `[[programs]]` и права service account |
| Задача зависла | проверьте `ai.task.events`/Runtime log; отмените задачу в UI |
| Reconnect loop | проверьте TLS-сертификат NEXUS; для локальной отладки только явно `allow_insecure_ws = true` |

Журналы: `C:\NexusAI\logs\nexus-ai-runtime.<date>` (ротация,
`logging.keep_files`). При старте службы stderr не подключён, поэтому файловый
журнал обязателен.
