# NEXUS WORKSPACE API

Документ описывает HTTP/WebSocket API версии `v1`. Источник истины по составу
маршрутов — сгенерированная спецификация [`openapi.json`](openapi.json),
доступная на работающем сервере как `GET /api/v1/openapi.json`. Таблица
эндпоинтов в конце документа генерируется из этой спецификации.

Связанные документы: [`ARCHITECTURE.md`](ARCHITECTURE.md),
[`openapi.json`](openapi.json). Схема манифеста релиза описана в
[`MANIFEST.md`](MANIFEST.md), машиночитаемый контракт —
[`schemas/manifest-1.0.json`](schemas/manifest-1.0.json).

---

## 1. Базовый префикс и версионирование

* Все методы API находятся под префиксом `/api/v1`.
* Версия передаётся в пути. Отдельного заголовка версии нет.
* Обратно совместимые изменения (новые поля, новые необязательные параметры,
  новые маршруты) не меняют номер версии.
* Несовместимые изменения оформляются как `/api/v2`; параллельная работа
  `v1` и `v2` допускается.
* Поле `info.version` в OpenAPI — версия сборки backend (`CARGO_PKG_VERSION`),
  а не версия протокола. Версия протокола — `v1` в пути.
* `GET /api/v1/openapi.json` — публичный, без аутентификации.
* Swagger UI `GET /api/v1/docs` по умолчанию **выключен** и отвечает `404`.
  Он включается только переменной окружения `NEXUS_ENABLE_DOCS=true`
  (значения `1`/`true`/`yes`). Страница использует CDN Swagger UI; при strict
  CSP разрешите источник CDN или пользуйтесь JSON-спецификацией.

---

## 2. Формат ответа

### 2.1 Успешный ответ

Все JSON-методы возвращают объект:

```json
{
  "success": true,
  "data": { }
}
```

`data` может быть объектом, массивом, числом или `null` в зависимости от метода.
Успешные JSON-ответы имеют статус `200`. Исключения:

* скачивание файла — `200` или `206 Partial Content` при `Range`;
* WebSocket — `101 Switching Protocols`;
* ошибки — соответствующий код `4xx`/`5xx`.

### 2.2 Ошибка

```json
{
  "success": false,
  "error": {
    "code": "FILE_NAME_CONFLICT",
    "message": "Файл с таким именем уже существует в этой папке.",
    "details": {
      "existingFileId": "6f1c2f5e-5f0e-4f7c-9c2e-2a3b4c5d6e7f"
    }
  },
  "request_id": "3f0f3c4e8a7d4f5b9c1e2d3a4b5c6d7e"
}
```

* `error.code` — стабильный машиночитаемый код (см. §8).
* `error.message` — безопасное сообщение для пользователя; технические детали
  остаются в журнале сервера.
* `error.details` — объект с контекстом (`offset`, `maxBytes`,
  `required_permission`, `retry_after_seconds`, `existingFileId`, `module`
  и т. п.) или `null`.
* Тело ошибки всегда содержит `request_id`.

### 2.3 `request_id`

* Сервер принимает `X-Request-Id` длиной до 128 символов из
  `[A-Za-z0-9_-]`. Некорректное значение игнорируется.
* Если заголовок не передан, генерируется UUID v4.
* Значение возвращается в заголовке `X-Request-Id` любого ответа.
* В теле ошибки `request_id` совпадает с заголовком.
* Успешные ответы не дублируют `request_id` в теле — используйте заголовок.
* Для диагностики передавайте свой `X-Request-Id` и сообщайте его администратору.

---

## 3. Аутентификация и авторизация

### 3.1 Браузер: cookie-сессия и CSRF

1. `POST /api/v1/auth/login` с `{"username":"…","password":"…"}`.
2. Сервер создаёт серверную сессию и устанавливает cookie:

   ```text
   Set-Cookie: nexus_session=<token>; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400[; Secure]
   ```

   `Secure` включается настройкой `cookie_secure` (рекомендуется для HTTPS).
   В БД хранится только SHA-256 хеш токена.
3. Ответ `data` содержит `csrfToken`, `user` (id, username, displayName,
   isSuperadmin, roles, permissions), `session.expiresAt` и `modules`.
4. Все изменяющие запросы (`POST`, `PUT`, `PATCH`, `DELETE`) обязаны передавать
   заголовок `X-CSRF-Token` со значением `data.csrfToken`. CSRF-токен —
   HMAC-SHA256 от токена сессии; он не хранится отдельно.
5. `GET /api/v1/auth/me` возвращает тот же `csrfToken` (можно обновить после
   перезагрузки страницы) и актуальные права.
6. Сессия продлевается при каждом обращении (скользящий TTL). `POST
   /api/v1/auth/logout` и `POST /api/v1/auth/sessions/revoke-all` завершают
   сессии и очищают cookie.

Исключения из CSRF: `POST /auth/login`, `GET /auth/setup-status`,
`GET /health`, `POST /launcher/auth/token`, `POST /launcher/auth/revoke`.
Все остальные изменяющие запросы без cookie или CSRF получают
`401 SESSION_EXPIRED` / `403 CSRF_INVALID`.

Права проверяются на сервере в каждом обработчике. Недостаточное право даёт
`403 FORBIDDEN` с `details.required_permission`. Суперадмин имеет все права.

### 3.2 Лаунчер: Bearer-токен и scopes

Лаунчер не использует cookie и CSRF.

1. `POST /api/v1/launcher/auth/token` (без сессии и CSRF) с логином, паролем,
   именем клиента, необязательными `scopes` и `expiresInHours`.

   ```json
   {
     "name": "nexus-launcher",
     "username": "player",
     "password": "…",
     "scopes": ["launcher.catalog", "launcher.artifacts"],
     "expiresInHours": 720
   }
   ```

   По умолчанию выдаются оба scope, срок — 720 часов (30 дней); допустимо
   от 1 до 8760 часов. В БД хранится только SHA-256 токена.
2. Токен передаётся одним из способов:

   ```http
   Authorization: Bearer nxl_…
   ```

   или

   ```http
   X-Launcher-Token: nxl_…
   ```

3. `launcher.catalog` разрешает каталог, сборки, релизы и манифесты.
   `launcher.artifacts` разрешает скачивание артефактов. При отсутствии scope —
   `403 LAUNCHER_SCOPE_REQUIRED` с `details.requiredScope`.
4. Ошибки токена: `401 LAUNCHER_TOKEN_REQUIRED`, `LAUNCHER_TOKEN_INVALID`,
   `LAUNCHER_TOKEN_REVOKED`, `LAUNCHER_TOKEN_EXPIRED`.
5. `POST /api/v1/launcher/auth/revoke` отзывает предъявленный токен.
   Администратор управляет токенами через `/api/v1/admin/launcher/tokens`
   (право `launcher.manage`).

Токен лаунчера не подходит для браузерных методов, а cookie-сессия — для
`/launcher/*`. Это разные схемы безопасности (`components.securitySchemes`).

#### 3.2.1 Приватные сборки

Токен действует от имени пользователя, который указан при его выдаче
(`access_tokens.user_id`). Сборка с `visibility = "private"` видна только
владельцу (`minecraft_builds.owner_id`).

Для чужого токена все методы лаунчера, которые принимают `slug` или
идентификатор артефакта, отвечают **404 `NOT_FOUND`**, а не 403: существование
приватной сборки не раскрывается. Это относится к:

| Метод | Поведение для чужой приватной сборки |
|---|---|
| `GET /api/v1/launcher/catalog` | сборка отсутствует в `builds` |
| `GET /api/v1/launcher/builds` | сборка отсутствует в `items` |
| `GET /api/v1/launcher/builds/{slug}` | `404 NOT_FOUND` |
| `GET /api/v1/launcher/builds/{slug}/icon` | `404 NOT_FOUND` |
| `GET /api/v1/launcher/builds/{slug}/releases` | `404 NOT_FOUND` |
| `GET /api/v1/launcher/builds/{slug}/latest/manifest` | `404 NOT_FOUND` |
| `GET /api/v1/launcher/builds/{slug}/releases/{version}/manifest` | `404 NOT_FOUND` |
| `GET /api/v1/launcher/artifacts/{id}` | `404 NOT_FOUND` |
| `POST /api/v1/launcher/auth/token` | выдаёт токен пользователю; данные сборок не отдаёт |
| `POST /api/v1/launcher/auth/revoke` | отзывает только предъявленный токен |

Публичные сборки (`visibility = "public"`) доступны любому действующему токену
с нужным scope. Приватная сборка остаётся доступной владельцу и после смены
`currentReleaseId`, а также для устаревших релизов (`deprecated`). Релизы,
снятые с публикации (`unpublished`), не отдаются ни в каталоге, ни в версиях,
ни в манифестах и файлах; их снимок можно опубликовать снова. Снять сборку с
публикации целиком можно через `POST /api/v1/minecraft/builds/{id}/unpublish`.

Административные `GET /api/v1/admin/launcher/tokens` и
`DELETE /api/v1/admin/launcher/tokens/{id}` управляют токенами по праву
`launcher.manage` и не возвращают содержимое сборок.

#### 3.2.2 Пути в манифесте и артефактах

`path` в манифесте и `Content-Disposition` артефакта — переносимый
относительный путь. Сервер отклоняет:

* абсолютные пути и UNC (`/etc/...`, `//server/share/...`);
* буквы дисков (`C:/...`, `C:file.jar`);
* обратные слэши;
* сегменты `..` и `.`;
* ADS-двоеточия (`file.jar:stream`);
* зарезервированные имена устройств Windows (`CON`, `NUL`, `COM1`, `LPT9`);
* имена, заканчивающиеся точкой или пробелом;
* управляющие символы и `< > " | ? *`;
* пути не в Unicode NFC.

Внутри одной сборки пути сравниваются без учёта регистра и нормализации
Unicode: `Mods/A.jar` и `mods/a.jar` — одна и та же запись на Windows, поэтому
такой релиз не публикуется. Артефакт с небезопасным путём не отдаётся
(`404 NOT_FOUND`).

#### 3.2.3 Репозиторий MyLauncher (публичный)

Stock MyLauncher 0.1.0 **не отправляет токен** и запрещает query/учётные
данные в адресе репозитория. Поэтому эти методы доступны без аутентификации
и отвечают **без** обёртки `{success,data}` — ровно как читает клиент:

| Метод | Путь | Ответ |
|---|---|---|
| `GET` | `/api/packs?channel=stable\|beta` | `{"packs":[…]}` |
| `GET` | `/api/packs/{packId}/versions?channel=…` | `{"versions":[…]}` |
| `GET` | `/api/packs/{packId}/versions/{version}/manifest?channel=…` | `manifestVersion: 1` |
| `GET` | `/api/packs/{packId}/icon` | изображение/404 |
| `GET` | `/api/files/sha256:{hex}?packId=&version=&channel=` | файл, Range |

Если клиент всё же пришлёт `Authorization: Bearer`, токен проверяется и
открывает приватные сборки владельца (`launcher.catalog`/`launcher.artifacts`).
Отозванный токен → `401`. Лимит — по токену или IP, `429` с `Retry-After`.
Приватные сборки stock-клиенту недоступны; контракт и ограничения —
[`LAUNCHER_CONTRACT.md`](LAUNCHER_CONTRACT.md).

### 3.4 Публичные ссылки

`GET /api/v1/public/shares/{token}` возвращает сведения о ссылке без сессии.
Для скачивания по ссылке с паролем:

1. `POST /api/v1/public/shares/{token}/unlock` с телом
   `{ "password": "…" }` — пароль передаётся только в теле запроса;
2. ответ `200` содержит `{ "ticket": "…", "expiresAt": "…" }`;
3. `GET /api/v1/public/shares/{token}/download?ticket=…` отдаёт файл.

Тикет живёт 1 час, привязан к `password_hash` ссылки и к её token: смена
пароля или использование тикета с другой ссылкой дают
`403 SHARE_TICKET_INVALID`. `Range` с тикетом возвращает `206` и **не**
увеличивает `downloadCount`.

Ошибки `unlock`: `403 SHARE_PASSWORD_INVALID`, `410 DOWNLOAD_LIMIT_REACHED`,
`429 RATE_LIMITED` с `Retry-After` (5 ошибок за 15 минут на `share_id|IP` и
10 запросов в минуту на IP).

Совместимый путь без тикета: `GET /download` с заголовком
`X-Share-Password` (оставлен на один релиз). Такой запрос атомарно учитывает
одно скачивание; `Range` не обходит `maxDownloads`. Параметр `?password=`
удалён — пароль не должен попадать в URL, Referer и access log.

Ссылка, созданная с `authOnly: true`, требует действующую cookie-сессию
`nexus_session` для `GET`, `unlock` и `download`. Гость получает
`401 SHARE_AUTH_REQUIRED` (`details.authRequired = true`), а отозванная или
истёкшая сессия — `401 SESSION_EXPIRED`.

### 3.4 Отключённые модули

Если модуль `cloud` или `minecraft` отключён администратором, его API отвечает
`403 MODULE_DISABLED` с `details.module`. Для внешнего `/api/v1/launcher/*`
причина не раскрывается: возвращается `404 NOT_FOUND`; административный
`/api/v1/admin/launcher/*` работает по общему правилу (`403 MODULE_DISABLED`).
Авторизация, системные методы, мониторинг, публичные ссылки и WebSocket
продолжают работать.

---

## 4. Пагинация, фильтрация, сортировка

### 4.1 Пагинация

Для списочных методов используется серверная пагинация:

| Параметр | Тип | По умолчанию | Ограничения |
|---|---|---|---|
| `page` | integer | `1` | минимум `1` |
| `perPage` | integer | `25` (Cloud, Users, Audit) или `24` (Minecraft) | `1..100` |

Ответ:

```json
{
  "success": true,
  "data": {
    "items": [],
    "page": 1,
    "perPage": 25,
    "total": 0
  }
}
```

`Cloud` дополнительно возвращает `breadcrumb` (путь до текущей папки) и
`usage` (`usedBytes`, `fileCount`). `GET /api/v1/jobs` использует не страницы,
а `limit` (`1..200`, по умолчанию `50`).

### 4.2 Фильтры и сортировка

| Метод | Параметры |
|---|---|
| `GET /api/v1/cloud/entries` | `page`, `perPage`, `parentId`, `q`, `sort` (`name`, `size`, `updated`, `type`), `direction` (`asc`, `desc`), `view` (`folder`, `trash`, `starred`, `recent`, `all`), `entryType` (`file`, `folder`) |
| `GET /api/v1/users` | `page`, `perPage`, `q`, `status` (`active`, `disabled`) |
| `GET /api/v1/minecraft/builds` | `page`, `perPage`, `q`, `status`, `visibility`, `loader`, `category`, `sort` (`updated`, `name`, `created`, `size`, `files`), `direction` |
| `GET /api/v1/system/audit` | `page`, `perPage`, `eventType` |
| `GET /api/v1/jobs` | `status`, `jobType`, `limit` |
| `GET /api/v1/monitoring/history` | `window` (`5m`, `15m`, `1h`, `24h`, `7d`) |

Поиск `q` нечувствителен к регистру; символы `%` и `_` экранируются.
Неизвестные значения `sort`/`direction`/`view` заменяются значениями по
умолчанию. `Cloud` сортирует папки перед файлами, затем применяет выбранное
поле.

---

## 5. Возобновляемая загрузка

### 5.1 Обзор

Cloud Storage использует собственную реализацию с семантикой tus 1.0.0:
создание сессии → запись чанков по смещению → завершение. Данные пишутся
потоком сразу в staging-файл выбранного тома, в память не буферизуются.
Поддерживается продолжение после обрыва соединения: клиент узнаёт `offset`
и повторяет передачу с этого места.

```text
POST   /api/v1/cloud/uploads                       создать сессию
GET    /api/v1/cloud/uploads/{id}                  узнать offset/статус
PATCH  /api/v1/cloud/uploads/{id}                  записать чанк
POST   /api/v1/cloud/uploads/{id}/complete         зафиксировать файл
DELETE /api/v1/cloud/uploads/{id}                  отменить и очистить staging
GET    /api/v1/cloud/config                        лимиты и серверное время
```

Методы сессии загрузки требуют право `cloud.file.upload` и браузерную сессию
с CSRF; `GET /cloud/config` требует `cloud.view`.

### 5.2 Лимиты

`GET /api/v1/cloud/config` возвращает:

```json
{
  "maxUploadBytes": 10737418240,
  "maxUploadMb": 10240,
  "chunkMaxBytes": 67108864,
  "uploadTtlHours": 24,
  "uploadConcurrency": 3,
  "serverTime": "2026-11-19T09:00:00Z"
}
```

* `maxUploadBytes` — максимальный заявленный размер файла (по умолчанию 10 ГиБ).
* `chunkMaxBytes` — максимальный размер одного чанка (по умолчанию 64 МиБ).
* `uploadTtlHours` — срок жизни сессии (по умолчанию 24 часа).
* `uploadConcurrency` — рекомендуемое число параллельных загрузок браузера.
* Значения — эффективные, из `system_settings` (`limits.*`), а не сырые из
  `nexus.toml`; `GET /api/v1/system/settings` отдаёт их же в поле `effective`.
* Дополнительно проверяется квота пользователя (`QUOTA_EXCEEDED`) и свободное
  место тома (`INSUFFICIENT_SPACE`).
* Значения могут отличаться в конкретной установке; источник истины — этот
  endpoint и `GET /api/v1/cloud/uploads/{id}` (`chunkMaxBytes`).

### 5.3 Создание сессии

```http
POST /api/v1/cloud/uploads
Content-Type: application/json
X-CSRF-Token: …

{
  "name": "backup-2026.zip",
  "parentId": "8d1e…",
  "size": 5368709120,
  "contentType": "application/zip"
}
```

* `parentId` отсутствует или `null` — корень.
* `size` — точный заявленный размер; он обязателен и должен быть не больше
  `maxUploadBytes`.
* `contentType` принимается для совместимости, но **не доверяется**: при
  завершении MIME определяется по первым байтам файла и сохраняется в
  `storage_objects.content_type` / `cloud_entries.content_type`.
* Имя проверяется на недопустимые символы. Конфликт с существующей записью
  разрешается при завершении (`strategy`), а не при создании сессии.
* Ответ — `UploadSession` (`id`, `fileName`, `receivedBytes`, `status`,
  `expiresAt`, …). `receivedBytes` при создании равно `0`.

### 5.4 Статус и продолжение

```http
GET /api/v1/cloud/uploads/{id}
```

```json
{
  "id": "…",
  "offset": 268435456,
  "size": 5368709120,
  "status": "active",
  "expiresAt": "2026-11-20T09:00:00Z",
  "chunkMaxBytes": 67108864
}
```

Алгоритм возобновления:

1. Запросить `GET /uploads/{id}` после перезапуска клиента или обрыва.
2. Взять `offset` из ответа.
3. Открыть файл на позиции `offset` и продолжить `PATCH` с этим смещением.
4. Если сервер вернул `409 UPLOAD_OFFSET_MISMATCH`, взять `details.offset`
   (или повторно `GET`) и повторить с актуального смещения.
5. Если `status` не `active` — сессия завершена, отменена или истекла;
   начинать нужно с нового `POST /uploads`.

### 5.5 Запись чанка

```http
PATCH /api/v1/cloud/uploads/{id}
Upload-Offset: 268435456
Content-Type: application/octet-stream
X-CSRF-Token: …

<двоичные данные чанка>
```

* `Upload-Offset` обязателен и должен **точно** совпадать с текущим `offset`.
* Тело — сырые байты, не multipart и не base64.
* Размер чанка не больше `chunkMaxBytes`.
* Сервер пишет данные на диск, синхронизирует файл и обновляет сессию.
* Ответ:

  ```json
  { "success": true, "data": { "offset": 335544320 } }
  ```

* `PATCH` идемпотентен только при совпадении смещения: повтор того же чанка
  после успешной записи даст `409 UPLOAD_OFFSET_MISMATCH`. Используйте
  актуальный `offset`.
* При `413 CHUNK_TOO_LARGE` сервер записывает не больше лимита и возвращает
  `details.limitBytes` / `details.receivedBytes`; продолжение возможно с нового
  offset.
* При превышении заявленного `size` — `422 VALIDATION_ERROR`.
* Ошибки: `409 UPLOAD_NOT_ACTIVE`, `409 UPLOAD_OFFSET_MISMATCH`,
  `413 CHUNK_TOO_LARGE`, `422 VALIDATION_ERROR`.

### 5.6 Завершение

```http
POST /api/v1/cloud/uploads/{id}/complete
Content-Type: application/json
Idempotency-Key: 7f3c9a2e-upload-1
X-CSRF-Token: …

{ "strategy": "auto" }
```

`strategy`:

| Значение | Поведение |
|---|---|
| `auto` (по умолчанию) | при совпадении имени — `409 FILE_NAME_CONFLICT` |
| `replace` | заменить содержимое существующего файла |
| `rename` | использовать `newName` (обязателен) |

Перед фиксацией проверяется, что размер staging-файла равен заявленному.
MIME определяется по первым байтам (клиентский `contentType` игнорируется):
HTML/SVG не отдаются inline, JS и текст — как `text/plain`.
Ответ — созданная или обновлённая запись `CloudEntry`. После успеха сессия
получает `status = completed`, staging удаляется, квота пересчитывается.
Событие `upload.completed` отправляется в WebSocket.

Ошибки: `409 UPLOAD_INCOMPLETE` (`received`, `declared`),
`409 UPLOAD_DATA_MISSING`, `409 FILE_NAME_CONFLICT` (`existingFileId`),
`409 VOLUME_UNAVAILABLE`, `409 UPLOAD_NOT_ACTIVE`, `422 VALIDATION_ERROR`.

### 5.7 Отмена

```http
DELETE /api/v1/cloud/uploads/{id}
```

Удаляет staging-файл, переводит сессию в `aborted`. Повторная отмена уже
неактивной сессии возвращает `{ "aborted": true }` без ошибки.

### 5.8 TTL и очистка

* `expiresAt = createdAt + uploadTtlHours` (по умолчанию 24 часа).
* Просроченные сессии переводятся в `expired`, staging удаляется:
  при создании новой сессии и фоновой задачей обслуживания
  (`system.uploads.cleanup`).
* Продолжение просроченной сессии — `409 UPLOAD_NOT_ACTIVE`; начинайте заново.

### 5.9 `Idempotency-Key`

`Idempotency-Key` поддерживается для опасных операций:

* `POST /cloud/uploads/{id}/complete` — фиксация загрузки (включая
  `strategy=replace`);
* `PATCH /cloud/entries/{id}` — только при `strategy=replace` в теле;
* `POST /cloud/shares` — создание ссылки;
* `POST /cloud/exports` — создание ZIP-архива;
* `POST /minecraft/uploads/{id}/complete` — фиксация артефакта сборки.

Правила одинаковы для всех маршрутов:

* Длина 1..255, допустимые символы: ASCII-буквы, цифры, `-`, `_`, `.`, `:`.
* Первый запрос выполняется, результат сохраняется в `idempotency_keys`
  в области `actor + scope` (ключи разных пользователей независимы).
* Повтор с тем же ключом и тем же телом возвращает сохранённый ответ без
  повторного изменения состояния, аудита и realtime-событий.
* Тот же ключ с другим телом — `422 IDEMPOTENCY_KEY_REUSED`.
* Параллельный повтор во время выполнения — `409 IDEMPOTENCY_IN_PROGRESS`.
* Ошибка выполнения освобождает ключ, поэтому запрос можно безопасно повторить.
* Ключи старше 24 часов удаляет задача `system.idempotency.cleanup`.
  Планировщик ставит её в очередь при старте и далее раз в час; повторная
  постановка не создаёт дубликат.

Клиентская очередь загрузок создаёт `crypto.randomUUID()` один раз на
операцию фиксации и передаёт тот же ключ при автоматическом retry.

### 5.10 Загрузка артефактов Minecraft

Протокол тот же, но свои маршруты и поля:

```text
POST   /api/v1/minecraft/uploads                 создать сессию
GET    /api/v1/minecraft/uploads/{id}            offset/статус
PATCH  /api/v1/minecraft/uploads/{id}            чанк (Upload-Offset)
POST   /api/v1/minecraft/uploads/{id}/complete   зафиксировать файл
DELETE /api/v1/minecraft/uploads/{id}            отменить
GET    /api/v1/minecraft/uploads/config          лимиты
```

Создание:

```json
{
  "buildId": "…",
  "path": "mods/example.jar",
  "size": 12345678,
  "mode": "create",
  "category": "mods",
  "side": "client",
  "required": true,
  "description": "…",
  "contentType": "application/java-archive"
}
```

`mode = "replace"` заменяет существующий файл по тому же пути. Режим хранится
в сессии (`replace_existing`, миграция `0016`), поэтому пауза, продолжение с
серверного offset и повторная фиксация остаются заменой, а не создают дубль.
Требуется право `minecraft.artifacts.upload`. `contentType` принимается для
совместимости, но MIME определяется по первым байтам при завершении. Завершение
не принимает тело и поддерживает `Idempotency-Key` (повтор не создаёт второй
файл); ответ — запись файла сборки. Ошибки те же
(`UPLOAD_OFFSET_MISMATCH`, `UPLOAD_INCOMPLETE`, `CHUNK_TOO_LARGE`,
`UPLOAD_NOT_ACTIVE`, `VOLUME_UNAVAILABLE`), плюс `BUILD_PATH_IS_FOLDER`,
`BUILD_PATH_CONFLICT`, `BUILD_ROOT_IMMUTABLE`.

#### 5.10.1 Канонические пути сборки

`path` всегда относителен корня версии Minecraft. На каждой мутации (upload,
create folder, rename/move) сервер проверяет:

* только относительный путь с прямыми `/`, без `..`, диска, UNC, ADS;
* каждый символ — ASCII-буква/цифра или ` _./+@()-§` (политика MyLauncher);
* расширение из allowlist клиента (`.defaults` разрешён отдельно), кроме
  legacy-категорий `scripts`, `overrides`, `other`, которые сохраняются как есть;
* `options.txt` — только в корне, уникален без учёта регистра, заменяется через
  `mode=replace`;
* `mods` — плоско, только `.jar`; `shaderpacks`/`resourcepacks` — плоско, только
  `.zip`; `config` — любая вложенность, allowlist расширений;
* `versions/`, `assets/`, `libraries/`, `.launcher_meta.json*`, зарезервированные
  имена Windows и коллизии регистра/Unicode отклоняются с конкретной причиной.

Категория определяется путём: для канонических узлов запрошенная категория
игнорируется в пользу пути; для legacy-категорий явное значение сохраняется.
Корневые узлы скелета нельзя переименовать или удалить (`BUILD_ROOT_IMMUTABLE`).

Список файлов: `GET /builds/{id}/files?folder=config/create&q=&category=&side=`.
`folder` возвращает прямых детей папки; пустое значение — корень.

---

## 6. WebSocket

### 6.1 Подключение

```text
ws(s)://<host>/api/v1/ws
```

* Авторизация — **только cookie `nexus_session`**, полученная через
  `POST /auth/login`. Токен лаунчера `Bearer` не принимается.
* Без cookie или с истёкшей сессией — HTTP `401 SESSION_EXPIRED` до Upgrade.
* Прокси (Caddy) должен поддерживать `Upgrade`/`Connection`.
* Канал дополняет polling, а не заменяет его: при недоступности WebSocket
  клиент продолжает работать через обычные HTTP-методы.

### 6.2 Кадры

Сервер отправляет текстовые кадры с JSON-объектом события:

```json
{
  "kind": "upload.progress",
  "userId": "…",
  "payload": { "uploadId": "…", "uploaded": 1048576, "size": 5368709120 },
  "at": "2026-11-19T09:00:00Z"
}
```

* `kind` — тип события.
* `userId` — владелец события; `null` у системных событий.
* `payload` — данные события.
* `at` — время события (UTC, RFC 3339).
* Клиент не отправляет прикладных команд. Допустимы только стандартные
  Ping/Pong/Close; сервер отвечает Pong на Ping.

Первое сообщение после подключения:

```json
{
  "kind": "connected",
  "userId": "…",
  "system": true,
  "activeConnections": 1
}
```

`system: true` означает, что пользователь имеет `system.monitoring.view` и
будет получать системные события.

### 6.3 События

Полный каталог. `Адресация` — кому сервер доставляет кадр: `владельцу`
(`userId` события) или `системное` (только пользователям с
`system.monitoring.view`). Колонка `Источник` указывает модуль публикации.

| `kind` | Адресация | `payload` | Источник |
|---|---|---|---|
| `connected` | текущему соединению | `userId`, `system`, `activeConnections` | `realtime/mod.rs` |
| `lagged` | текущему соединению | `skipped` | `realtime/mod.rs` |
| `closed` | текущему соединению | `reason` (`connection_limit`) | `realtime/mod.rs` |
| `upload.progress` | владельцу загрузки | `uploadId`, `uploaded`, `size` | `cloud/uploads.rs` |
| `upload.completed` | владельцу загрузки | `uploadId`, `entryId`, `name`, `size` | `cloud/routes.rs` |
| `upload.failed` | владельцу загрузки | `uploadId`, `entryName?`, `code`, `message`, `uploaded`, `size` | `cloud/uploads.rs`, `cloud/routes.rs`, `minecraft/uploads.rs` |
| `download.progress` | владельцу файла | `entryId`, `name`, `transferred`, `size`, `percent` | `cloud/files.rs` |
| `download.completed` | владельцу файла | `entryId`, `name`, `transferred`, `size`, `percent` (100) | `cloud/files.rs` |
| `download.failed` | владельцу файла | `entryId`, `name`, `transferred`, `size`, `percent`, `code`, `message` | `cloud/files.rs` |
| `job.progress` | системное | `jobId`, `done`, `total`, `progress` | `system/jobs.rs` |
| `job.paused` | системное | `jobId`, `pausedAt` | `system/jobs_routes.rs`, `system/jobs.rs` |
| `job.resumed` | системное | `jobId` | `system/jobs_routes.rs` |
| `job.completed` | системное | `jobId`, `jobType`, `progress` | `system/jobs.rs` |
| `job.failed` | системное | `jobId`, `jobType`, `error` | `system/jobs.rs` |
| `job.cancelled` | системное | `jobId`, `jobType` | `system/jobs_routes.rs`, `system/jobs.rs` |
| `storage.volume.updated` | системное | `volumeId`, `name`, `state`, `previousState?`, `totalBytes`, `availableBytes` | `storage/routes.rs`, `system/jobs.rs` |
| `system.metrics.updated` | системное | `cpu`, `memoryUsed`, `memoryTotal`, `rxRate`, `txRate`, `requestsPerSecond`, `activeRequests`, `activeDownloads`, `activeWebSockets` | `system/monitoring.rs` |
| `build.validation.completed` | системное | `buildId`, `valid`, `errors` (число), `warnings` (число) | `minecraft/routes.rs` |
| `build.published` | системное | `buildId`, `releaseId`, `version` | `minecraft/routes.rs` |
| `build.publish.failed` | системное | `buildId`, `releaseId`, `code`, `message` | `minecraft/routes.rs` |
| `build.unpublished` | системное | `buildId` | `minecraft/routes.rs` |

`upload.failed` не содержит внутренних путей и SQL: только код, безопасное
сообщение и прогресс. Коды: `UPLOAD_DATA_MISSING`, `UPLOAD_INCOMPLETE`,
`UPLOAD_ABORTED`, `UPLOAD_EXPIRED`, `INTERNAL_ERROR` и другие безопасные коды
`ApiError`. Прогресс скачивания троттлится (не чаще 4 раз/с, для файлов от
16 МиБ — раз в 500 мс), финальный кадр `download.progress` всегда содержит
`percent: 100`; `download.completed` приходит при полной передаче,
`download.failed` — при обрыве до неё (`DOWNLOAD_INTERRUPTED`) или ошибке
чтения (`DOWNLOAD_FAILED`).

Пользователь получает только свои пользовательские события. Системные события
(с `userId: null`) получают только пользователи с `system.monitoring.view`.
Каждый `kind` из таблицы реально публикуется через `Hub::publish_user` /
`publish_system`, а адресация и доставка покрыты интеграционными тестами
`backend/tests/realtime_tests.rs` (включая успешные `upload.completed`,
`build.validation.completed` и `build.published`).

### 6.4 Ограничения

* Не более **3** одновременных соединений на пользователя. При открытии
  четвёртого старейшее соединение получает `closed` с
  `reason: "connection_limit"` и закрывается.
* Широковещательный буфер — 512 событий. Медленный клиент получает
  `lagged` со счётчиком пропущенных; после этого поток продолжается.
* Heartbeat: сервер отправляет Ping каждые 30 секунд. Клиент должен отвечать
  Pong (браузерный `WebSocket` делает это автоматически).
* Закрытие соединения клиентом или разрыв сети освобождает слот.
* WebSocket не обходит проверку отключённых модулей для HTTP-методов, но сам
  канал остаётся доступным для системных уведомлений.

---

## 7. Launcher API: лимиты и заголовки `RateLimit`

Лимиты считаются отдельно для каждого токена лаунчера (token bucket).
Каталог, сборки, релизы и манифесты:

| Параметр | Значение по умолчанию |
|---|---|
| Всплеск (ёмкость) | 120 запросов (равен минутному лимиту) |
| Пополнение | 120 запросов/минуту |
| Ключ | `access_tokens.id` |

Скачивание артефактов:

| Параметр | Значение по умолчанию |
|---|---|
| Всплеск (ёмкость) | 600 запросов (равен минутному лимиту) |
| Пополнение | 600 запросов/минуту |
| Одновременных потоков на токен | 8 |
| Ключ | `access_tokens.id` |

Значения задаются в `nexus.toml`:

```toml
[launcher]
requests_per_minute = 120
artifacts_per_minute = 600
max_parallel_downloads = 8
```

или переменными `NEXUS_LAUNCHER_RPM` и `NEXUS_LAUNCHER_ARTIFACTS_RPM`
(`max_parallel_downloads` — только TOML). Лимиты применяются при старте
службы; для Windows-службы переменные должны быть уровня машины или лежать в
`.env` каталога установки (см. [`DEPLOYMENT.md`](DEPLOYMENT.md)).

Успешные ответы лимитируемых методов содержат:

| Заголовок | Значение |
|---|---|
| `X-RateLimit-Limit` | ёмкость корзины |
| `X-RateLimit-Remaining` | сколько запросов осталось |
| `X-RateLimit-Reset` | секунд до восстановления одного токена (`0`, если токен уже есть) |

При превышении частоты:

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 1
X-RateLimit-Limit: 120
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1
```

```json
{
  "success": false,
  "error": {
    "code": "RATE_LIMITED",
    "message": "Слишком много попыток. Повторите позже.",
    "details": { "retry_after_seconds": 1 }
  },
  "request_id": "…"
}
```

При исчерпании параллельных скачиваний (`max_parallel_downloads` на токен)
возвращается `503` с `Retry-After`; слот освобождается сразу после конца
потока или обрыва соединения клиентом:

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 5
```

```json
{
  "success": false,
  "error": {
    "code": "DOWNLOAD_LIMIT_REACHED",
    "message": "Слишком много параллельных скачиваний. Повторите позже.",
    "details": { "maxParallel": 8, "retry_after_seconds": 5 }
  },
  "request_id": "…"
}
```

`POST /launcher/auth/token` ограничивается отдельным лимитером попыток входа
(по `username`) и также может вернуть `429 RATE_LIMITED` с `Retry-After`. `POST
/launcher/auth/revoke` не расходует квоту каталога/артефактов.

Логин браузера ограничивается по ключу `IP|username`: после серии неудач
возвращается `429 RATE_LIMITED` с `Retry-After` и `details.retry_after_seconds`;
после 10
неудачных попыток аккаунт блокируется на 15 минут (`423 ACCOUNT_LOCKED`).

---

## 8. Коды ошибок

`code` стабилен; `message` может уточняться. `details` необязателен.

### Общие и аутентификация

| Код | HTTP | Когда |
|---|---|---|
| `VALIDATION_ERROR` | 422 | Некорректные параметры, тело или значение |
| `NOT_FOUND` | 404 | Ресурс или метод не найден |
| `FORBIDDEN` | 403 | Нет права (`details.required_permission`) |
| `SESSION_EXPIRED` | 401 | Нет cookie, сессия истекла или отозвана |
| `INVALID_CREDENTIALS` | 401 | Неверный логин или пароль |
| `CSRF_INVALID` | 403 | Нет/неверен `X-CSRF-Token` |
| `ACCOUNT_DISABLED` | 403 | Учётная запись отключена |
| `ACCOUNT_LOCKED` | 423 | Временная блокировка после неудачных входов |
| `RATE_LIMITED` | 429 | Превышен лимит попыток/запросов |
| `MODULE_DISABLED` | 403 | Модуль отключён (`details.module`); для лаунчера — 404 |
| `INTERNAL_ERROR` | 500 | Внутренняя ошибка (детали только в журнале) |

### Лаунчер

| Код | HTTP | Когда |
|---|---|---|
| `LAUNCHER_TOKEN_REQUIRED` | 401 | Нет `Authorization: Bearer` / `X-Launcher-Token` |
| `LAUNCHER_TOKEN_INVALID` | 401 | Токен не найден |
| `LAUNCHER_TOKEN_REVOKED` | 401 | Токен отозван |
| `LAUNCHER_TOKEN_EXPIRED` | 401 | Истёк срок действия |
| `LAUNCHER_SCOPE_REQUIRED` | 403 | Нет нужного scope (`details.requiredScope`) |
| `DOWNLOAD_LIMIT_REACHED` | 503 | Исчерпан лимит параллельных скачиваний (`details.maxParallel`, `Retry-After`) |

### Cloud и загрузки

| Код | HTTP | Когда |
|---|---|---|
| `FILE_NAME_CONFLICT` | 409 | Имя занято без учёта регистра (`details.existingFileId`, если известен) |
| `PATH_TOO_LONG` | 422 | Полный путь длиннее 240 UTF-16 единиц (`details.maxLength`, `details.actualLength`) |
| `FILE_LOCKED` | 409 | ZIP-архив открыт другим процессом; удаление можно повторить |
| `FILE_TOO_LARGE` | 413 | Размер больше `maxUploadBytes` (`details.maxBytes`) |
| `QUOTA_EXCEEDED` | 413 | Превышена квота (лимит/использовано/запрошено) |
| `INSUFFICIENT_SPACE` | 507 | Нет места на томе |
| `VOLUME_UNAVAILABLE` | 409 | Том отключён/не online |
| `VOLUME_EXISTS` | 409 | Том с таким путём уже зарегистрирован |
| `VOLUME_NOT_EMPTY` | 409 | Том нельзя удалить: есть объекты |
| `OBJECT_UNAVAILABLE` | 409/404 | Объект отсутствует или повреждён |
| `NOT_IN_TRASH` | 409 | Восстановление записи вне корзины |
| `SHARE_PASSWORD_REQUIRED` | 403 | Публичная ссылка защищена паролем |
| `SHARE_AUTH_REQUIRED` | 401 | Ссылка создана с `authOnly`, нет cookie-сессии (`details.authRequired`) |
| `EXPORT_NOT_READY` | 409 | ZIP-архив ещё не готов |
| `EXPORT_NOT_FINISHED` | 409 | Удаление архива в статусе `pending`/`running`/`cancelled` |
| `UPLOAD_NOT_ACTIVE` | 409 | Сессия завершена, отменена или истекла |
| `UPLOAD_OFFSET_MISMATCH` | 409 | `Upload-Offset` не равен серверному (`details.offset`) |
| `UPLOAD_DATA_MISSING` | 409 | Staging-данные отсутствуют |
| `UPLOAD_INCOMPLETE` | 409 | Размер на диске не равен заявленному |
| `UPLOAD_PATH_MISMATCH` | 409 | Путь замены не совпадает с сессией |
| `CHUNK_TOO_LARGE` | 413 | Чанк больше `chunkMaxBytes` (`details.limitBytes`, `details.receivedBytes`) |
| `IDEMPOTENCY_KEY_REUSED` | 422 | Ключ уже использован с другим телом |
| `IDEMPOTENCY_IN_PROGRESS` | 409 | Операция с этим ключом ещё выполняется |

Имена Cloud проверяются по правилам Windows/NTFS:

* уникальность в папке — без учёта регистра (`File.txt` = `file.txt`),
  обеспечивается индексом `cloud_entries_name_uq` по `lower(name)`;
* запрещены `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`
  (в том числе с расширением: `con.txt`);
* запрещены символы `< > : " / \ | ? *`, управляющие символы и alternate
  data streams (`file.txt:payload`);
* имя не может заканчиваться точкой или пробелом;
* полная длина виртуального пути ограничена 240 UTF-16 единицами — это
  резерв под `MAX_PATH`, корень тома и каталог `.nexus/exports`; лимит
  действует независимо от ограничения в 100 уровней.

Политика ошибок для Windows/NTFS:

* опасное имя (traversal, диск, UNC, ADS, зарезервированное устройство,
  завершающая точка/пробел) → `422 VALIDATION_ERROR`; сервер не переименовывает
  файл молча;
* превышение бюджета пути → `422 PATH_TOO_LONG` с `details.maxLength` и
  `details.actualLength`; имя не обрезается;
* коллизия без учёта регистра в одной папке → `409 FILE_NAME_CONFLICT` с
  `details.existingFileId`; существующий объект не перезаписывается;
* символическая ссылка/junction/reparse point в томе или при регистрации
  тома → `422 VALIDATION_ERROR` (см. `docs/SECURITY.md` §12.1).

Удаление объектов, заменяемых файлов и ZIP-архивов на Windows повторяется
с backoff при `ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION`. Если файл
так и остался открыт, `DELETE /cloud/exports/{id}` возвращает `FILE_LOCKED`,
а запись архива не удаляется; фоновая очистка оставляет задачу до следующего
запуска. Объекты с нулевыми ссылками, которые не удалось удалить, остаются
в `storage_objects` и повторяются при следующей очистке.

### Пользователи, роли, задачи

| Код | HTTP | Когда |
|---|---|---|
| `USERNAME_TAKEN` | 409 | Логин уже существует |
| `ROLE_KEY_TAKEN` | 409 | Ключ роли занят |
| `ROLE_IN_USE` | 409 | Роль назначена пользователям |
| `SYSTEM_ROLE` | 409 | Системную роль нельзя удалить/изменить |
| `LAST_SUPERADMIN` | 409 | Нельзя удалить последнего суперадмина |
| `JOB_FINISHED` | 409 | Задача уже завершена |
| `JOB_NOT_CANCELLABLE` | 409 | Задача не поддерживает отмену |

### Minecraft

| Код | HTTP | Когда |
|---|---|---|
| `BUILD_SLUG_TAKEN` | 409 | Slug сборки занят |
| `BUILD_PATH_CONFLICT` | 409 | Путь файла/папки конфликтует |
| `BUILD_PATH_IS_FOLDER` | 409 | По пути находится папка |
| `BUILD_ARCHIVED` | 409 | Сборка в архиве |
| `BUILD_HAS_PUBLISHED_RELEASES` | 409 | Нельзя удалить сборку с опубликованными релизами |
| `RELEASE_VERSION_TAKEN` | 409 | Версия релиза уже существует |
| `RELEASE_ARCHIVED` | 409 | Релиз в архиве |
| `RELEASE_NOT_PUBLISHED` | 409 | Действие требует опубликованного релиза |
| `VALIDATION_WARNINGS` | 422 | Проверка нашла ошибки (`details`) |
| `NO_REQUIRED_FILES` | 422 | Нет обязательных файлов |
| `MINECRAFT_VERSION_MISSING` | 422 | Не указана версия Minecraft |
| `LOADER_VERSION_MISSING` | 422 | Не указана версия загрузчика |
| `JAVA_INVALID` | 422 | Некорректная версия Java |

---

## 9. Примеры

### 9.1 curl: вход, CSRF и список файлов

```bash
BASE=http://localhost:8080

# Вход: cookie сохраняется в cookies.txt
curl -sS -c cookies.txt \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"SecretPass1!"}' \
  "$BASE/api/v1/auth/login"

# CSRF-токен из ответа (jq)
CSRF=$(curl -sS -b cookies.txt "$BASE/api/v1/auth/me" | jq -r '.data.csrfToken')

# Список корня
curl -sS -b cookies.txt \
  "$BASE/api/v1/cloud/entries?page=1&perPage=25&view=folder"

# Создание папки
curl -sS -b cookies.txt \
  -H "X-CSRF-Token: $CSRF" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Backups","parentId":null}' \
  "$BASE/api/v1/cloud/entries"
```

### 9.2 curl: возобновляемая загрузка

```bash
# 1. Лимиты
curl -sS -b cookies.txt "$BASE/api/v1/cloud/config"

# 2. Создание сессии
SESSION=$(curl -sS -b cookies.txt \
  -H "X-CSRF-Token: $CSRF" \
  -H 'Content-Type: application/json' \
  -d '{"name":"big.bin","size":10485760,"contentType":"application/octet-stream"}' \
  "$BASE/api/v1/cloud/uploads" | jq -r '.data.id')

# 3. Смещение (после обрыва)
curl -sS -b cookies.txt "$BASE/api/v1/cloud/uploads/$SESSION"

# 4. Чанк: файл сначала разрезан на части, затем отправляется с Upload-Offset
OFFSET=0
curl -sS -b cookies.txt -X PATCH \
  -H "X-CSRF-Token: $CSRF" \
  -H "Upload-Offset: $OFFSET" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @chunk-000.bin \
  "$BASE/api/v1/cloud/uploads/$SESSION"

# 5. Завершение (повтор безопасен с тем же Idempotency-Key)
curl -sS -b cookies.txt \
  -H "X-CSRF-Token: $CSRF" \
  -H 'Idempotency-Key: upload-big-1' \
  -H 'Content-Type: application/json' \
  -d '{"strategy":"auto"}' \
  "$BASE/api/v1/cloud/uploads/$SESSION/complete"
```

### 9.3 PowerShell: вход и загрузка

```powershell
$Base = "http://localhost:8080"
$Login = Invoke-RestMethod -Method Post -Uri "$Base/api/v1/auth/login" `
  -ContentType "application/json" `
  -Body (@{ username = "admin"; password = "SecretPass1!" } | ConvertTo-Json) `
  -SessionVariable Session
$Csrf = $Login.data.csrfToken
$Headers = @{ "X-CSRF-Token" = $Csrf }

$Config = Invoke-RestMethod -WebSession $Session -Headers $Headers `
  -Uri "$Base/api/v1/cloud/config"
$ChunkMax = [int64]$Config.data.chunkMaxBytes

$File = "C:\data\big.bin"
$Size = (Get-Item $File).Length
$Created = Invoke-RestMethod -WebSession $Session -Headers $Headers `
  -Method Post -Uri "$Base/api/v1/cloud/uploads" -ContentType "application/json" `
  -Body (@{
    name = [IO.Path]::GetFileName($File)
    size = $Size
    contentType = "application/octet-stream"
  } | ConvertTo-Json)
$UploadId = $Created.data.id

# Продолжение: узнать актуальное смещение
$Status = Invoke-RestMethod -WebSession $Session -Headers $Headers `
  -Uri "$Base/api/v1/cloud/uploads/$UploadId"
$Offset = [int64]$Status.data.offset

$Stream = [IO.File]::OpenRead($File)
try {
  $Stream.Position = $Offset
  while ($Offset -lt $Size) {
    $Length = [Math]::Min($ChunkMax, $Size - $Offset)
    $Buffer = New-Object byte[] $Length
    $Read = $Stream.Read($Buffer, 0, $Length)
    if ($Read -le 0) { break }
    if ($Read -ne $Length) {
      $Trimmed = New-Object byte[] $Read
      [Array]::Copy($Buffer, $Trimmed, $Read)
      $Buffer = $Trimmed
    }

    $ChunkHeaders = @{
      "X-CSRF-Token" = $Csrf
      "Upload-Offset" = "$Offset"
    }
    $Result = Invoke-RestMethod -WebSession $Session -Headers $ChunkHeaders `
      -Method Patch -Uri "$Base/api/v1/cloud/uploads/$UploadId" `
      -ContentType "application/octet-stream" -Body $Buffer
    $Offset = [int64]$Result.data.offset
  }
} finally {
  $Stream.Dispose()
}

$Complete = Invoke-RestMethod -WebSession $Session `
  -Headers (@{ "X-CSRF-Token" = $Csrf; "Idempotency-Key" = "upload-big-1" }) `
  -Method Post -Uri "$Base/api/v1/cloud/uploads/$UploadId/complete" `
  -ContentType "application/json" -Body (@{ strategy = "auto" } | ConvertTo-Json)
$Complete.data
```

### 9.4 PowerShell: токен лаунчера и каталог

```powershell
$Base = "http://localhost:8080"

$Token = Invoke-RestMethod -Method Post -Uri "$Base/api/v1/launcher/auth/token" `
  -ContentType "application/json" `
  -Body (@{
    name = "nexus-launcher"
    username = "player"
    password = "SecretPass1!"
    scopes = @("launcher.catalog", "launcher.artifacts")
    expiresInHours = 720
  } | ConvertTo-Json)

$Headers = @{ Authorization = "Bearer $($Token.data.token)" }
$Catalog = Invoke-RestMethod -Headers $Headers -Uri "$Base/api/v1/launcher/catalog"
$Catalog.data.builds
```

### 9.5 Публичная ссылка с паролем

```bash
# 1. Пароль передаётся только в теле; в URL его нет.
TICKET=$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -d '{"password":"secret"}' \
  "$BASE/api/v1/public/shares/$TOKEN/unlock" | jq -r '.data.ticket')

# 2. Скачивание по короткоживущему тикету; Range не увеличивает счётчик.
curl -sS -o shared-file.bin \
  "$BASE/api/v1/public/shares/$TOKEN/download?ticket=$TICKET"
```

---

## 10. Перечень эндпоинтов

Раздел генерируется из `docs/openapi.json`. Не редактируйте его вручную:
измените спецификацию в коде и выполните

```bash
cargo run --manifest-path backend/Cargo.toml --example dump_openapi
node scripts/gen-api-doc.mjs
```

<!-- BEGIN GENERATED ENDPOINTS -->
> Всего путей: **173**, операций: **208**.
> Таблица генерируется из `docs/openapi.json` командой `node scripts/gen-api-doc.mjs`.

### Auth — ╨Р╨▓╤В╨╛╤А╨╕╨╖╨░╤Ж╨╕╤П ╨╕ ╤Б╨╡╤Б╤Б╨╕╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `POST` | `/api/v1/auth/login` | ╨Т╤Е╨╛╨┤ ╨┐╨╛ ╨╗╨╛╨│╨╕╨╜╤Г ╨╕ ╨┐╨░╤А╨╛╨╗╤О | — |
| `POST` | `/api/v1/auth/logout` | ╨Т╤Л╤Е╨╛╨┤ | — |
| `GET` | `/api/v1/auth/me` | ╨в╨╡╨║╤Г╤Й╨╕╨╣ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤М ╨╕ ╨┐╤А╨░╨▓╨░ | Cookie |
| `POST` | `/api/v1/auth/password` | ╨б╨╝╨╡╨╜╨░ ╨┐╨░╤А╨╛╨╗╤П | Cookie + CSRF |
| `POST` | `/api/v1/auth/sessions/revoke-all` | ╨Ч╨░╨▓╨╡╤А╤И╨╕╤В╤М ╨▓╤Б╨╡ ╤Б╨╡╤Б╤Б╨╕╨╕ | Cookie + CSRF |
| `GET` | `/api/v1/auth/setup-status` | ╨Э╤Г╨╢╨╜╨░ ╨╗╨╕ ╨┐╨╡╤А╨▓╨╕╤З╨╜╨░╤П ╨╜╨░╤Б╤В╤А╨╛╨╣╨║╨░ | — |

### Users — ╨Я╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╨╕ ╨╕ ╨║╨▓╨╛╤В╤Л

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/users` | ╨б╨┐╨╕╤Б╨╛╨║ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╨╡╨╣ | Cookie |
| `POST` | `/api/v1/users` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie + CSRF |
| `GET` | `/api/v1/users/{id}` | ╨Я╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤М | Cookie |
| `PATCH` | `/api/v1/users/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П, ╨║╨▓╨╛╤В╤Г ╨╕ ╨┐╤А╨░╨▓╨░ | Cookie + CSRF |
| `DELETE` | `/api/v1/users/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie + CSRF |
| `POST` | `/api/v1/users/{id}/password` | ╨б╨▒╤А╨╛╤Б╨╕╤В╤М ╨┐╨░╤А╨╛╨╗╤М | Cookie + CSRF |
| `POST` | `/api/v1/users/{id}/sessions/revoke` | ╨Ч╨░╨▓╨╡╤А╤И╨╕╤В╤М ╤Б╨╡╤Б╤Б╨╕╨╕ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie + CSRF |
| `POST` | `/api/v1/users/{id}/unlock` | ╨а╨░╨╖╨▒╨╗╨╛╨║╨╕╤А╨╛╨▓╨░╤В╤М ╨░╨║╨║╨░╤Г╨╜╤В | Cookie + CSRF |

### Roles — ╨а╨╛╨╗╨╕ ╨╕ ╨│╤А╨░╨╜╤Г╨╗╤П╤А╨╜╤Л╨╡ ╨┐╤А╨░╨▓╨░

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/permissions` | ╨Ъ╨░╤В╨░╨╗╨╛╨│ ╨┐╤А╨░╨▓ | Cookie |
| `GET` | `/api/v1/roles` | ╨б╨┐╨╕╤Б╨╛╨║ ╤А╨╛╨╗╨╡╨╣ | Cookie |
| `POST` | `/api/v1/roles` | ╨б╨╛╨╖╨┤╨░╤В╤М ╤А╨╛╨╗╤М | Cookie + CSRF |
| `PATCH` | `/api/v1/roles/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╤А╨╛╨╗╤М | Cookie + CSRF |
| `DELETE` | `/api/v1/roles/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╤А╨╛╨╗╤М | Cookie + CSRF |

### Settings — ╨Я╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤М╤Б╨║╨╕╨╡ ╨╕ ╤Б╨╕╤Б╤В╨╡╨╝╨╜╤Л╨╡ ╨╜╨░╤Б╤В╤А╨╛╨╣╨║╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/me/settings` | ╨Э╨░╤Б╤В╤А╨╛╨╣╨║╨╕ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie |
| `PUT` | `/api/v1/me/settings/{key}` | ╨б╨╛╤Е╤А╨░╨╜╨╕╤В╤М ╨╜╨░╤Б╤В╤А╨╛╨╣╨║╤Г ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie + CSRF |
| `GET` | `/api/v1/me/usage` | ╨Ш╤Б╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╨╜╨╕╨╡ ╨║╨▓╨╛╤В╤Л | Cookie |
| `GET` | `/api/v1/public/appearance/background` | 3D-╤Д╨╛╨╜ ╨┤╨╗╤П ╤Б╤В╤А╨░╨╜╨╕╤Ж ╨▒╨╡╨╖ ╤Б╨╡╤Б╤Б╨╕╨╕ | — |
| `GET` | `/api/v1/system/settings` | ╨б╨╕╤Б╤В╨╡╨╝╨╜╤Л╨╡ ╨╜╨░╤Б╤В╤А╨╛╨╣╨║╨╕ | Cookie |
| `PUT` | `/api/v1/system/settings/{key}` | ╨б╨╛╤Е╤А╨░╨╜╨╕╤В╤М ╤Б╨╕╤Б╤В╨╡╨╝╨╜╤Г╤О ╨╜╨░╤Б╤В╤А╨╛╨╣╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/system/settings/catalog` | ╨а╨╡╨╡╤Б╤В╤А ╤Б╨╕╤Б╤В╨╡╨╝╨╜╤Л╤Е ╨╜╨░╤Б╤В╤А╨╛╨╡╨║ | Cookie |

### System — ╨б╨╛╤Б╤В╨╛╤П╨╜╨╕╨╡ ╤Б╨╕╤Б╤В╨╡╨╝╤Л ╨╕ ╨╝╨╛╨┤╤Г╨╗╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/docs` | Swagger UI (NEXUS_ENABLE_DOCS) | — |
| `GET` | `/api/v1/health` | ╨б╨╛╤Б╤В╨╛╤П╨╜╨╕╨╡ ╤Б╨╡╤А╨▓╨╡╤А╨░ | — |
| `GET` | `/api/v1/openapi.json` | ╨б╨┐╨╡╤Ж╨╕╤Д╨╕╨║╨░╤Ж╨╕╤П OpenAPI 3.1 | — |
| `GET` | `/api/v1/system/audit` | ╨Ц╤Г╤А╨╜╨░╨╗ ╤Б╨╛╨▒╤Л╤В╨╕╨╣ | Cookie |
| `GET` | `/api/v1/system/modules` | ╨б╨╛╤Б╤В╨╛╤П╨╜╨╕╨╡ ╨╝╨╛╨┤╤Г╨╗╨╡╨╣ | Cookie |
| `PUT` | `/api/v1/system/modules/{key}` | ╨Т╨║╨╗╤О╤З╨╕╤В╤М ╨╕╨╗╨╕ ╨╛╤В╨║╨╗╤О╤З╨╕╤В╤М ╨╝╨╛╨┤╤Г╨╗╤М | Cookie + CSRF |
| `GET` | `/api/v1/system/overview` | ╨Ю╨▒╨╖╨╛╤А ╤Б╨╕╤Б╤В╨╡╨╝╤Л | Cookie |

### Storage — ╨Ф╨╕╤Б╨║╨╕ ╨╕ ╤В╨╛╨╝╨░ ╤Е╤А╨░╨╜╨╡╨╜╨╕╤П

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/storage/browse` | ╨Ю╨▒╨╖╨╛╤А ╨║╨░╤В╨░╨╗╨╛╨│╨╛╨▓ ╤Б╨╡╤А╨▓╨╡╤А╨░ | Cookie |
| `GET` | `/api/v1/storage/disks` | ╨д╨╕╨╖╨╕╤З╨╡╤Б╨║╨╕╨╡ ╨┤╨╕╤Б╨║╨╕ | Cookie |
| `GET` | `/api/v1/storage/owners` | ╨Я╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╨╕ ╨┤╨╗╤П ╨┐╤А╨╕╨▓╤П╨╖╨║╨╕ ╤В╨╛╨╝╨░ | Cookie |
| `GET` | `/api/v1/storage/summary` | ╨б╨▓╨╛╨┤╨║╨░ ╤Е╤А╨░╨╜╨╕╨╗╨╕╤Й | Cookie |
| `GET` | `/api/v1/storage/volumes` | ╨в╨╛╨╝╨░ ╤Е╤А╨░╨╜╨╡╨╜╨╕╤П | Cookie |
| `POST` | `/api/v1/storage/volumes` | ╨Ч╨░╤А╨╡╨│╨╕╤Б╤В╤А╨╕╤А╨╛╨▓╨░╤В╤М ╤В╨╛╨╝ | Cookie + CSRF |
| `PATCH` | `/api/v1/storage/volumes/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╤В╨╛╨╝ | Cookie + CSRF |
| `DELETE` | `/api/v1/storage/volumes/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╤В╨╛╨╝ | Cookie + CSRF |
| `POST` | `/api/v1/storage/volumes/{id}/check` | ╨Я╤А╨╛╨▓╨╡╤А╨╕╤В╤М ╤В╨╛╨╝ | Cookie + CSRF |
| `POST` | `/api/v1/storage/volumes/scan` | ╨Я╤А╨╛╨▓╨╡╤А╨╕╤В╤М ╨║╨░╤В╨░╨╗╨╛╨│ ╨┐╨╡╤А╨╡╨┤ ╤А╨╡╨│╨╕╤Б╤В╤А╨░╤Ж╨╕╨╡╨╣ ╤В╨╛╨╝╨░ | Cookie + CSRF |

### Cloud — ╨Ю╨▒╨╗╨░╤З╨╜╨╛╨╡ ╤Е╤А╨░╨╜╨╕╨╗╨╕╤Й╨╡

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/cloud/config` | ╨Ы╨╕╨╝╨╕╤В╤Л ╨╖╨░╨│╤А╤Г╨╖╨║╨╕ | Cookie |
| `GET` | `/api/v1/cloud/entries` | ╨б╨┐╨╕╤Б╨╛╨║ ╤Д╨░╨╣╨╗╨╛╨▓ ╨╕ ╨┐╨░╨┐╨╛╨║ | Cookie |
| `POST` | `/api/v1/cloud/entries` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┐╨░╨┐╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/cloud/entries/{id}` | ╨б╨▓╨╡╨┤╨╡╨╜╨╕╤П ╨╛ ╨╖╨░╨┐╨╕╤Б╨╕ | Cookie |
| `PATCH` | `/api/v1/cloud/entries/{id}` | ╨Я╨╡╤А╨╡╨╕╨╝╨╡╨╜╨╛╨▓╨░╤В╤М ╨╕╨╗╨╕ ╨┐╨╡╤А╨╡╨╝╨╡╤Б╤В╨╕╤В╤М ╨╖╨░╨┐╨╕╤Б╤М (Idempotency-Key ╨┐╤А╨╕ strategy=replace) | Cookie + CSRF |
| `DELETE` | `/api/v1/cloud/entries/{id}` | ╨Я╨╡╤А╨╡╨╝╨╡╤Б╤В╨╕╤В╤М ╨▓ ╨║╨╛╤А╨╖╨╕╨╜╤Г | Cookie + CSRF |
| `GET` | `/api/v1/cloud/entries/{id}/download` | ╨б╨║╨░╤З╨░╤В╤М ╤Д╨░╨╣╨╗ (Range) | Cookie |
| `GET` | `/api/v1/cloud/entries/{id}/preview` | ╨Я╤А╨╡╨┤╨┐╤А╨╛╤Б╨╝╨╛╤В╤А ╤Д╨░╨╣╨╗╨░ | Cookie |
| `DELETE` | `/api/v1/cloud/entries/{id}/purge` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨▒╨╡╨╖╨▓╨╛╨╖╨▓╤А╨░╤В╨╜╨╛ | Cookie + CSRF |
| `POST` | `/api/v1/cloud/entries/{id}/restore` | ╨Т╨╛╤Б╤Б╤В╨░╨╜╨╛╨▓╨╕╤В╤М ╨╕╨╖ ╨║╨╛╤А╨╖╨╕╨╜╤Л | Cookie + CSRF |
| `POST` | `/api/v1/cloud/entries/batch` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┐╨░╨┐╨║╨╕ ╨┐╨░╨║╨╡╤В╨╜╨╛ (╨┤╨╛ 200 ╨╕╨╝╤С╨╜) | Cookie + CSRF |
| `GET` | `/api/v1/cloud/exports` | ╨б╨┐╨╕╤Б╨╛╨║ ╨░╤А╤Е╨╕╨▓╨╛╨▓ | Cookie |
| `POST` | `/api/v1/cloud/exports` | ╨б╨╛╨╖╨┤╨░╤В╤М ZIP-╨░╤А╤Е╨╕╨▓ (Idempotency-Key) | Cookie + CSRF |
| `GET` | `/api/v1/cloud/exports/{id}` | ╨б╨╛╤Б╤В╨╛╤П╨╜╨╕╨╡ ╨░╤А╤Е╨╕╨▓╨░ | Cookie |
| `DELETE` | `/api/v1/cloud/exports/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨░╤А╤Е╨╕╨▓ ╨╕ ╨╖╨░╨┐╨╕╤Б╤М ╨╖╨░╨┤╨░╤З╨╕ | Cookie + CSRF |
| `GET` | `/api/v1/cloud/exports/{id}/file` | ╨б╨║╨░╤З╨░╤В╤М ╨░╤А╤Е╨╕╨▓ | Cookie |
| `GET` | `/api/v1/cloud/shares` | ╨б╨┐╨╕╤Б╨╛╨║ ╤Б╤Б╤Л╨╗╨╛╨║ ╨╛╨▒╤Й╨╡╨│╨╛ ╨┤╨╛╤Б╤В╤Г╨┐╨░ | Cookie |
| `POST` | `/api/v1/cloud/shares` | ╨б╨╛╨╖╨┤╨░╤В╤М ╤Б╤Б╤Л╨╗╨║╤Г (Idempotency-Key) | Cookie + CSRF |
| `DELETE` | `/api/v1/cloud/shares/{id}` | ╨Ю╤В╨╛╨╖╨▓╨░╤В╤М ╤Б╤Б╤Л╨╗╨║╤Г | Cookie + CSRF |
| `DELETE` | `/api/v1/cloud/trash` | ╨Ю╤З╨╕╤Б╤В╨╕╤В╤М ╨║╨╛╤А╨╖╨╕╨╜╤Г | Cookie + CSRF |
| `POST` | `/api/v1/cloud/uploads` | ╨Э╨░╤З╨░╤В╤М ╨▓╨╛╨╖╨╛╨▒╨╜╨╛╨▓╨╗╤П╨╡╨╝╤Г╤О ╨╖╨░╨│╤А╤Г╨╖╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/cloud/uploads/{id}` | ╨б╨╝╨╡╤Й╨╡╨╜╨╕╨╡ ╨╖╨░╨│╤А╤Г╨╖╨║╨╕ | Cookie |
| `PATCH` | `/api/v1/cloud/uploads/{id}` | ╨Ч╨░╨┐╨╕╤Б╨░╤В╤М ╤З╨░╨╜╨║ | Cookie + CSRF |
| `DELETE` | `/api/v1/cloud/uploads/{id}` | ╨Ю╤В╨╝╨╡╨╜╨╕╤В╤М ╨╖╨░╨│╤А╤Г╨╖╨║╤Г | Cookie + CSRF |
| `POST` | `/api/v1/cloud/uploads/{id}/complete` | ╨Ч╨░╨▓╨╡╤А╤И╨╕╤В╤М ╨╖╨░╨│╤А╤Г╨╖╨║╤Г (Idempotency-Key) | Cookie + CSRF |

### Public — ╨Я╤Г╨▒╨╗╨╕╤З╨╜╤Л╨╡ ╤Б╤Б╤Л╨╗╨║╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/public/shares/{token}` | ╨б╨▓╨╡╨┤╨╡╨╜╨╕╤П ╨╛ ╨┐╤Г╨▒╨╗╨╕╤З╨╜╨╛╨╣ ╤Б╤Б╤Л╨╗╨║╨╡ | — |
| `GET` | `/api/v1/public/shares/{token}/download` | ╨б╨║╨░╤З╨░╤В╤М ╨┐╨╛ ╨┐╤Г╨▒╨╗╨╕╤З╨╜╨╛╨╣ ╤Б╤Б╤Л╨╗╨║╨╡ | — |
| `POST` | `/api/v1/public/shares/{token}/unlock` | ╨Я╤А╨╛╨▓╨╡╤А╨╕╤В╤М ╨┐╨░╤А╨╛╨╗╤М ╨╕ ╨┐╨╛╨╗╤Г╤З╨╕╤В╤М ╤В╨╕╨║╨╡╤В ╤Б╨║╨░╤З╨╕╨▓╨░╨╜╨╕╤П | — |

### Minecraft — ╨б╨▒╨╛╤А╨║╨╕, ╤Д╨░╨╣╨╗╤Л ╨╕ ╤А╨╡╨╗╨╕╨╖╤Л

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/minecraft/builds` | ╨б╨┐╨╕╤Б╨╛╨║ ╤Б╨▒╨╛╤А╨╛╨║ | Cookie |
| `POST` | `/api/v1/minecraft/builds` | ╨б╨╛╨╖╨┤╨░╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}` | ╨б╨▒╨╛╤А╨║╨░ ╤Б ╤Д╨░╨╣╨╗╨░╨╝╨╕ ╨╕ ╤А╨╡╨╗╨╕╨╖╨░╨╝╨╕ | Cookie |
| `PATCH` | `/api/v1/minecraft/builds/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `DELETE` | `/api/v1/minecraft/builds/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/archive` | ╨Р╤А╤Е╨╕╨▓╨╕╤А╨╛╨▓╨░╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/clone` | ╨Ъ╨╗╨╛╨╜╨╕╤А╨╛╨▓╨░╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/files` | ╨д╨░╨╣╨╗╤Л ╤Б╨▒╨╛╤А╨║╨╕ | Cookie |
| `DELETE` | `/api/v1/minecraft/builds/{id}/files` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨▓╤Б╨╡ ╤Д╨░╨╣╨╗╤Л | Cookie + CSRF |
| `PATCH` | `/api/v1/minecraft/builds/{id}/files/{file_id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╤Д╨░╨╣╨╗ | Cookie + CSRF |
| `DELETE` | `/api/v1/minecraft/builds/{id}/files/{file_id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╤Д╨░╨╣╨╗ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/files/{file_id}/replace` | ╨Ч╨░╨╝╨╡╨╜╨╕╤В╤М ╤Б╨╛╨┤╨╡╤А╨╢╨╕╨╝╨╛╨╡ ╤Д╨░╨╣╨╗╨░ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/files/folder` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┐╨░╨┐╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/history` | ╨Ш╤Б╤В╨╛╤А╨╕╤П ╨╕╨╖╨╝╨╡╨╜╨╡╨╜╨╕╨╣ | Cookie |
| `GET` | `/api/v1/minecraft/builds/{id}/icon` | ╨Ш╨║╨╛╨╜╨║╨░ ╤Б╨▒╨╛╤А╨║╨╕ | Cookie |
| `PUT` | `/api/v1/minecraft/builds/{id}/icon` | ╨Ч╨░╨│╤А╤Г╨╖╨╕╤В╤М ╨╕╨║╨╛╨╜╨║╤Г | Cookie + CSRF |
| `DELETE` | `/api/v1/minecraft/builds/{id}/icon` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨╕╨║╨╛╨╜╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/releases` | ╨а╨╡╨╗╨╕╨╖╤Л ╤Б╨▒╨╛╤А╨║╨╕ | Cookie |
| `POST` | `/api/v1/minecraft/builds/{id}/releases` | ╨б╨╛╨╖╨┤╨░╤В╤М ╤А╨╡╨╗╨╕╨╖ | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/releases/{release_id}` | ╨а╨╡╨╗╨╕╨╖ | Cookie |
| `POST` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/activate` | ╨б╨┤╨╡╨╗╨░╤В╤М ╤А╨╡╨╗╨╕╨╖ ╤В╨╡╨║╤Г╤Й╨╕╨╝ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/archive` | ╨Р╤А╤Е╨╕╨▓╨╕╤А╨╛╨▓╨░╤В╤М ╤А╨╡╨╗╨╕╨╖ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/deprecate` | ╨Я╨╛╨╝╨╡╤В╨╕╤В╤М ╤А╨╡╨╗╨╕╨╖ ╤Г╤Б╤В╨░╤А╨╡╨▓╤И╨╕╨╝ | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/launcher-manifest` | ╨Ь╨░╨╜╨╕╤Д╨╡╤Б╤В MyLauncher ╨┤╨╗╤П ╤А╨╡╨╗╨╕╨╖╨░ | Cookie |
| `GET` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/manifest` | ╨Ь╨░╨╜╨╕╤Д╨╡╤Б╤В ╤А╨╡╨╗╨╕╨╖╨░ | Cookie |
| `POST` | `/api/v1/minecraft/builds/{id}/releases/{release_id}/publish` | ╨Ю╨┐╤Г╨▒╨╗╨╕╨║╨╛╨▓╨░╤В╤М ╤А╨╡╨╗╨╕╨╖ | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/builds/{id}/releases/compare` | ╨б╤А╨░╨▓╨╜╨╕╤В╤М ╤А╨╡╨╗╨╕╨╖╤Л | Cookie |
| `POST` | `/api/v1/minecraft/builds/{id}/restore` | ╨Т╨╛╤Б╤Б╤В╨░╨╜╨╛╨▓╨╕╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/unpublish` | ╨б╨╜╤П╤В╤М ╤Б╨▒╨╛╤А╨║╤Г ╤Б ╨┐╤Г╨▒╨╗╨╕╨║╨░╤Ж╨╕╨╕ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/builds/{id}/validate` | ╨Я╤А╨╛╨▓╨╡╤А╨╕╤В╤М ╤Б╨▒╨╛╤А╨║╤Г | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/metadata/fabric` | ╨Т╨╡╤А╤Б╨╕╨╕ Fabric | Cookie |
| `GET` | `/api/v1/minecraft/metadata/forge` | ╨Т╨╡╤А╤Б╨╕╨╕ Forge | Cookie |
| `GET` | `/api/v1/minecraft/metadata/loaders` | ╨б╨╛╨▓╨╝╨╡╤Б╤В╨╕╨╝╤Л╨╡ ╨▓╨╡╤А╤Б╨╕╨╕ ╨╖╨░╨│╤А╤Г╨╖╤З╨╕╨║╨░ | Cookie |
| `GET` | `/api/v1/minecraft/metadata/minecraft` | ╨Т╨╡╤А╤Б╨╕╨╕ Minecraft | Cookie |
| `GET` | `/api/v1/minecraft/metadata/neoforge` | ╨Т╨╡╤А╤Б╨╕╨╕ NeoForge | Cookie |
| `GET` | `/api/v1/minecraft/metadata/quilt` | ╨Т╨╡╤А╤Б╨╕╨╕ Quilt | Cookie |
| `GET` | `/api/v1/minecraft/stats` | ╨б╨▓╨╛╨┤╨║╨░ ╨┐╨╛ ╤Б╨▒╨╛╤А╨║╨░╨╝ | Cookie |
| `POST` | `/api/v1/minecraft/uploads` | ╨Э╨░╤З╨░╤В╤М ╨╖╨░╨│╤А╤Г╨╖╨║╤Г ╨░╤А╤В╨╡╤Д╨░╨║╤В╨░ | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/uploads/{id}` | ╨б╨╝╨╡╤Й╨╡╨╜╨╕╨╡ ╨╖╨░╨│╤А╤Г╨╖╨║╨╕ ╨░╤А╤В╨╡╤Д╨░╨║╤В╨░ | Cookie |
| `PATCH` | `/api/v1/minecraft/uploads/{id}` | ╨Ч╨░╨┐╨╕╤Б╨░╤В╤М ╤З╨░╨╜╨║ ╨░╤А╤В╨╡╤Д╨░╨║╤В╨░ | Cookie + CSRF |
| `DELETE` | `/api/v1/minecraft/uploads/{id}` | ╨Ю╤В╨╝╨╡╨╜╨╕╤В╤М ╨╖╨░╨│╤А╤Г╨╖╨║╤Г ╨░╤А╤В╨╡╤Д╨░╨║╤В╨░ | Cookie + CSRF |
| `POST` | `/api/v1/minecraft/uploads/{id}/complete` | ╨Ч╨░╨▓╨╡╤А╤И╨╕╤В╤М ╨╖╨░╨│╤А╤Г╨╖╨║╤Г ╨░╤А╤В╨╡╤Д╨░╨║╤В╨░ (Idempotency-Key) | Cookie + CSRF |
| `GET` | `/api/v1/minecraft/uploads/config` | ╨Ы╨╕╨╝╨╕╤В╤Л ╨╖╨░╨│╤А╤Г╨╖╨║╨╕ ╨░╤А╤В╨╡╤Д╨░╨║╤В╨╛╨▓ | Cookie |

### Launcher — API ╨┤╨╗╤П ╨║╨╗╨╕╨╡╨╜╤В╨░ ╨╗╨░╤Г╨╜╤З╨╡╤А╨░

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/files/{hash}` | ╨д╨░╨╣╨╗ ╤Б╨▒╨╛╤А╨║╨╕ MyLauncher | — |
| `GET` | `/api/packs` | ╨Ъ╨░╤В╨░╨╗╨╛╨│ ╤Б╨▒╨╛╤А╨╛╨║ MyLauncher | — |
| `GET` | `/api/packs/{pack_id}/icon` | ╨Ш╨║╨╛╨╜╨║╨░ ╤Б╨▒╨╛╤А╨║╨╕ MyLauncher | — |
| `GET` | `/api/packs/{pack_id}/versions` | ╨Т╨╡╤А╤Б╨╕╨╕ ╤Б╨▒╨╛╤А╨║╨╕ MyLauncher | — |
| `GET` | `/api/packs/{pack_id}/versions/{version}/manifest` | ╨Ь╨░╨╜╨╕╤Д╨╡╤Б╤В MyLauncher | — |
| `GET` | `/api/v1/admin/launcher/repository` | ╨Я╨░╤А╨░╨╝╨╡╤В╤А╤Л ╤А╨╡╨┐╨╛╨╖╨╕╤В╨╛╤А╨╕╤П MyLauncher | Cookie |
| `GET` | `/api/v1/admin/launcher/tokens` | ╨в╨╛╨║╨╡╨╜╤Л ╨╗╨░╤Г╨╜╤З╨╡╤А╨░ | Cookie |
| `DELETE` | `/api/v1/admin/launcher/tokens/{id}` | ╨Ю╤В╨╛╨╖╨▓╨░╤В╤М ╤В╨╛╨║╨╡╨╜ | Cookie + CSRF |
| `GET` | `/api/v1/launcher/artifacts/{id}` | ╨б╨║╨░╤З╨░╤В╤М ╨░╤А╤В╨╡╤Д╨░╨║╤В | Bearer · launcher.artifacts |
| `POST` | `/api/v1/launcher/auth/revoke` | ╨Ю╤В╨╛╨╖╨▓╨░╤В╤М ╤Б╨▓╨╛╨╣ ╤В╨╛╨║╨╡╨╜ | Bearer |
| `POST` | `/api/v1/launcher/auth/token` | ╨Т╤Л╨┤╨░╤В╤М ╤В╨╛╨║╨╡╨╜ ╨╗╨░╤Г╨╜╤З╨╡╤А╨░ | — |
| `GET` | `/api/v1/launcher/builds` | ╨б╨┐╨╕╤Б╨╛╨║ ╤Б╨▒╨╛╤А╨╛╨║ | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/builds/{slug}` | ╨б╨▒╨╛╤А╨║╨░ ╨┐╨╛ slug | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/builds/{slug}/icon` | ╨Ш╨║╨╛╨╜╨║╨░ ╤Б╨▒╨╛╤А╨║╨╕ | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/builds/{slug}/latest/manifest` | ╨Ь╨░╨╜╨╕╤Д╨╡╤Б╤В ╨┐╨╛╤Б╨╗╨╡╨┤╨╜╨╡╨│╨╛ ╤А╨╡╨╗╨╕╨╖╨░ | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/builds/{slug}/releases` | ╨а╨╡╨╗╨╕╨╖╤Л ╤Б╨▒╨╛╤А╨║╨╕ | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/builds/{slug}/releases/{version}/manifest` | ╨Ь╨░╨╜╨╕╤Д╨╡╤Б╤В ╨▓╨╡╤А╤Б╨╕╨╕ | Bearer · launcher.catalog |
| `GET` | `/api/v1/launcher/catalog` | ╨Ъ╨░╤В╨░╨╗╨╛╨│ ╤Б╨▒╨╛╤А╨╛╨║ | Bearer · launcher.catalog |

### Monitoring — ╨Ь╨╡╤В╤А╨╕╨║╨╕ ╨╕ ╨╕╤Б╤В╨╛╤А╨╕╤П

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/monitoring/current` | ╨в╨╡╨║╤Г╤Й╨╕╨╡ ╨╝╨╡╤В╤А╨╕╨║╨╕ | Cookie |
| `GET` | `/api/v1/monitoring/history` | ╨Ш╤Б╤В╨╛╤А╨╕╤П ╨╝╨╡╤В╤А╨╕╨║ | Cookie |
| `GET` | `/api/v1/monitoring/intervals` | ╨Ш╨╜╤В╨╡╤А╨▓╨░╨╗╤Л ╨╕╤Б╤В╨╛╤А╨╕╨╕ | Cookie |
| `GET` | `/api/v1/monitoring/modules` | ╨Ь╨╡╤В╤А╨╕╨║╨╕ ╨╝╨╛╨┤╤Г╨╗╨╡╨╣ | Cookie |

### Jobs — ╨д╨╛╨╜╨╛╨▓╤Л╨╡ ╨╖╨░╨┤╨░╤З╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/jobs` | ╨д╨╛╨╜╨╛╨▓╤Л╨╡ ╨╖╨░╨┤╨░╤З╨╕ | Cookie |
| `GET` | `/api/v1/jobs/{id}` | ╨Ч╨░╨┤╨░╤З╨░ | Cookie |
| `POST` | `/api/v1/jobs/{id}/cancel` | ╨Ю╤В╨╝╨╡╨╜╨╕╤В╤М ╨╖╨░╨┤╨░╤З╤Г | Cookie + CSRF |
| `POST` | `/api/v1/jobs/{id}/pause` | ╨Я╤А╨╕╨╛╤Б╤В╨░╨╜╨╛╨▓╨╕╤В╤М ╨╖╨░╨┤╨░╤З╤Г | Cookie + CSRF |
| `POST` | `/api/v1/jobs/{id}/resume` | ╨Т╨╛╨╖╨╛╨▒╨╜╨╛╨▓╨╕╤В╤М ╨╖╨░╨┤╨░╤З╤Г | Cookie + CSRF |
| `POST` | `/api/v1/jobs/maintenance` | ╨Ч╨░╨┐╤Г╤Б╤В╨╕╤В╤М ╨╛╨▒╤Б╨╗╤Г╨╢╨╕╨▓╨░╨╜╨╕╨╡ | Cookie + CSRF |

### Realtime — WebSocket-╤Б╨╛╨▒╤Л╤В╨╕╤П

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/ws` | WebSocket-╨║╨░╨╜╨░╨╗ ╨┐╤А╨╛╨│╤А╨╡╤Б╤Б╨░ | Cookie |

### AI — AI Chat, ╨┤╨╕╨░╨╗╨╛╨│╨╕ ╨╕ Agent ╨╖╨░╨┤╨░╤З╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `POST` | `/api/v1/ai/approvals/{id}/approve` | ╨Я╨╛╨┤╤В╨▓╨╡╤А╨┤╨╕╤В╤М ╨┤╨╡╨╣╤Б╤В╨▓╨╕╨╡ | Cookie + CSRF |
| `POST` | `/api/v1/ai/approvals/{id}/reject` | ╨Ю╤В╨║╨╗╨╛╨╜╨╕╤В╤М ╨┤╨╡╨╣╤Б╤В╨▓╨╕╨╡ | Cookie + CSRF |
| `GET` | `/api/v1/ai/approvals/pending` | ╨Ю╨╢╨╕╨┤╨░╤О╤Й╨╕╨╡ approvals | Cookie |
| `GET` | `/api/v1/ai/conversations` | ╨б╨┐╨╕╤Б╨╛╨║ ╨┤╨╕╨░╨╗╨╛╨│╨╛╨▓ | Cookie |
| `POST` | `/api/v1/ai/conversations` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┤╨╕╨░╨╗╨╛╨│ | Cookie + CSRF |
| `GET` | `/api/v1/ai/conversations/{id}` | ╨Ь╨╡╤В╨░╨┤╨░╨╜╨╜╤Л╨╡ ╨┤╨╕╨░╨╗╨╛╨│╨░ | Cookie |
| `PATCH` | `/api/v1/ai/conversations/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╨┤╨╕╨░╨╗╨╛╨│ | Cookie + CSRF |
| `DELETE` | `/api/v1/ai/conversations/{id}` | ╨Р╤А╤Е╨╕╨▓╨╕╤А╨╛╨▓╨░╤В╤М ╨┤╨╕╨░╨╗╨╛╨│ | Cookie + CSRF |
| `POST` | `/api/v1/ai/conversations/{id}/attachments` | ╨Ч╨░╨│╤А╤Г╨╖╨╕╤В╤М ╤В╨╡╨║╤Б╤В╨╛╨▓╨╛╨╡ ╨▓╨╗╨╛╨╢╨╡╨╜╨╕╨╡ | Cookie + CSRF |
| `GET` | `/api/v1/ai/conversations/{id}/messages` | ╨Ш╤Б╤В╨╛╤А╨╕╤П ╤Б╨╛╨╛╨▒╤Й╨╡╨╜╨╕╨╣ | Cookie |
| `POST` | `/api/v1/ai/conversations/{id}/messages` | ╨Ю╤В╨┐╤А╨░╨▓╨╕╤В╤М ╤Б╨╛╨╛╨▒╤Й╨╡╨╜╨╕╨╡ | Cookie + CSRF |
| `PATCH` | `/api/v1/ai/conversations/{id}/messages/{message_id}` | ╨а╨╡╨┤╨░╨║╤В╨╕╤А╨╛╨▓╨░╤В╤М ╤Б╨╛╨╛╨▒╤Й╨╡╨╜╨╕╨╡ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie + CSRF |
| `POST` | `/api/v1/ai/conversations/{id}/messages/{message_id}/regenerate` | ╨Я╨╛╨▓╤В╨╛╤А╨╜╨░╤П ╨│╨╡╨╜╨╡╤А╨░╤Ж╨╕╤П ╨╛╤В╨▓╨╡╤В╨░ | Cookie + CSRF |
| `POST` | `/api/v1/ai/conversations/{id}/stop` | ╨Ю╤Б╤В╨░╨╜╨╛╨▓╨╕╤В╤М ╨│╨╡╨╜╨╡╤А╨░╤Ж╨╕╤О | Cookie + CSRF |
| `GET` | `/api/v1/ai/providers` | ╨Ф╨╛╤Б╤В╤Г╨┐╨╜╤Л╨╡ ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╤Л | Cookie |
| `GET` | `/api/v1/ai/providers/{id}` | ╨Я╤А╨╛╨▓╨░╨╣╨┤╨╡╤А ╨▒╨╡╨╖ ╤Б╨╡╨║╤А╨╡╤В╨╛╨▓ | Cookie |
| `GET` | `/api/v1/ai/skills` | ╨Э╨░╨╖╨╜╨░╤З╨╡╨╜╨╜╤Л╨╡ Skills | Cookie |
| `GET` | `/api/v1/ai/skills/{id}` | ╨б╨╛╨┤╨╡╤А╨╢╨╕╨╝╨╛╨╡ ╨╜╨░╨╖╨╜╨░╤З╨╡╨╜╨╜╨╛╨│╨╛ Skill | Cookie |
| `GET` | `/api/v1/ai/tasks` | ╨Ч╨░╨┤╨░╤З╨╕ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie |
| `POST` | `/api/v1/ai/tasks` | ╨б╨╛╨╖╨┤╨░╤В╤М Agent Task | Cookie + CSRF |
| `GET` | `/api/v1/ai/tasks/{id}` | ╨б╨╛╤Б╤В╨╛╤П╨╜╨╕╨╡ ╨╖╨░╨┤╨░╤З╨╕ | Cookie |
| `POST` | `/api/v1/ai/tasks/{id}/cancel` | ╨Ю╤В╨╝╨╡╨╜╨╕╤В╤М ╨╖╨░╨┤╨░╤З╤Г | Cookie + CSRF |
| `GET` | `/api/v1/ai/tasks/{id}/events` | ╨б╨╛╨▒╤Л╤В╨╕╤П ╨╖╨░╨┤╨░╤З╨╕ | Cookie |
| `GET` | `/api/v1/ai/tasks/{id}/executions` | ╨Ш╤Б╤В╨╛╤А╨╕╤П ╨╕╨╜╤Б╤В╤А╤Г╨╝╨╡╨╜╤В╨╛╨▓ | Cookie |
| `GET` | `/api/v1/ai/tools` | ╨Ъ╨░╤В╨░╨╗╨╛╨│ ╤А╨░╨╖╤А╨╡╤И╤С╨╜╨╜╤Л╤Е ╨╕╨╜╤Б╤В╤А╤Г╨╝╨╡╨╜╤В╨╛╨▓ | Cookie |
| `GET` | `/api/v1/ai/workspaces` | ╨а╨░╨▒╨╛╤З╨╕╨╡ ╨┐╤А╨╛╤Б╤В╤А╨░╨╜╤Б╤В╨▓╨░ ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╤П | Cookie |
| `GET` | `/api/v1/ai/workspaces/{id}` | ╨Ь╨╡╤В╨░╨┤╨░╨╜╨╜╤Л╨╡ workspace | Cookie |
| `GET` | `/api/v1/ai/workspaces/{id}/usage` | ╨Ш╤Б╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╨╜╨╕╨╡ ╨╕ ╨║╨▓╨╛╤В╤Л | Cookie |

### AI Admin — Runtime, ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╤Л, workspace, Skills ╨╕ ╨┐╨╛╨╗╨╕╤В╨╕╨║╨╕

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/ai/admin/enrollment-codes` | ╨Ъ╨╛╨┤╤Л ╤А╨╡╨│╨╕╤Б╤В╤А╨░╤Ж╨╕╨╕ Runtime | Cookie |
| `POST` | `/api/v1/ai/admin/enrollment-codes` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨╛╨┤╨╜╨╛╤А╨░╨╖╨╛╨▓╤Л╨╣ ╨║╨╛╨┤ Runtime | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/enrollment-codes/{id}/revoke` | ╨Ю╤В╨╛╨╖╨▓╨░╤В╤М ╨║╨╛╨┤ ╤А╨╡╨│╨╕╤Б╤В╤А╨░╤Ж╨╕╨╕ | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/logs` | AI ╨░╤Г╨┤╨╕╤В ╨╕ Runtime-╨╢╤Г╤А╨╜╨░╨╗ | Cookie |
| `GET` | `/api/v1/ai/admin/overview` | AI Overview | Cookie |
| `GET` | `/api/v1/ai/admin/programs` | ╨Ч╨░╤А╨╡╨│╨╕╤Б╤В╤А╨╕╤А╨╛╨▓╨░╨╜╨╜╤Л╨╡ ╨┐╤А╨╛╨│╤А╨░╨╝╨╝╤Л | Cookie |
| `PATCH` | `/api/v1/ai/admin/programs/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╨┐╤А╨╛╨│╤А╨░╨╝╨╝╤Г | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/providers` | ╨б╨╛╨╖╨┤╨░╤В╤М ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╨░ | Cookie + CSRF |
| `PATCH` | `/api/v1/ai/admin/providers/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╨░ | Cookie + CSRF |
| `DELETE` | `/api/v1/ai/admin/providers/{id}` | ╨Ю╤В╨║╨╗╤О╤З╨╕╤В╤М ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╨░ | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/providers/{id}/models` | ╨Ю╨▒╨╜╨╛╨▓╨╕╤В╤М ╤Б╨┐╨╕╤Б╨╛╨║ ╨╝╨╛╨┤╨╡╨╗╨╡╨╣ | Cookie |
| `GET` | `/api/v1/ai/admin/providers/{id}/status` | ╨б╤В╨░╤В╤Г╤Б ╨┐╤А╨╛╨▓╨░╨╣╨┤╨╡╤А╨░ | Cookie |
| `POST` | `/api/v1/ai/admin/providers/{id}/test` | ╨Я╤А╨╛╨▓╨╡╤А╨╕╤В╤М PolyStrata ╤З╨╡╤А╨╡╨╖ Runtime | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/quotas` | AI-╨║╨▓╨╛╤В╤Л ╨┐╨╛╨╗╤М╨╖╨╛╨▓╨░╤В╨╡╨╗╨╡╨╣ | Cookie |
| `PUT` | `/api/v1/ai/admin/quotas/{user_id}` | ╨г╤Б╤В╨░╨╜╨╛╨▓╨╕╤В╤М AI-╨║╨▓╨╛╤В╤Г | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/runtime-nodes` | ╨б╨┐╨╕╤Б╨╛╨║ Runtime | Cookie |
| `GET` | `/api/v1/ai/admin/runtime-nodes/{id}` | ╨Ф╨╡╤В╨░╨╗╨╕ Runtime | Cookie |
| `POST` | `/api/v1/ai/admin/runtime-nodes/{id}/disable` | ╨Ю╤В╨║╨╗╤О╤З╨╕╤В╤М Runtime | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/runtime-nodes/{id}/enable` | ╨Т╨║╨╗╤О╤З╨╕╤В╤М Runtime | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/runtime-nodes/{id}/revoke` | ╨Ю╤В╨╛╨╖╨▓╨░╤В╤М Runtime | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/runtime-nodes/{id}/rotate-token` | ╨а╨╛╤В╨╕╤А╨╛╨▓╨░╤В╤М device token | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/skills` | ╨Т╤Б╨╡ Skills | Cookie |
| `PATCH` | `/api/v1/ai/admin/skills/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М Skill | Cookie + CSRF |
| `DELETE` | `/api/v1/ai/admin/skills/{id}` | ╨г╨┤╨░╨╗╨╕╤В╤М Skill | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/skills/{id}/assignments` | ╨Э╨░╨╖╨╜╨░╤З╨╡╨╜╨╕╤П Skill | Cookie |
| `POST` | `/api/v1/ai/admin/skills/{id}/assignments` | ╨Э╨░╨╖╨╜╨░╤З╨╕╤В╤М Skill | Cookie + CSRF |
| `DELETE` | `/api/v1/ai/admin/skills/{id}/assignments/{assignment_id}` | ╨г╨┤╨░╨╗╨╕╤В╤М ╨╜╨░╨╖╨╜╨░╤З╨╡╨╜╨╕╨╡ Skill | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/skills/{id}/disable` | ╨Ю╤В╨║╨╗╤О╤З╨╕╤В╤М Skill | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/skills/{id}/enable` | ╨Т╨║╨╗╤О╤З╨╕╤В╤М Skill | Cookie + CSRF |
| `POST` | `/api/v1/ai/admin/skills/import` | ╨Ш╨╝╨┐╨╛╤А╤В ZIP Skill | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/tools` | Tool Registry ╨╕ ╨┐╨╛╨╗╨╕╤В╨╕╨║╨╕ | Cookie |
| `PATCH` | `/api/v1/ai/admin/tools/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М ╨┐╨╛╨╗╨╕╤В╨╕╨║╤Г ╨╕╨╜╤Б╤В╤А╤Г╨╝╨╡╨╜╤В╨░ | Cookie + CSRF |
| `GET` | `/api/v1/ai/admin/workspaces` | ╨Т╤Б╨╡ workspace | Cookie |
| `POST` | `/api/v1/ai/admin/workspaces` | ╨б╨╛╨╖╨┤╨░╤В╤М workspace | Cookie + CSRF |
| `PATCH` | `/api/v1/ai/admin/workspaces/{id}` | ╨Ш╨╖╨╝╨╡╨╜╨╕╤В╤М workspace | Cookie + CSRF |

### AI Runtime — ╨а╨╡╨│╨╕╤Б╤В╤А╨░╤Ж╨╕╤П ╨╕ ╨╕╤Б╤Е╨╛╨┤╤П╤Й╨╕╨╣ ╨║╨░╨╜╨░╨╗ NEXUS AI Runtime

| Метод | Путь | Описание | Доступ |
|---|---|---|---|
| `GET` | `/api/v1/ai/runtime/connect` | ╨Ш╤Б╤Е╨╛╨┤╤П╤Й╨╕╨╣ WebSocket Runtime (Bearer device token) | Bearer |
| `POST` | `/api/v1/ai/runtime/enroll` | ╨а╨╡╨│╨╕╤Б╤В╤А╨░╤Ж╨╕╤П Runtime ╨┐╨╛ ╨╛╨┤╨╜╨╛╤А╨░╨╖╨╛╨▓╨╛╨╝╤Г ╨║╨╛╨┤╤Г | — |
<!-- END GENERATED ENDPOINTS -->

---

## 11. Системные настройки и эффективные значения

`GET /api/v1/system/settings` (право `system.settings.manage`) возвращает
сохранённые значения, реестр и **эффективные** значения:

```json
{
  "values": {
    "limits.max_upload_mb": 512
  },
  "meta": {
    "limits.max_upload_mb": { "requiresRestart": false, "kind": "int" }
  },
  "catalog": [ { "key": "limits.max_upload_mb", "kind": "int", "min": 1, "max": 102400 } ],
  "effective": {
    "limits.max_upload_mb": 512,
    "limits.chunk_max_mb": 64,
    "limits.upload_ttl_hours": 24,
    "limits.share_ttl_hours": 168,
    "limits.export_ttl_hours": 6,
    "limits.trash_retention_days": 30,
    "limits.quota_cloud_gb": 50,
    "limits.quota_minecraft_gb": 100,
    "jobs.max_concurrent": 2,
    "cloud.upload.concurrency": 3,
    "security.session_ttl_hours": 24,
    "security.login_max_failures": 10,
    "security.password_min_length": 12,
    "network.allowed_origins": ["https://nexus.example.com"],
    "network.trust_proxy": false,
    "appearance.background.quality": "auto",
    "appearance.background.defaults": { "count": 260, "quality": "auto" }
  }
}
```

* `values` — то, что сохранено в `system_settings`; ключа может не быть.
* `effective` — то, что действует в данный момент. Для незаданного ключа —
  действующее значение из `nexus.toml`/встроенное, а не `min` реестра.
* `catalog` — реестр из 17 ключей (тот же, что
  `GET /api/v1/system/settings/catalog`).

`PUT /api/v1/system/settings/{key}` сохраняет значение и сразу
инвалидирует кеш. Ответ — нормализованное значение:

```json
{ "success": true, "data": 512 }
```

* Неизвестный ключ → `422 UNKNOWN_SETTING`; значение вне диапазона или
  неверного типа → `422 VALIDATION_ERROR`.
* `requiresRestart: true` только у `network.allowed_origins` и
  `network.trust_proxy`. Они сохраняются сразу, но применяются в `serve()`
  при следующем старте службы: CORS и `trust_proxy` пересобираются до
  `AppState`/маршрутизатора. До перезапуска действует прежний список.
* Значения `network.*` из `system_settings` приоритетнее
  `NEXUS_CORS_ORIGINS`/`NEXUS_TRUST_PROXY` и `nexus.toml`.
* Остальные 15 ключей применяются без перезапуска: лимиты и TTL загрузок,
  квоты по умолчанию, конкурентность загрузок браузера, политика входа,
  минимальная длина пароля, срок хранения корзины, fallback-параметры фона.

`GET /api/v1/public/appearance/background` доступен без сессии и возвращает
эффективный фон для страниц входа и публичных ссылок: встроенные значения,
поверх них `appearance.background.defaults`, затем
`appearance.background.quality`. Ответ содержит только параметры 3D-фона и не
раскрывает остальные системные настройки. Задавать системное значение может
только пользователь с правом `system.settings.manage`.

`GET /api/v1/jobs` дополнительно возвращает `maxConcurrent` — лимит
параллельных задач из `jobs.max_concurrent`, а также счётчик `paused`.
Планировщик читает значение на каждом цикле (1,5 с), поэтому изменение
применяется без перезапуска.

### Фоновые задачи: пауза, возобновление и отмена

`POST /api/v1/jobs/maintenance` принимает `{ "kind": "..." }`, где `kind` —
один из `storage_check`, `verify_objects`, `cleanup_uploads`, `cleanup_shares`,
`cleanup_trash`, `metrics_prune`, `manifests_regenerate`. Для
`verify_objects` доступны `mode`
(`full` — все объекты, `sample` — выборка, по умолчанию) и `samplePercent`
(1–100, по умолчанию 10). Задача создаётся в статусе `pending`; ответ
содержит `id` и `jobType`.

`cleanup_trash` (`system.trash.cleanup`) удаляет записи корзины, у которых
`trashedAt` старше `limits.trash_retention_days`, вместе с поддеревом,
освобождает ссылки и вызывает `delete_zero_ref_objects`: объекты с внешними
ссылками (например, из сборок Minecraft) остаются. Результат задачи содержит
`{ entries, files, retentionDays }`, в аудит пишется `cloud.trash.cleaned`
с тем же количеством. Повторный запуск идемпотентен.

`manifests_regenerate` (`system.manifests.regenerate`) аддитивно пересобирает
манифесты опубликованных и устаревших релизов: добавляет `icon`, если у
сборки появилась иконка, и не меняет файлы или метаданные. Релизы с
неаддитивным расхождением пропускаются и попадают в
`result.skippedReleases`; каждое изменение пишется в аудит
(`minecraft.manifest.regenerated`). Повторный запуск идемпотентен.

Помимо ручного вызова, планировщик сам ставит в очередь обслуживание при
старте и далее раз в час: `system.idempotency.cleanup`,
`system.uploads.cleanup`, `system.shares.cleanup`, `system.trash.cleanup`,
`system.metrics.prune`. Если такая задача уже ожидает или выполнялась
последний час, дубликат не создаётся. Поэтому очистка ключей старше 24 часов
и автоочистка корзины работают без действий администратора.

`POST /api/v1/jobs/{id}/pause` (право `system.jobs.manage`):

* `pending` → сразу `paused`, поле `pausedAt` заполняется;
* `running` → `pauseRequested = true`; исполнитель доводит текущую единицу
  работы до конца, выставляет `paused`, `pausedAt`, сбрасывает флаг и
  освобождает слот планировщика.

`POST /api/v1/jobs/{id}/resume` переводит `paused` → `pending` и сбрасывает
`pauseRequested`/`pausedAt`. `POST /api/v1/jobs/{id}/cancel` для `paused`
даёт `cancelled` сразу, без ожидания исполнителя.

Realtime-события задач: `job.progress`, `job.paused`, `job.resumed`,
`job.completed`, `job.failed`, `job.cancelled`. Элемент списка содержит
`pauseRequested` и `pausedAt`.

### TTL ссылок и архивов

* `POST /api/v1/cloud/shares` без `expiresInHours` берёт
  `limits.share_ttl_hours` (fallback — `storage.share_default_hours`).
  Значение по умолчанию также приходит в `GET /cloud/config` как
  `shareTtlHours` и показывается в форме создания ссылки.
* `POST /api/v1/cloud/exports` и очистка старых ZIP используют
  `limits.export_ttl_hours` (fallback — `storage.export_ttl_hours`); значение
  доступно в `/cloud/config` как `exportTtlHours`.
* `DELETE /api/v1/cloud/exports/{id}` (право `cloud.file.download`) удаляет
  ZIP и запись задачи. Разрешено только владельцу и только для задачи в
  статусе `completed` или `failed`; иначе — `409 EXPORT_NOT_FINISHED`.
  После удаления `GET /cloud/exports/{id}` и
  `GET /cloud/exports/{id}/file` отвечают `404`.

---

## 12. Проверки контракта

* `cargo test --test openapi_tests` — каждый зарегистрированный маршрут описан
  в спецификации, path-параметры объявлены, операции уникальны.
* `path_count_matches_route_registrations` — число шаблонов путей в
  спецификации совпадает с числом `.route(` в `backend/src`.
* `node scripts/gen-api-doc.mjs` — повторяет проверку числа путей и
  перегенерирует таблицу §10; при расхождении завершается с ненулевым кодом.

---

## AI Workspace API

Префикс `/api/v1/ai/`. Пользовательские маршруты: providers, conversations,
messages/regenerate/edit/stop, attachments, tasks, executions, approvals,
workspaces, skills, tools. Административные — `/api/v1/ai/admin/*`
(`ai.admin.manage`). Runtime enrollment: `POST /api/v1/ai/runtime/enroll`;
управляющий WebSocket: `GET /api/v1/ai/runtime/connect` (Bearer device token).

Полный список, события `ai.*`, конверт протокола и коды ошибок:
[`AI_WORKSPACE.md`](AI_WORKSPACE.md). OpenAPI 3.1 обновляется вместе с
`backend/src/api/openapi/ai.rs`.
