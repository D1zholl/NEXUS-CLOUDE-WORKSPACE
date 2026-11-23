# K2. Backup бинарных объектов на Windows

Дата: 20.11.2026, 13:45–13:57 MSK.  
Стенд: Windows 11 Pro 10.0.26300, Windows PowerShell 5.1, PostgreSQL 17.12,
служба `NexusWorkspace` (Running), Caddy (Running), БД `nexus_restore_j3b`.
Тестовый том: `K2 Small Volume`, `C:\NexusK2Volume` (scope `cloud`), два файла
Cloud: `small-object.bin` (262 144 байт) и `second-object.bin` (131 072 байт).

## 1. Что реализовано

| Файл | Назначение |
|---|---|
| `deployment/objects-common.ps1` | длинные пути `\\?\`, потоковый SHA-256, копирование с повторами, VSS, psql, icacls |
| `deployment/backup-objects.ps1` | full/incremental, VSS/Pause/Live, манифест, сверка со `storage_objects`, Task Scheduler, retention |
| `deployment/restore-objects.ps1` | цепочка наборов, robocopy, SHA-256, сверка со `storage_objects`, icacls, health |
| `docs/ADMIN_GUIDE.md` §10.2.1, `docs/DEPLOYMENT.md` §9.2–9.5 | эксплуатация, права, Defender, ограничения |

Выбранный подход к открытым файлам — **VSS-снимок** (`Win32_ShadowCopy`,
`ClientAccessible`) с автоматическим откатом к остановке службы (`Auto`).
VSS требует администратора и NTFS; снимок crash-consistent. `robocopy` не
читает `\\?\GLOBALROOT\Device\...` (ошибка 53), поэтому используется
временная символьная ссылка, удаляемая вместе со снимком в `finally`.

## 2. Полный backup

```powershell
.\deployment\backup-objects.ps1 `
  -BackupDir C:\NexusK2AcceptanceBackups -VolumeRoot C:\NexusK2Volume `
  -Mode Full -Consistency Auto -Label k2-acceptance -Force
```

Фактический вывод (лог [`logs/k2-backup.log`](logs/k2-backup.log)):

```text
==> Подготовка резервной копии объектов в C:\NexusK2AcceptanceBackups\objects-20261120-135451
    VSS-снимок C:\

==> Том K2 Small Volume: C:\NexusK2Volume
    SHA-256 .nexus/objects/d8/4a/82e6f71702bf-f1c750adcea5
    файлов в наборе: 1, новых/изменённых: 1, без изменений: 0, байт: 262 144
    сверка со storage_objects: проверено 1, все совпадают

==> Готово: C:\NexusK2AcceptanceBackups\objects-20261120-135451
  файлов: 1, байт: 262 144, согласованность: vss
  манифест: manifest.json, SHA256SUMS
  storage_objects: проверено 1, отсутствует 0, несовпадений 0, вне БД 0
```

`manifest.json` (полностью — [`logs/k2-manifest.json`](logs/k2-manifest.json)):
`format=nexus-objects/1`, `mode=Full`, `consistency=vss`,
`database.verified=1`, `database.hashMismatch=0`. `SHA256SUMS`
([`logs/k2-SHA256SUMS.txt`](logs/k2-SHA256SUMS.txt)):

```text
dc8cc5df284c87176fd0ca31ba8656987a02c57d4f5123c8c9f46e04c0283b21  manifest.json
d84a82e6f71702bf44cabd6440389c763532b467c71ce98b4cc4b6adb1212780  volumes/e52e90ce-1818-4bb7-8b6e-6b3d84c4338f/.nexus/objects/d8/4a/82e6f71702bf-f1c750adcea5
```

`backup-result.json`: `pass=true`, `consistencyUsed=vss`,
`serviceStopped=false`, `files=1`, `bytes=262144`,
`manifestSha256=dc8cc5df…`.

## 3. Удаление каталога и ожидаемая ошибка

```powershell
Remove-Item C:\NexusK2Volume -Recurse -Force
```

```text
Before: root exists=True, files=2
After delete: root exists=False
Download after delete: HTTP 500, bytes=148
{"error":{"code":"STORAGE_ERROR", ...}}
```

Это подтверждает, что скачивание действительно читает объект с тома, а не из
кэша/БД.

## 4. Restore полного набора

```powershell
.\deployment\restore-objects.ps1 `
  -BackupPath C:\NexusK2AcceptanceBackups\objects-20261120-135451 -Force
```

Фактический вывод (лог [`logs/k2-restore.log`](logs/k2-restore.log),
JSON [`logs/k2-restore-result.json`](logs/k2-restore-result.json)):

```text
==> Восстановление бинарных объектов
    том K2 Small Volume: C:\NexusK2Volume -> C:\NexusK2Volume

==> Остановка службы NexusWorkspace
    служба остановлена

==> Набор objects-20261120-135451 (Full)

==> Права icacls для NT SERVICE\NexusWorkspace
    C:\NexusK2Volume

==> Проверка SHA-256: файлов 1
    проверено 1, отсутствует 0, несовпадений 0

==> Сверка со storage_objects
    объектов 1: совпало 1, отсутствует 0, несовпадений 0, вне БД 0

==> Восстановление объектов завершено
  файлов: 1, байт: 262 144
  права: NT SERVICE\NexusWorkspace

==> Запуск службы NexusWorkspace
    Running
    health: 200
```

`icacls C:\NexusK2Volume`:

```text
NT SERVICE\NexusWorkspace:(OI)(CI)(M)
NT AUTHORITY\СИСТЕМА:(I)(OI)(CI)(F)
BUILTIN\Администраторы:(I)(OI)(CI)(F)
BUILTIN\Пользователи:(I)(OI)(CI)(F)
```

## 5. Скачивание после restore

```text
first:  HTTP 200 size=262144 sha=d84a82e6f71702bf44cabd6440389c763532b467c71ce98b4cc4b6adb1212780 match=True
second: HTTP 200 size=131072 sha=b874e9a21ef2fda64e9319fd1788f145937659964568836dbbc3d6cedf34ce3b match=True
```

JSON: [`logs/k2-download.json`](logs/k2-download.json), заголовки:
[`logs/k2-download-headers.txt`](logs/k2-download-headers.txt).
Служба `Running`, `/api/v1/health` → 200 `{"status":"ok","database":"up"}`.

## 6. Incremental и цепочка

Второй файл загружен в тот же том, затем:

```powershell
.\deployment\backup-objects.ps1 -BackupDir C:\NexusK2AcceptanceBackups `
  -VolumeRoot C:\NexusK2Volume -Mode Incremental -Consistency Auto `
  -Label k2-acceptance-inc -Force
```

```text
==> Том K2 Small Volume: C:\NexusK2Volume
    файлов в наборе: 1, новых/изменённых: 1, без изменений: 1, байт: 131 072
    сверка со storage_objects: проверено 2, все совпадают
```

Манифест incremental содержит `base=C:\NexusK2AcceptanceBackups\objects-20261120-135451`
и только новый объект. Каталог тома снова удалён, восстановление выполнено
каталогом (full + incremental):

```text
==> Набор objects-20261120-135451 (Full)
==> Набор objects-20261120-135600 (Incremental)
    проверено 2, отсутствует 0, несовпадений 0
    объектов 2: совпало 2, отсутствует 0, несовпадений 0, вне БД 0
    Running
    health: 200
```

Логи: [`logs/k2-backup-incremental.log`](logs/k2-backup-incremental.log),
[`logs/k2-restore-chain.log`](logs/k2-restore-chain.log),
[`logs/k2-restore-chain-result.json`](logs/k2-restore-chain-result.json).
После chain-restore скачаны оба файла, SHA-256 совпали.

Финальный прогон уже готовым кодом (после правок цепочки и первого
incremental) повторён на той же цепочке: `2/2` файла, `storage_objects 2/2`,
`health=200`; оба скачивания снова HTTP 200 и совпали по SHA-256. Лог:
[`logs/k2-restore-final.log`](logs/k2-restore-final.log),
JSON: [`logs/k2-restore-final.json`](logs/k2-restore-final.json).

## 7. Открытый файл и junction

В `.nexus/staging` создан `locked-probe.bin` 4096 байт и удерживался с
`FileShare.None`; запущен `backup-objects.ps1 -Consistency Auto`:

```text
    VSS-снимок C:\
    SHA-256 .nexus/staging/locked-probe.bin
    файлов в наборе: 3, новых/изменённых: 3, без изменений: 0, байт: 397 312
    сверка со storage_objects: проверено 2, все совпадают
copied exists=True sha_match=True
```

То есть VSS снимает блокировку открытого файла; при недоступности VSS скрипт
останавливает службу (`-Consistency Pause`, проверено: служба остановлена,
копия снята, служба снова `Running`). `-Consistency Live` оставлен для
диагностики и не гарантирует согласованность.

Отдельно проверено, что junction внутри `.nexus/objects` не обходится:
создан `objects/link` → внешний каталог с `outside.txt`, запущены Full
(`robocopy /XJ`) и Incremental (ручной обход с пропуском reparse points).
В обоих наборах `outside.txt` отсутствует, в манифесте только
`objects/real/inside.bin` (Full: 1 файл, Incremental: 0 новых).

## 8. Расписание, retention и удаление

Задача проверена с тестовым именем и затем удалена:

```text
задача NexusObjectsBackupK2Test: Ready, следующее выполнение 2026-11-20T03:30:00+03:00
Execute   : C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe
Arguments : ... -File "...\backup-objects.ps1" -BackupDir "C:\NexusK2AcceptanceBackups"
            -InstallDir "C:\NexusWorkspace" -Mode Incremental -Consistency Auto
            -Retain 7 -Prune -Force
==> Задача NexusObjectsBackupK2Test удалена
```

Retention проверен на каталоге с двумя старыми полными наборами и новым
полным (`-Mode Full -Retain 1 -Prune -Force`):

```text
before: 2 sets
    удалён старый набор objects-20260101-000001
    удалён старый набор objects-20260101-000002
RC=0; after=1
```

Без `-Prune` старые наборы не удаляются; в неинтерактивном режиме без `-Force`
удаление пропускается. Отдельно проверены защита цепочки и порядок удаления:
в каталоге `old Full + old Incremental + новый Full` при `-Retain 2` удалён
именно старый incremental, а old Full оставлен; старая цепочка удаляется с
конца (сначала incremental, потом их Full), поэтому наборы не остаются без
базы. Если цепочка после последнего Full длиннее `-Retain`, скрипт
предупреждает и не удаляет её. Периодический полный набор задаётся
`-FullEveryDays` (по умолчанию 7 дней).

## 9. Ограничения (приняты и задокументированы)

* VSS: только Windows + NTFS + администратор; снимок crash-consistent, не
  application-consistent. `-Consistency Auto` при ошибке VSS останавливает
  службу; `Vss` — падает, `Pause` — всегда останавливает, `Live` — без
  гарантий.
* Incremental опирается на неизменяемость объектов (размер + mtime); для
  восстановления нужна вся цепочка после `Full`, `restore-objects.ps1`
  проверяет `base`.
* Retention удаляет только по `-Prune` и подтверждению/`-Force`; restore не
  удаляет лишние файлы.
* Сверка с БД проверяет `state='committed'`; `untracked` считается
  предупреждением (quarantine/остатки), `missingOnDisk`/`hashMismatch` —
  ошибка при `-StrictDatabase`, иначе предупреждение.

## 10. Вывод

На маленьком томе воспроизведён полный цикл:
`backup (VSS) → удаление каталога → restore (icacls, SHA-256, storage_objects)
→ скачивание Cloud HTTP 200 с совпавшим SHA-256`. Дополнительно проверены
incremental-цепочка, открытый файл, Task Scheduler и retention. Все итоговые
проверки зелёные, служба `Running`, health 200.

После прогона временная учётная запись `k2admin` переведена в `disabled`
(сессии удалены), чтобы не оставлять на стенде активного администратора с
паролем из теста; том `C:\NexusK2Volume` и наборы
`C:\NexusK2AcceptanceBackups` оставлены как артефакты проверки.
