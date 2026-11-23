# Система событий NEXUS-launcher

**Версия документа:** 1.0
**Версия схемы событий:** `1`
**Дата:** 21.11.2026
**Статус:** проект. Реализация не начата.
**Связанные документы:** `docs/ARCHITECTURE.md`, `docs/COMMANDS.md`,
`docs/DECISIONS.md`.

---

## 1. Назначение

События — **уведомления о фактах и прогрессе**, а не транспорт состояния.

| Что | Как передаётся |
|---|---|
| Намерение пользователя | типизированная команда |
| Авторитетное состояние | ответ команды / повторный запрос |
| Прогресс, консоль, метрики, смена статуса | событие |
| Ошибка команды | `ErrorDto` в ответе |
| Ошибка фоновой операции | `job:failed` + доменное событие |

Правила:

- событие не является единственным источником истины;
- UI после события инвалидирует запрос или обновляет «живой» буфер;
- события идемпотентны по смыслу: повторная доставка не должна ломать UI;
- события не содержат секретов, полных путей и содержимого файлов;
- события не заменяют логи: лог — на диске, событие — для UI.

---

## 2. Конверт события

Каждое событие имеет одинаковый конверт:

```json
{
  "schemaVersion": 1,
  "name": "download:progress",
  "eventId": "0f0a7f6e-9f6e-4f2b-9f0e-6f7f5f4f3f2e",
  "occurredAt": 1795274960000,
  "jobId": "0193b2c4-7f10-7c3a-9d55-2f4b1c8e9a01",
  "entity": { "kind": "download", "id": "0193..." },
  "seq": 42,
  "payload": {
    "filesDone": 41,
    "filesTotal": 124,
    "bytesDone": 25000000,
    "bytesTotal": 60500000,
    "speedBps": 4200000,
    "etaSeconds": 8
  }
}
```

| Поле | Тип | Обязательно | Смысл |
|---|---|---|---|
| `schemaVersion` | u16 | да | Версия конверта, сейчас `1` |
| `name` | string | да | То же имя, что у Tauri-события |
| `eventId` | UUID | да | Уникальный id; для дедупликации в тестах/логах |
| `occurredAt` | i64 | да | Unix ms UTC |
| `jobId` | UUID | нет | Родительская операция |
| `entity` | object | нет | `{ kind, id }` затронутого объекта |
| `seq` | u64 | нет | Монотонный номер в рамках `(name, entity)` |
| `payload` | object | да | Типизированные данные события |

Правила:

- `entity.kind` — `instance`, `server`, `download`, `account`, `job`,
  `source`, `backup`, `java`, `run`.
- `seq` обязателен для потоковых событий (`server:console`,
  `minecraft:log`) и для `job:progress`.
- В `payload` нет `camelCase`-дублей метаданных; они только в конверте.

### 2.1. Именование Tauri-событий

`<domain>:<event>` в нижнем регистре. Допустимые символы: `a-z`, `0-9`,
`-`, `_`, `/`, `:`. Имя после релиза не меняется; новое поведение — новое
событие.

---

## 3. Доставка и порядок

| Свойство | Гарантия |
|---|---|
| Транспорт | `AppHandle::emit` из `TauriEventSink` |
| Порядок | Гарантирован в пределах одного `(name, entity)` при эмиссии из одного канала |
| Дубликаты | Возможны при переподключении UI; обязательна дедупликация по `eventId`/`seq` |
| Потеря событий | Возможна при скрытом/перезагруженном WebView; авторитетное состояние — команда |
| Буферизация | Только ограниченные ring buffer-ы (консоль 5 000 строк); без бесконечных очередей |
| Размер | ≤ 16 КиБ на событие; консольный батч ≤ 32 КиБ / 200 строк |

`TauriEventSink` сериализует публикацию через один канал (`mpsc`) и задачу
эмиссии. Это исключает перемешивание `seq` и позволяет троттлить.

### 3.1. Троттлинг и батчинг

| Поток | Частота | Механизм |
|---|---|---|
| `job:progress` | ≤ 10 Гц на job | коалесинг последнего значения |
| `download:progress` | ≤ 5 Гц на задачу, агрегат ≤ 10 Гц | коалесинг |
| `server:console` | ≤ 10 Гц | батч до 200 строк |
| `minecraft:log` | ≤ 10 Гц | батч до 200 строк |
| `server:metrics` | 1 Гц | таймер |
| `server:state-changed` | по факту | без троттлинга |
| `instance:*`, `auth:*`, `nexus:*` | по факту | без троттлинга |
| `diagnostics:health-changed` | ≤ 1 Гц | коалесинг |

Если значение не изменилось, событие не публикуется (кроме heartbeat
метрик).

### 3.2. Отмена и завершение

- `job:cancelled` публикуется после фактической остановки работы, а не в
  момент запроса.
- При shutdown приложение не публикует «фальшивый» `completed`; UI при
  следующем старте видит `interrupted` через `jobs_list`.
- События, отправленные после `app:shutdown`, отбрасываются.

---

## 4. Каталог событий

### 4.1. Приложение, БД, конфигурация

| Событие | Когда | Payload |
|---|---|---|
| `app:ready` | окно готово, сервисы инициализированы | `{ version, ipcSchemaVersion, dbSchemaVersion }` |
| `app:error` | ошибка вне команды (фон, старт) | `{ error: ErrorDto }` |
| `app:shutdown` | начато корректное завершение | `{ reason }` |
| `app:settings-changed` | настройки применены | `{ changedKeys: string[], requiresRestart: boolean }` |
| `db:ready` | БД открыта и мигрирована | `{ schemaVersion, journalMode }` |
| `db:migration-progress` | применяется миграция | `{ from, to, current }` |
| `db:degraded` | БД восстановлена/работает ограниченно | `{ reason, recoveredFromBackup }` |
| `security:degraded` | keyring недоступен или политика ослаблена | `{ component: "keyring"|"tls"|"url", message }` |

`app:settings-changed` не содержит значений секретов; только ключи.

### 4.2. Job

| Событие | Когда | Payload |
|---|---|---|
| `job:started` | работа началась | `{ kind, entity, startedAt }` |
| `job:progress` | прогресс/стадия | `{ stage, done, total, unit, message? }` |
| `job:finished` | успех | `{ outcome: "succeeded", durationMs }` |
| `job:failed` | ошибка | `{ error: ErrorDto }` |
| `job:cancelled` | отмена завершена | `{ durationMs }` |

`unit` — `bytes`, `files`, `steps`, `percent`. `total` может быть `null` для
неопределённого прогресса.

### 4.3. Инстансы

| Событие | Когда | Payload |
|---|---|---|
| `instance:created` | манифест записан | `{ instanceId, name, gameVersion, loader }` |
| `instance:updated` | изменены настройки/имя/иконка | `{ instanceId, changed: string[] }` |
| `instance:deleted` | каталог удалён | `{ instanceId }` |
| `instance:install-progress` | установка MC/loader | `{ instanceId, stage, done, total }` |
| `instance:installed` | установка завершена | `{ instanceId, versionId, javaMajor }` |
| `instance:validated` | проверка целостности завершена | `{ instanceId, missing, modified, unexpected, lockMismatch }` |
| `instance:exported` | экспорт инстанса завершён | `{ instanceId, format, sizeBytes }` |
| `instance:validation-failed` | найдены расхождения | `{ instanceId, missing, modified, unexpected }` |

### 4.4. Minecraft: метаданные, установка, Java

| Событие | Когда | Payload |
|---|---|---|
| `minecraft:catalog-updated` | каталог обновлён | `{ source: "mojang", versionCount, stale }` |
| `minecraft:metadata-updated` | version JSON/asset index обновлён | `{ versionId, stale }` |
| `minecraft:install-started` | начата установка версии | `{ versionId, totalFiles, totalBytes }` |
| `minecraft:install-progress` | прогресс | `{ versionId, stage, filesDone, filesTotal, bytesDone, bytesTotal }` |
| `minecraft:install-completed` | установка успешна | `{ versionId, installed, skipped, bytesDownloaded }` |
| `minecraft:install-failed` | ошибка установки | `{ versionId, error: ErrorDto }` |
| `minecraft:verify-failed` | проверка целостности провалилась | `{ versionId, files: string[] }` |
| `java:runtimes-changed` | список runtime изменился | `{ added: string[], removed: string[] }` |
| `java:install-progress` | установка Java | `{ major, stage, bytesDone, bytesTotal }` |
| `java:install-completed` | Java установлена | `{ runtimeId, major, vendor }` |
| `java:install-failed` | ошибка установки Java | `{ major, error: ErrorDto }` |

`files: string[]` в `verify-failed` — относительные пути, не абсолютные.

### 4.5. Запуск игры

| Событие | Когда | Payload |
|---|---|---|
| `minecraft:launching` | preflight пройден, spawn начат | `{ instanceId, runId, versionId }` |
| `minecraft:started` | процесс жив, получен первый вывод/подтверждение | `{ instanceId, runId, pid }` |
| `minecraft:log` | батч строк | `{ runId, lines: [{ seq, ts, stream, text }] }` |
| `minecraft:stopped` | штатное завершение | `{ instanceId, runId, exitCode, durationMs }` |
| `minecraft:crashed` | ненулевой exit/crash report | `{ instanceId, runId, exitCode, crashReportId? }` |

`stream` — `stdout|stderr|launcher`. `text` — уже redacted, без токенов.

### 4.6. Загрузчики

| Событие | Когда | Payload |
|---|---|---|
| `loader:install-progress` | установка загрузчика | `{ instanceId, loader, stage, done, total }` |
| `loader:installed` | profile готов | `{ instanceId, loader, loaderVersion, versionId }` |
| `loader:install-failed` | ошибка | `{ instanceId, loader, error: ErrorDto }` |

### 4.7. Аккаунты и авторизация

| Событие | Когда | Payload |
|---|---|---|
| `auth:device-code` | получен код устройства | `{ challengeId, userCode, verificationUri, expiresAt }` |
| `auth:login-completed` | вход успешен | `{ accountId, provider, username, uuid }` |
| `auth:login-failed` | вход не удался/отменён | `{ challengeId, error: ErrorDto }` |
| `auth:account-updated` | профиль/статус/токен обновлён | `{ accountId, status }` |
| `auth:account-removed` | аккаунт удалён | `{ accountId }` |

Токены, коды и refresh-значения в payload **отсутствуют**.

### 4.8. Моды и модпаки

| Событие | Когда | Payload |
|---|---|---|
| `mods:install-progress` | установка модов | `{ instanceId, stage, done, total }` |
| `mods:changed` | набор файлов изменился | `{ instanceId, added, removed, enabled, disabled }` |
| `mods:updates-checked` | проверка обновлений завершена | `{ instanceId, available: [{ projectId, current, latest }] }` |
| `mods:operation-failed` | ошибка вне job-а | `{ instanceId, error: ErrorDto }` |
| `modpacks:import-progress` | импорт | `{ inspectionId, instanceId?, stage, done, total }` |
| `modpacks:imported` | импорт завершён | `{ instanceId, format, name }` |
| `modpacks:exported` | экспорт завершён | `{ instanceId, format, sizeBytes }` |
| `modpacks:operation-failed` | ошибка | `{ inspectionId?, instanceId?, error: ErrorDto }` |

### 4.9. Загрузки

| Событие | Когда | Payload |
|---|---|---|
| `download:queued` | задача создана | `{ taskId, kind, label, filesTotal, bytesTotal }` |
| `download:started` | первая запись началась | `{ taskId, startedAt }` |
| `download:progress` | прогресс | `{ taskId, filesDone, filesTotal, bytesDone, bytesTotal, speedBps, etaSeconds? }` |
| `download:paused` | пауза | `{ taskId, bytesDone, bytesTotal }` |
| `download:completed` | все обязательные файлы готовы | `{ taskId, filesSkipped, bytesDownloaded, durationMs }` |
| `download:failed` | ошибка | `{ taskId, error: ErrorDto }` |
| `download:cancelled` | отмена | `{ taskId }` |
| `download:removed` | запись удалена | `{ taskId }` |

`download:completed` не означает «установка завершена»: это отдельные
события `minecraft:install-completed`, `mods:changed` и т.д.

### 4.10. Серверы

| Событие | Когда | Payload |
|---|---|---|
| `server:created` | манифест создан | `{ serverId, name, gameVersion, loader }` |
| `server:installed` | серверный дистрибутив установлен | `{ serverId, kind, versionId }` |
| `server:starting` | spawn начат | `{ serverId, runId }` |
| `server:started` | процесс жив | `{ serverId, runId, pid }` |
| `server:console` | батч строк | `{ serverId, runId, lines: [{ seq, ts, stream, text }] }` |
| `server:stopped` | штатная остановка | `{ serverId, runId, exitCode, durationMs }` |
| `server:crashed` | аварийное завершение | `{ serverId, runId, exitCode, autoRestartScheduled }` |
| `server:metrics` | раз в секунду | `{ serverId, cpuPercent, memoryBytes, playersOnline, uptimeSeconds }` |
| `server:state-changed` | смена состояния | `{ serverId, from, to, consoleReadOnly }` |
| `server:auto-restart-scheduled` | запланирован рестарт | `{ serverId, attempt, delaySeconds }` |
| `server:players-changed` | изменился состав игроков | `{ serverId, online, max }` |

`server:console` после перезапуска лаунчера может быть `readOnly=true`; это
отражается в `server:state-changed` (`consoleReadOnly`) и в
`servers_status`.

### 4.11. NEXUS Workspace

| Событие | Когда | Payload |
|---|---|---|
| `nexus:source-status` | проверка источника | `{ sourceId, status, latencyMs? }` |
| `nexus:catalog-updated` | каталог обновлён | `{ sourceId, channel, packCount }` |
| `nexus:install-progress` | установка сборки | `{ instanceId?, sourceId, packId, stage, done, total }` |
| `nexus:installed` | сборка установлена | `{ instanceId, sourceId, packId, release }` |
| `nexus:updated` | сборка обновлена | `{ instanceId, sourceId, packId, from, to }` |
| `nexus:update-available` | найдено обновление | `{ instanceId, current, latest, channel }` |

Событие `nexus:update-available` не запускает обновление.

### 4.12. Бэкапы и диагностика

| Событие | Когда | Payload |
|---|---|---|
| `backup:started` | бэкап начат | `{ backupId, target, mode }` |
| `backup:progress` | прогресс | `{ backupId, filesDone, filesTotal, bytesDone, bytesTotal }` |
| `backup:completed` | бэкап готов | `{ backupId, sizeBytes, durationMs }` |
| `backup:failed` | ошибка | `{ backupId, error: ErrorDto }` |
| `backup:verified` | проверка архива завершена | `{ backupId, valid, entries }` |
| `backup:restored` | восстановление завершено | `{ backupId, target, preRestoreBackupId }` |
| `diagnostics:export-progress` | экспорт пакета | `{ exportId, stage, done, total }` |
| `diagnostics:export-ready` | пакет готов | `{ exportId, sizeBytes, fileName }` |
| `diagnostics:health-changed` | изменился health-статус | `{ status, checks: string[] }` |

`fileName` — только имя, не полный путь.

---

## 5. Примеры payload

### 5.1. `download:progress`

```json
{
  "schemaVersion": 1,
  "name": "download:progress",
  "eventId": "9f1c...",
  "occurredAt": 1795274960123,
  "jobId": "0193...",
  "entity": { "kind": "download", "id": "0193..." },
  "seq": 41,
  "payload": {
    "taskId": "0193...",
    "filesDone": 41,
    "filesTotal": 124,
    "bytesDone": 25000000,
    "bytesTotal": 60500000,
    "speedBps": 4200000,
    "etaSeconds": 8
  }
}
```

### 5.2. `server:console`

```json
{
  "schemaVersion": 1,
  "name": "server:console",
  "eventId": "4a2e...",
  "occurredAt": 1795274960200,
  "entity": { "kind": "server", "id": "0193..." },
  "seq": 1201,
  "payload": {
    "serverId": "0193...",
    "runId": "0193...",
    "lines": [
      { "seq": 1200, "ts": 1795274960150, "stream": "stdout", "text": "[Server thread/INFO]: Done (2.341s)!" },
      { "seq": 1201, "ts": 1795274960190, "stream": "stdout", "text": "[Server thread/INFO]: Player joined the game" }
    ]
  }
}
```

### 5.3. `job:failed`

```json
{
  "schemaVersion": 1,
  "name": "job:failed",
  "eventId": "77aa...",
  "occurredAt": 1795274961000,
  "jobId": "0193...",
  "entity": { "kind": "instance", "id": "0193..." },
  "payload": {
    "error": {
      "code": "INTEGRITY_MISMATCH",
      "message": "Файл библиотеки повреждён. Повторите установку.",
      "details": { "file": "libraries/…/example.jar" },
      "retryable": true,
      "requestId": "2f9c..."
    }
  }
}
```

В `details.file` — относительный путь, не абсолютный.

---

## 6. Frontend-интеграция

### 6.1. Один подписчик

```ts
// app/events.ts
import { listen } from "@tauri-apps/api/event";
import { queryClient } from "@/app/queryClient";

const invalidation: Record<string, string[][]> = {
  "instance:created": [["instances"]],
  "instance:updated": [["instances"], ["instance"]],
  "instance:deleted": [["instances"]],
  "minecraft:install-completed": [["instances"], ["instance"]],
  "mods:changed": [["mods", "installed"], ["instance"]],
  "server:state-changed": [["servers"], ["server"]],
  "nexus:catalog-updated": [["nexus", "catalog"]],
  "backup:completed": [["backups"]],
};

export async function initEvents() {
  for (const [name, keys] of Object.entries(invalidation)) {
    await listen(name, () => {
      for (const key of keys) queryClient.invalidateQueries({ queryKey: key });
    });
  }
}
```

Правила:

- `listen` вызывается один раз на приложение;
- после `job:finished` инвалидируются ключи по `kind`/`entity`;
- прогресс обновляет локальный store, но не пишется в TanStack Query;
- консоль хранится в ring buffer store, ограниченном 5 000 строк;
- при `visibilitychange → visible` выполняется добор состояния командами
  (`jobs_list`, `servers_status`, `downloads_list`).

### 6.2. Дедупликация

- `eventId` — для логов и тестов;
- `seq` — для потоков; UI отбрасывает `seq <= lastSeq` по
  `(name, entity)`;
- после перезагрузки WebView `seq` начинается заново — буфер сбрасывается.

### 6.3. Ошибки

- `app:error` показывает глобальный toast;
- `job:failed` показывает ошибку у операции и не дублирует toast;
- повторяющиеся `RATE_LIMITED`/`OFFLINE` показываются один раз с таймером.

---

## 7. Безопасность событий

Запрещено в payload:

- access/refresh токены, коды авторизации, XSTS-токены;
- пароли, `client_secret`;
- полные HTTP-заголовки;
- абсолютные пути пользователя (только `displayName`, относительные пути или
  сокращённые `%LOCALAPPDATA%\...`);
- содержимое файлов конфигурации;
- SQL/стектрейсы.

Обязательно:

- redaction на уровне `TracingLayer` и на уровне сборки payload;
- тест «в событиях нет известного секрета» для auth/launch/server;
- ограничение размера строки консоли (8 КиБ) с обрезкой и маркером
  `truncated: true`.

---

## 8. Тестирование событий

| Тест | Что проверяется |
|---|---|
| Envelope schema | Обязательные поля, типы, `schemaVersion` |
| Registry snapshot | Полный список имён не меняется случайно |
| Throttle | 1 000 progress-обновлений → ≤ 10 событий/с |
| Coalescing | Последнее значение не теряется |
| Console batching | Порядок `seq`, лимит строк, размер батча |
| Dedup | UI отбрасывает повторный `seq` |
| No secrets | Токены/коды не появляются в payload |
| Redaction paths | Нет абсолютных путей в ошибках |
| Ordering | `job:started` → `progress` → `finished` для одного job |
| Cancellation | `job:cancelled` после остановки, не раньше |
| Shutdown | Нет событий после `app:shutdown` |
| Hidden window | Пропуск событий не ломает восстановление состояния |

Схемы событий экспортируются в `docs/schemas/events-1.json` и проверяются
contract-тестом, как и IPC DTO.

---

## 9. Полный список имён (сводка)

```text
app:ready
app:error
app:shutdown
app:settings-changed

db:ready
db:migration-progress
db:degraded
security:degraded

job:started
job:progress
job:finished
job:failed
job:cancelled

instance:created
instance:updated
instance:deleted
instance:install-progress
instance:installed
instance:validated
instance:exported
instance:validation-failed

minecraft:catalog-updated
minecraft:metadata-updated
minecraft:install-started
minecraft:install-progress
minecraft:install-completed
minecraft:install-failed
minecraft:verify-failed
minecraft:launching
minecraft:started
minecraft:log
minecraft:stopped
minecraft:crashed

java:runtimes-changed
java:install-progress
java:install-completed
java:install-failed

loader:install-progress
loader:installed
loader:install-failed

auth:device-code
auth:login-completed
auth:login-failed
auth:account-updated
auth:account-removed

mods:install-progress
mods:changed
mods:updates-checked
mods:operation-failed

modpacks:import-progress
modpacks:imported
modpacks:exported
modpacks:operation-failed

download:queued
download:started
download:progress
download:paused
download:completed
download:failed
download:cancelled
download:removed

server:created
server:installed
server:starting
server:started
server:console
server:stopped
server:crashed
server:metrics
server:state-changed
server:auto-restart-scheduled
server:players-changed

nexus:source-status
nexus:catalog-updated
nexus:install-progress
nexus:installed
nexus:updated
nexus:update-available

backup:started
backup:progress
backup:completed
backup:failed
backup:verified
backup:restored

diagnostics:export-progress
diagnostics:export-ready
diagnostics:health-changed
```
