# Контракт MyLauncher ↔ NEXUS WORKSPACE

Документ зафиксирован по фактическому коду клиента `MC-launcher`
(`src-tauri/src/modpack.rs`, `src-tauri/src/lib.rs`, `src/views/RepositoryView.tsx`),
а не по README или предположениям. Клиент — источник истины. Серверная часть
NEXUS адаптируется под него; код лаунчера не изменяется.

Версия клиента в репозитории: MyLauncher `0.1.0` (`user_agent` — `MyLauncher/0.1.0`).

## 1. Транспорт и адреса

Лаунчер принимает только базовый URL репозитория:

* схема `https://` либо `http://` для локальной сети (`localhost`, `*.local`,
  приватные IPv4/IPv6, loopback, link-local);
* без логина/пароля, без query и fragment;
* путь может быть вложенным (`https://host/nexus`), клиент добавляет `/`;
* все запросы — `GET`, заголовок `User-Agent: MyLauncher/0.1.0`;
* редиректы не выполняются (`redirect::Policy::none()`);
* таймаут каталога/манифеста 30 с, файлов 600 с;
* CORS не нужен: HTTP-вызовы делает Rust-бэкенд Tauri, а не WebView.

Итоговые пути (склеиваются через `Url::join` от базы):

| Назначение | Метод и путь | Query |
|---|---|---|
| Каталог сборок | `GET {base}/api/packs` | `channel=stable\|beta` |
| Версии сборки | `GET {base}/api/packs/{pack_id}/versions` | `channel=stable\|beta` |
| Манифест версии | `GET {base}/api/packs/{pack_id}/versions/{version}/manifest` | `channel=stable\|beta` |
| Файл сборки | `GET {base}/api/files/sha256:{hex}` | `packId`, `version`, `channel` |

`pack_id` проверяется клиентом: 3–64 символа, только `a-z0-9-`, первый и
последний — буква/цифра. `version` — `a-zA-Z0-9._-+`, до 64 символов.

### Аутентификация

В коде клиента **нет** ни `Authorization`, ни `X-Launcher-Token`, ни токена в
query. `repository_url()` явно запрещает query и учётные данные в URL. Выдача и
отзыв токена клиентом не вызываются, проверки подключения нет.

Следствие: репозиторные endpoints обязаны быть доступны без токена. Приватные
сборки этому клиенту недоступны в принципе. NEXUS дополнительно принимает
`Authorization: Bearer` (scope `launcher.catalog` / `launcher.artifacts`) для
будущих клиентов, но stock MyLauncher его не отправляет. Это ограничение
зафиксировано в разделе 7.

## 2. Каталог: `GET /api/packs?channel=stable|beta`

Клиент десериализует (camelCase, лишние поля игнорируются):

```json
{
  "packs": [
    {
      "id": "create-expert",
      "name": "Create Expert",
      "description": "Сборка для экспертов Create",
      "authors": ["NEXUS"],
      "categories": ["tech"],
      "iconUrl": "https://nexus.example.com/api/packs/create-expert/icon",
      "latestVersion": "1.4.0",
      "channel": "stable"
    }
  ]
}
```

Обязательные поля: `id`, `name`, `description`, `authors`, `categories`,
`latestVersion`, `channel`. `iconUrl` — `null` или URL. UI показывает иконку
только если итоговый URL начинается с `https:`; для локального HTTP иконка
может быть скрыта, это не ошибка подключения.

Семантика: в выдачу попадают только опубликованные, неархивные сборки,
у которых есть опубликованный (`status = published`) релиз запрошенного канала.
`latestVersion` — версия самого нового такого релиза. `channel` в ответе равен
запрошенному.

## 3. Версии: `GET /api/packs/{pack_id}/versions?channel=...`

```json
{
  "versions": [
    { "version": "1.4.0", "channel": "stable", "updatedAt": 1730000000 },
    { "version": "1.3.0", "channel": "stable", "updatedAt": 1720000000 }
  ]
}
```

`updatedAt` — Unix-время в секундах (`i64`), не ISO-строка. Первый элемент
клиент выбирает по умолчанию как «последний». В список включаются `published`
и `deprecated` релизы канала; `archived` исключаются. Опубликованные сортируются
выше устаревших.

## 4. Манифест: `.../versions/{version}/manifest?channel=...`

### 4.1. Полная схема, которую читает парсер

```json
{
  "manifestVersion": 1,
  "packId": "create-expert",
  "version": "1.4.0",
  "channel": "stable",
  "name": "Create Expert",
  "description": "Сборка для экспертов Create",
  "authors": ["NEXUS"],
  "categories": ["tech"],
  "minecraft": {
    "version": "1.20.1",
    "loader": "forge",
    "loaderVersion": "47.4.10"
  },
  "java": { "major": 17 },
  "files": [
    {
      "path": "mods/create-0.5.1.jar",
      "hash": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "size": 1234567,
      "url": "/api/files/sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "required": true
    }
  ],
  "settings": {
    "memoryMb": 4096,
    "jvmArgs": "-XX:+UseG1GC",
    "windowWidth": 1280,
    "windowHeight": 720,
    "fullscreen": false
  }
}
```

Правила парсера (Rust `serde`, `rename_all = "camelCase"`):

| Поле | Тип | Обязательное | Ограничения клиента |
|---|---|---|---|
| `manifestVersion` | u32 | да | ровно `1` |
| `packId` | string | да | совпадает с запросом, 3–64 `[a-z0-9-]` |
| `version` | string | да | совпадает с запросом, `valid_component` |
| `name` | string | да | непустое для UI |
| `description` | string | нет | по умолчанию `""` |
| `authors` | string[] | нет | по умолчанию `[]` |
| `categories` | string[] | нет | по умолчанию `[]` |
| `minecraft.version` | string | да | `a-zA-Z0-9._-+`, ≤64; точный id Mojang |
| `minecraft.loader` | string | да | `vanilla`, `fabric`, `forge`, `neoforge` |
| `minecraft.loaderVersion` | string/null | для не-vanilla | ≤128, без `/` и `\` |
| `java.major` | u8 | да | ≥8 и **ровно** требуется версией MC |
| `files[]` | массив | да | ≤100 000 записей |
| `files[].path` | string | да | правила ниже |
| `files[].hash` | string | да | ровно `sha256:` + 64 hex |
| `files[].size` | u64 | да | ≤8 ГиБ, сумма ≤20 ГБ |
| `files[].url` | string | да | строковое равенство `/api/files/{hash}` |
| `files[].required` | bool | да | `false` — ошибка скачивания пропускается |
| `settings` | object | нет | неизвестные поля игнорируются |

`quilt` клиент отвергает (`validate_loader_selection`). Поэтому публикация
Quilt-сборки в лаунчер блокируется на сервере, хотя внутренний NEXUS-формат
её допускает.

### 4.2. Формат версии загрузчика

Клиент передаёт `loaderVersion` в установщик как есть:

* Fabric — версия Fabric Loader (`0.15.11`);
* Forge — **суффикс** без версии MC (`47.4.10` для `1.20.1-47.4.10`);
* NeoForge — версия NeoForge (`21.1.209`, допустим и `1.20.4-20.4.237`);
* vanilla — `loaderVersion` отсутствует или `null`.

### 4.3. Java

Клиент получает обязательную Java из метаданных Mojang
(`javaVersion.majorVersion`) и сравнивает с `java.major` **на равенство**.
Если не совпало — установка прерывается. Резервное правило клиента:

* MC `1.0–1.16.x` → 8;
* `1.17.x` → 16;
* `1.18–1.19.x` → 17;
* `1.20.0–1.20.4` → 17;
* `1.20.5+`, `21.x–24.x` → 21;
* `25.x+` → 25.

NEXUS хранит `java_major` сборки; при публикации проверяется совпадение с этим
правилом (и с `javaVersion.majorVersion` Mojang, если метаданные доступны).

### 4.4. Пути файлов

Клиент (`safe_pack_path` + `safe_repository_path`) принимает только:

* относительный путь с прямыми `/`, непустые сегменты;
* сегмент не `.`/`..`, не заканчивается пробелом или точкой;
* без `\`, `:`, управляющих символов;
* без зарезервированных имён Windows (`CON`, `PRN`, `AUX`, `NUL`, `COM1–9`, `LPT1–9`);
* первый сегмент не `versions`, `assets`, `libraries`, не `.launcher_meta.json*`;
* **каждый символ** — ASCII alphanumeric или один из ` _./+@()-§`;
  кириллица и прочий Unicode запрещены;
* расширение (регистр не важен) из списка: `jar, zip, json, json5, toml, bak,
  backup, cfg, conf, properties, txt, md, png, jpg, jpeg, webp, mcmeta, dat,
  nbt, snbt, mcfunction, vsh, fsh, glsl, lang, yml, yaml, xml, ini`; имя
  `.defaults` разрешено без расширения;
* пути уникальны без учёта регистра (клиент приводит к lower-case).

Категория и `side` клиентом не читаются. Поэтому NEXUS:

* в манифест включает только `side = client|both`; `server`-файлы не должны
  попадать в клиентский инстанс;
* отключённые (`enabled = false`) файлы не включает;
* путь и расширение проверяет по правилам выше перед публикацией.

### 4.5. Файлы, SHA-256, Range

* `hash` — `sha256:` + **нижний** hex (клиент сравнивает с `format!("{:x}")`;
  верхний регистр приведёт к вечной ошибке «хэш не совпал»);
* `url` — строго относительный `/api/files/{hash}`; абсолютные URL, другой
  домен, другой путь и query в самом `url` запрещены. Клиент сам добавляет
  `packId`, `version`, `channel`;
* размер ответа должен совпасть с `size`, иначе ошибка;
* при обрыве клиент шлёт `Range: bytes=N-` и `If-Range: "sha256-{hex}"`.
  Сервер обязан вернуть `206` с корректным `Content-Range: bytes N-M/total`
  либо `200` (тогда клиент начнёт заново). `416` при недостижимом диапазоне;
* повторные попытки: 3, с `Retry-After`; retryable — 408, 429, 500, 502, 503, 504.

## 5. Обновление и удаление файлов

Клиент хранит в `.launcher_meta.json` полный список установленных файлов.
При обновлении:

1. Скачивает манифест новой версии и требует совпадения `minecraft.version`,
   `loader` и `loaderVersion` с установленными. Смена загрузчика/MC — только
   новый инстанс.
2. Сверяет локальные файлы по `size` и SHA-256; совпавшие не скачивает.
3. Полный список нового манифеста — источник истины: файлы, которых в нём нет,
   удаляются из инстанса (с резервной копией в staging).
4. `required: false` при ошибке скачивания пропускается; `required: true`
   прерывает установку.
5. Пользовательские файлы по путям, которых не было в старой сборке, не
   перезаписываются.

Сервер не передаёт «явные удаления»: достаточно отдать новый полный список.

## 6. Каналы

* Ровно два значения: `stable` и `beta`, регистр важен.
* `channel` — обязательный query-параметр всех трёх JSON-вызовов.
* «Последняя версия» — первый элемент `versions[]` и `latestVersion` каталога.
* `archived` исключается; `deprecated` остаётся доступным по точной версии, но
  не должен выбираться как latest.

## 7. Несовместимости и ограничения

| Требование | Факт клиента | Решение NEXUS |
|---|---|---|
| Токен лаунчера | Клиент не отправляет токен и запрещает query/учётные данные в URL | Репозиторные endpoints публичные; Bearer поддержан опционально для будущих клиентов. Приватные сборки stock MyLauncher не увидит. |
| Приватные сборки | Нет способа передать токен | Только публичные; приватные → 404 без токена. Требуется изменение клиента — по отдельному согласованию. |
| Проверка подключения | Отдельного вызова нет | Не требуется; каталог и есть проверка. |
| Выдача/отзыв токена | Не вызываются | Оставлены в `/api/v1/launcher/auth/*` для других клиентов. |
| Quilt | `validate_loader_selection` отвергает | Публикация Quilt блокируется launcher-ready проверкой. |
| Side | Не читается | Сервер фильтрует `client`/`both`. |
| Абсолютные download_url | Запрещены | Только `/api/files/{hash}`. |
| `127.0.0.1`/внутренние URL | Запрещены | В манифест не попадают. |
| CORS | Запросы из Rust Tauri | Не требуется. |

## 8. Соответствие «вызов лаунчера → endpoint NEXUS»

| Вызов клиента | NEXUS до работы | NEXUS после | Действие |
|---|---|---|---|
| `GET /api/packs?channel=` | нет | `GET /api/packs` | реализовано |
| `GET /api/packs/{id}/versions?channel=` | нет | есть | реализовано |
| `GET /api/packs/{id}/versions/{v}/manifest?channel=` | нет (другой путь и формат) | есть | реализовано |
| `GET /api/packs/{id}/icon` | нет | есть | реализовано |
| `GET /api/files/sha256:{hex}?packId=&version=&channel=` | нет (`/api/v1/launcher/artifacts/{uuid}`) | есть | реализовано, Range/416 |
| Bearer-токен | не отправляется | опционален | документировано |
| `latest` по каналу | не было каналов | `channel` у релизов | миграция + выборка |
| Снимок манифеста | только внутренний 1.0 | `launcher_manifest` + hash | миграция |
| Удаление файлов | полный список | полный список | без изменений |
| `iconUrl` | относительный старый | абсолютный из `NEXUS_PUBLIC_URL`, если задан | каталог |

Старые `/api/v1/launcher/*` и внутренний манифест `schema_version: "1.0"`
сохранены без изменений контракта.
