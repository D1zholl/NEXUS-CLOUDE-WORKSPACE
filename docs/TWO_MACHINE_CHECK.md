# NEXUS Agent Mode — проверка на двух физических компьютерах

Документ описывает обязательный интеграционный сценарий для Agent Mode.
**Он не выполнялся в среде разработки**: автоматические тесты используют
локальный стенд на одной машине (реальный Runtime + контролируемая модель или
реальная PolyStrata по переменной окружения). Двухмашинный прогон должен быть
выполнен оператором и оформлен отдельным отчётом по шаблону в конце.

## 1. Топология

| Компьютер | Компоненты | Сеть |
|---|---|---|
| PC-1 | PostgreSQL, NEXUS Backend, Frontend, Caddy (HTTPS) | входящий 443 |
| PC-2 | `nexus-ai-runtime.exe`, PolyStrata, локальная модель, workspace | только **исходящий** WSS к PC-1 |

Требования:

- на PC-1 валидный TLS-сертификат (Caddy), `NEXUS_COOKIE_SECURE=true`;
- на PC-2 Runtime работает от непривилегированной учётной записи;
- `allowed_roots` Runtime содержит каталог workspace и не пересекается с
  Cloud Storage;
- часы PC-1 и PC-2 синхронизированы (допуск протокола ±15 минут);
- PolyStrata доступна с PC-2 по OpenAI-compatible API;
- версии Backend и Runtime совместимы (`runtime check --json` показывает
  protocolVersion и capabilities).

## 2. Подготовка

### PC-1

```powershell
# Backend + миграции (0021 применяется штатным migrate; на рабочей БД —
# только после резервной копии и отдельного разрешения).
.\start.bat
# Проверка HTTPS
Invoke-WebRequest https://nexus.example/api/v1/health
```

В AI Management:

1. Runtime → создать enrollment code (TTL 15 минут).
2. Provider → base URL PolyStrata, default model, привязать к Runtime-узлу.
3. Workspace → владелец, абсолютный путь на PC-2, allowlist инструментов.
   Для старого workspace нажать «Добавить безопасные», если показан список
   отсутствующих инструментов (`fs.stat`, `skills.list` и т. п.).

### PC-2

```powershell
.\runtime\target\release\nexus-ai-runtime.exe `
  --config C:\NexusAI\runtime.toml check --json

.\runtime\target\release\nexus-ai-runtime.exe `
  --config C:\NexusAI\runtime.toml enroll --code nxe_... --name $env:COMPUTERNAME

.\install-service.ps1 -BinaryPath C:\NexusAI\nexus-ai-runtime.exe `
  -ConfigPath C:\NexusAI\runtime.toml
Start-Service NexusAiRuntime
```

## 3. Автоматизированный сценарий

Скрипт запускается **на PC-2** (ему нужен доступ к workspace и службе Runtime),
а Backend вызывает по HTTPS на PC-1:

```powershell
$env:NEXUS_ADMIN_PASSWORD = '<пароль администратора>'
.\scripts\ai-two-machine-verify.ps1 `
  -BackendUrl https://nexus.example `
  -AdminUser admin `
  -AdminPasswordEnv NEXUS_ADMIN_PASSWORD `
  -RuntimeNodeName $env:COMPUTERNAME `
  -RuntimeServiceName NexusAiRuntime `
  -ProviderUrl http://127.0.0.1:8080/v1 `
  -Model <модель> `
  -WorkspaceRoot C:\NexusAI\workspaces\agent-check `
  -SkillSlug <slug, если проверяются Skills>
```

Скрипт проверяет:

| Проверка | Автоматически |
|---|---|
| HTTPS/TLS, health PC-1 | да |
| PolyStrata `/models`, `/health`, `/status` | да |
| Runtime config, protocolVersion, capabilities, наличие инструментов | да |
| Enrollment и online-статус по WSS | да |
| Совместимость workspace: безопасные инструменты и `missingSafeTools` | да |
| Agent-задача чтения файла, события (`eventId`, порядок, уникальность) | да |
| Создание файла на диске PC-2 через Runtime | да (если модель вызвала `fs_write`) |
| Approval: reject не меняет файл, approve меняет | да, при вызове опасного инструмента |
| Cancel с подтверждением Runtime | да |
| Перезапуск службы Runtime, offline → online, новая задача | да |
| Нет повторного enrollment после рестарта | да |
| Последовательный многошаговый прогон (≥ 2 инструментов) | да (зависит от модели) |
| Skills: видимость и `skill.load` | да, при `-SkillSlug` |
| Отказ при отсутствии прав | вручную: выдать workspace без `fs.write` и запросить запись |
| Реальная модель вызывает нужный инструмент | WARN, если модель ответила без tool call |

Коды выхода: `0` — нет FAIL (WARN допустимы), `1` — есть FAIL, `2` —
предусловия не выполнены.

## 4. Ручные проверки, которые нельзя автоматизировать

1. **Отказ в правах**: создать второй workspace без `fs.write`/`fs.delete`,
   отправить задачу на запись, убедиться, что Runtime не выполняет операцию,
   а Backend возвращает `AI_TOOL_NOT_ALLOWED`.
2. **Reconnect при обрыве сети**: отключить сетевой интерфейс PC-2 на 30–60 с
   во время задачи; после восстановления проверить, что Runtime переподключился,
   задача не выполнила неидемпотентную операцию повторно
   (`AI_EXECUTION_UNKNOWN` либо receipt-восстановление), история событий полна.
3. **Рестарт Backend во время задачи**: перезапустить службу NEXUS, убедиться,
   что задача продолжилась либо завершилась контролируемой ошибкой, без
   дублирования side effect.
4. **F5 в браузере**: обновить страницу во время выполнения, проверить
   восстановление активной задачи, промежуточного текста, executions и событий.
5. **Длительная работа**: задача с 4–6 последовательными инструментами
   (чтение → запись → patch → чтение → отчёт), проверить `changedFiles`, diff,
   exit code и отсутствие зависания.
6. **Аудит**: в System → Audit убедиться, что есть `ai.task.*`, `ai.tool.*`,
   `ai.approval.*`, `ai.workspace.tools_granted`, без аргументов и путей.

## 5. Шаблон отчёта

```text
Дата:
PC-1 (Backend): версия, ОС, HTTPS/TLS
PC-2 (Runtime): версия Runtime, ОС, PolyStrata, модель
ProtocolVersion:
Enrollment: да/нет
Runtime online: да/нет
Chat streaming: pass/fail
Agent streaming: pass/fail
Tool calling: pass/fail (какие инструменты)
Файлы: pass/fail (пути)
Skills: pass/fail
Approval approve: pass/fail
Approval reject: pass/fail
Отказ в правах: pass/fail
Cancel: pass/fail (cancelStatus)
Reconnect: pass/fail
Recovery после рестарта Backend: pass/fail
Recovery после рестарта Runtime: pass/fail
Восстановление событий: pass/fail
Длительный многошаговый прогон: pass/fail (число инструментов)
Замечания:
```

Результат должен быть приложен к `docs/TESTING.md`. До выполнения этого
сценария Agent Mode нельзя объявлять проверенным на двух компьютерах.
