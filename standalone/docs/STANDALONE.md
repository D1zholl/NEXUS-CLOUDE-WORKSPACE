# NEXUS WORKSPACE 1.0.0 — Standalone (Windows 11 x64)

## Назначение

Переносимый пользовательский релиз NEXUS: production-backend, production-
frontend, portable PostgreSQL 17, NEXUS AI Runtime и native-лаунчер. Запуск не
требует Rust, Cargo, Node.js, npm, Visual Studio, Git, IDE, системного
PostgreSQL или прав администратора.

## Структура

```
NEXUS.exe                 native launcher (GUI + CLI)
Start.bat ...             управляющие сценарии
backend\nexus.exe         production backend (Rust, axum, SQLx)
frontend\                 production SPA (React + Vite build)
database\bin|lib|share    portable PostgreSQL 17
runtime\                  nexus-ai-runtime.exe + пример конфигурации
config\                   instance.json, secrets.json, nexus.toml
data\postgres\            изолированный кластер PostgreSQL
storage\                  пользовательские файлы Cloud Storage
logs\                     launcher.log, backend.log, database.log
backups\                  резервные копии
cache\                    временные файлы
docs\, LICENSES\, tools\  документация, лицензии, проверка целостности
```

## Точки входа

- Лаунчер: `NEXUS.exe` (GUI), CLI-режимы `--start`, `--stop`, `--restart`,
  `--status`, `--diagnostics`, `--provision`, `--backup`, `--restore`,
  `--backend`.
- Backend: `backend\nexus.exe serve` (также `migrate`, `create-admin`,
  `check --json`, `user`, `objects`).
- Frontend: статические файлы `frontend\` отдаёт сам backend с SPA fallback.
- PostgreSQL: `database\bin\pg_ctl.exe` управляет кластером `data\postgres`.
- AI Runtime: `runtime\nexus-ai-runtime.exe run --config runtime\runtime.toml`
  (опционально, отдельный процесс).

## Пути

Все пути вычисляются от каталога, в котором находится `NEXUS.exe`, а не от
текущей рабочей директории. В `config\nexus.toml` используются относительные
пути (`frontend`, `logs`). В `config\instance.json` нет абсолютных путей.
Поэтому папку можно переносить между дисками, включая пути с пробелами и
кириллицей. PostgreSQL запускается с `-D <фактический data\postgres>`.

## Жизненный цикл

1. Проверка единственного экземпляра (named mutex `Local\NEXUS-Workspace-Standalone-Launcher`).
2. Проверка структуры релиза и версии.
3. Чтение/создание `config\instance.json` и `config\secrets.json`.
4. Проверка портов. Если порт занят чужим процессом, выбирается свободный
   локальный порт; чужой процесс не завершается.
5. `initdb` (однократно) с случайным паролем, SCRAM-SHA-256, UTF-8,
   `listen_addresses='127.0.0.1'`.
6. `pg_ctl start -w`, затем `pg_isready` (реальная проверка готовности).
7. Создание базы `nexus` при необходимости.
8. `nexus migrate` (SQLx migrations).
9. Регистрация локального тома `storage\`, если томов ещё нет.
10. Проверка администратора. Если его нет — интерактивный
    `nexus create-admin --username admin` в отдельной консоли (rpassword,
    пароль без эха, не в аргументах и не в логах).
11. Запуск `nexus serve` с окружением, указывающим на эту папку.
12. Ожидание `GET /api/v1/health` со статусом `ok` (до 180 с, без
    фиксированного Sleep).
13. Открытие браузера пользователя по `http://127.0.0.1:<port>`.

Остановка: файл-сигнал `data\run\shutdown.request` → backend завершает
HTTP-сервер, планировщик, advisory lock и пул БД штатно; затем
`pg_ctl stop -m fast -w`. Принудительное завершение — только аварийный
fallback после таймаута.

## База данных

- Включён PostgreSQL 17.6 (официальные Windows x64 binaries EDB).
- Кластер `data\postgres`, кодировка UTF8, locale C.
- Роль `nexus` — суперпользователь только внутри изолированного кластера;
  пароль генерируется случайно (48 hex-символов).
- `pg_hba.conf` разрешает только local, 127.0.0.1/32 и ::1/128 по SCRAM.
- Порт по умолчанию 55432 (не 5432), чтобы не конфликтовать с системным
  PostgreSQL. Меняется в `config\instance.json` (`db_port`).
- `DATABASE_URL` не хранится в `nexus.toml`: лаунчер передаёт его дочернему
  процессу через окружение. Пароль хранится в `config\secrets.json` с ACL
  текущего пользователя.

## Frontend

`frontend\` — результат `npm run build` (Vite production, без dev-server).
API и WebSocket используют относительные пути `/api/v1...`, поэтому
приложение работает на любом локальном порту. SPA fallback реализован в
backend. Node.js для запуска не нужен.

## AI Runtime

`runtime\nexus-ai-runtime.exe` — отдельный процесс, исходящее WSS-соединение,
без входящих портов. Для работы нужны локальная PolyStrata (OpenAI-
compatible API) и enrollment через AI Management. Без них NEXUS запускается,
остальные модули работают, AI-модуль показывает Runtime offline.
См. `docs\AI_RUNTIME.md`.

## Безопасность

- Только loopback: backend и PostgreSQL не публикуются во внешнюю сеть.
- CORS по умолчанию не ослаблен (same-origin), CSRF, серверные сессии,
  RBAC и проверки путей сохранены без изменений.
- Нет автоматических правил брандмауэра, служб, автозапуска и записей
  реестра. UAC не требуется.
- Пароли и токены не пишутся в журналы. Диагностика не печатает секреты.
- Резервные копии содержат `config\secrets.json`: храните их защищённо.

## Обновление и откат

Замените программные файлы (`NEXUS.exe`, `backend`, `frontend`, `database`,
`runtime`, `docs`, `LICENSES`, `tools`), сохранив `config`, `data`,
`storage`, `logs`, `backups`, `cache`. Миграции применяются при старте.
Automatic update-сервис намеренно не включён; проверка целостности —
`tools\Verify-Integrity.ps1` и `manifest.json`.

## Ограничения

- Релиз собран и проверен на Windows 11 x64. Чистая виртуальная машина без
  dev-инструментов в рамках данной сборки недоступна; независимость от
  dev-среды проверяется запуском с очищенным PATH и в перенесённом каталоге.
- AI Runtime не содержит PolyStrata и моделей: это внешний локальный сервис.
- Автоматическое обновление и Windows-служба не входят в базовый режим.
- Порт 8080/55432 может быть занят; лаунчер выберет свободный локальный порт.
