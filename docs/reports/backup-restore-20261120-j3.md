# J3. Backup / restore / update на Windows

Дата: 20.11.2026, 12:34–12:50 MSK.  
Стенд: Windows 11 Pro 10.0.26300, PostgreSQL 17.12, служба `NexusWorkspace`,
`C:\NexusWorkspace`, активная БД на момент начала — `nexus`.

## 1. Что изменено в скриптах

| Файл | Изменения |
|---|---|
| `deployment/backup.ps1` | `pg_dump --format=custom` с явным `--file`; пароль только через временный `PGPASSFILE`; служба останавливается **до** дампа и копирования; манифест, SHA-256 дампа и `SHA256SUMS`; запуск службы в `finally` |
| `deployment/restore.ps1` | `pg_restore --exit-on-error`; восстановление в **новую** БД (`-TargetDatabase`), `DROP DATABASE ... WITH (FORCE)` и `createdb`; `-SwitchInstall` переключает `.env`; проверка `nexus check`, health=200 и входа; данные — только при остановленной службе |
| `deployment/update-windows.ps1` | служба останавливается **первой**, затем backup; `nexus.previous.exe`; при ошибке миграции — откат БД из дампа + бинарника + запуск службы; `-SimulateMigrationFailure` |
| `deployment/nexus-common.ps1` | разбор `DATABASE_URL`, `.pgpass`, пути PostgreSQL, `Stop/Start-Service`, health, login, запуск процесса без `NativeCommandError` |

Пароль PostgreSQL не попадает в командную строку: `pg_dump`/`pg_restore`/`psql`
получают только `--host/--port/--username`, а пароль — из временного `.pgpass`
(`PGPASSFILE`), который удаляется в `finally`. Поле базы в `.pgpass` — `*`,
иначе служебное подключение к `postgres` для `drop/create database` не находит
пароль.

## 2. Backup при остановленной службе

```powershell
.\deployment\backup.ps1 -BackupDir C:\NexusBackups -InstallDir C:\NexusWorkspace
```

Факт (12:34:00):

```text
==> Остановка службы NexusWorkspace
    служба остановлена
==> Дамп базы nexus (custom format)
    6962569 байт, SHA-256 d28a665e597491928a5d9c4744659d056b4ed26992f963bb4ebaa277e588b8ef
==> Копирование каталога данных
    скопировано 1 073 741 824 байт из C:\NexusWorkspace\data
==> Запуск службы NexusWorkspace
    Running
```

`C:\NexusBackups\20261120-123400\manifest.json`: `format=nexus-backup/2`,
`pgDumpVersion=pg_dump (PostgreSQL) 17.12`, `serviceStopped=true`,
`data.bytes=1073741824`. `pg_restore --list` подтверждает `Format: CUSTOM`,
`TOC Entries: 219`.

Лог: [`logs/j3-backup.log`](logs/j3-backup.log).

## 3. Restore в новую БД

```powershell
$env:NEXUS_TEST_PASSWORD = '…'
.\deployment\restore.ps1 -BackupPath C:\NexusBackups\20261120-123400 `
    -TargetDatabase nexus_restore_j3b -DropExisting -SwitchInstall `
    -SkipData -TestUsername j2admin -Force
```

Факт (12:46):

```text
==> Подготовка базы nexus_restore_j3b
    создана nexus_restore_j3b (owner nexus)
==> pg_restore в nexus_restore_j3b
    готово (pg_restore)
==> nexus check
    миграции: 13; Проверка пройдена
==> Запуск службы NexusWorkspace
    Running
    login: 200, /auth/me: 200
```

`-SkipData` во втором прогоне: каталог `data` уже восстановлен первым прогоном,
повторное копирование 1 ГиБ не требуется. `.env` переключён на
`postgres://nexus:***@127.0.0.1:5432/nexus_restore_j3b`.

Проверка старого объекта после восстановления (через Caddy):

```text
login=200
download=206 Content-Range=bytes 0-1048575/1073741824
nexus_restore_j3b entries=4
```

То есть вход работает, запись `f174f102-…` и её 1-ГиБ объект доступны.
JSON: [`logs/j3-restore.json`](logs/j3-restore.json), лог:
[`logs/j3-restore-final.log`](logs/j3-restore-final.log).

## 4. Update: успешный прогон

```powershell
.\deployment\update-windows.ps1 -SkipBuild -TestUsername j2admin
```

Факт (12:47):

```text
==> Остановка службы
==> Резервная копия (20261120-124707, serviceStopped=true)
==> Замена файлов: nexus.exe, nexus.previous.exe, frontend
==> Миграции: exit 0
==> Запуск службы: Running, health=200
    login: 200, /auth/me: 200
```

JSON: [`logs/update-20261120-124707.json`](logs/update-20261120-124707.json),
лог: [`logs/j3-update-ok.log`](logs/j3-update-ok.log).

## 5. Update: имитация неудачной миграции и откат

Симуляция — повреждение checksum уже применённой миграции в
`_sqlx_migrations` (реальная ошибка sqlx, не подмена кода возврата):

```sql
update _sqlx_migrations
   set checksum = decode(repeat('00', 48), 'hex')
 where version = (select min(version) from _sqlx_migrations);
```

```powershell
.\deployment\update-windows.ps1 -SkipBuild -SimulateMigrationFailure
```

Факт (12:48):

```text
==> Имитация неудачной миграции: повреждение checksum
UPDATE 1
==> Миграции
    Error: не удалось применить миграции
    Caused by:
        migration 1 was previously applied but has been modified
==> Миграции не применились - откат
==> Откат базы nexus_restore_j3 из C:\NexusBackups\20261120-124836
    создана nexus_restore_j3 (owner nexus)
    готово (pg_restore)
==> возвращён nexus.previous.exe
    служба снова Running, health=200
ОШИБКА: Обновление отменено: миграция завершилась с кодом 1; выполнен откат.
```

Код возврата скрипта — 1 (ожидаемо: обновление отменено). Состояние после
отката: `rollback=true`, `databaseRestored=true`, `binaryRestored=true`,
`serviceAfterRollback=Running`, `healthAfterRollback=200`,
`checkAfterRollback=0`. Повторный `nexus.exe migrate` → 0, checksum миграции
восстановлен (`870a2711…`), вход 200/200.

JSON: [`logs/update-20261120-124836.json`](logs/update-20261120-124836.json),
лог: [`logs/j3-update-fail.log`](logs/j3-update-fail.log).

## 6. Найденные и исправленные дефекты

1. **`.pgpass` без wildcard для БД.** Строка содержала имя целевой БД, поэтому
   `psql -d postgres` (drop/create) не находил пароль и ждал ввод. Исправлено:
   поле базы `*`. После исправления restore проходит без интерактивного ввода.
2. **`& nexus.exe migrate 2>&1` при `ErrorActionPreference=Stop`.** PowerShell 5.1
   превращал stderr в `NativeCommandError` и обрывал скрипт до разбора кода
   возврата, поэтому откат не запускался. Добавлен `Invoke-NexusProcess`
   (`Start-Process` с redirect в файлы); после этого откат выполняется штатно.

Оба дефекта воспроизведены и исправлены в рамках сессии; итоговые прогоны выше
сделаны уже на исправленных скриптах.

## 7. Вывод

`backup → restore в новую БД → update → имитация сбоя → откат` воспроизводимы
по инструкции [`docs/INSTALL-WINDOWS.md`](../INSTALL-WINDOWS.md). После каждого
шага `nexus check` зелёный, служба `Running`, health=200, вход работает.
Финальное состояние стенда: БД `nexus_restore_j3b`, служба `NexusWorkspace`
Running, Caddy Running.
