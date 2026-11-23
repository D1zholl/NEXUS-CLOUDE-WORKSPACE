# Чек-лист развёртывания NEXUS WORKSPACE на Windows

Назначение: пошаговая приёмка чистой установки на Windows 11 / Windows Server
2019+, включая службу, порты, firewall, HTTPS-прокси и перезагрузку хоста.

Документ заполняется **по ходу прогона**: команды копируются в PowerShell,
результат сверяется с «Ожидаемо», затем вписывается фактический результат.
Пустое поле — проверка ещё не выполнена. Все команды выполняются из корня
репозитория (там, где лежат `deployment\` и `tests\`), если не указано иное.

- Стенд: `______________________` (VM / физический)
- Windows: `______________________` (`winver`)
- Дата: `______________________`
- Исполнитель: `______________________`
- Версия NEXUS: `______________________`
- Домен: `______________________`

> ⚠ Раздел 9 перезагружает хост. Выполняйте его только в согласованное окно
> обслуживания. Раздел 10 (update/backup/restore) — отдельное окно.

---

## 1. Предварительные условия

### 1.1. Права администратора и PowerShell

```powershell
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
(New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
  [Security.Principal.WindowsBuiltInRole]::Administrator)
$PSVersionTable.PSVersion.ToString()
```

Ожидаемо:

```text
True
5.1.x (или 7.x)
```

**Фактический результат:** ______________________________________________

### 1.2. Зависимости

```powershell
Get-Service postgresql* | Select-Object Name,Status,StartType
& 'C:\Program Files\PostgreSQL\17\bin\psql.exe' --version
Get-Command caddy.exe -ErrorAction SilentlyContinue | Select-Object Source
```

Ожидаемо: PostgreSQL `Running`, `psql 17.x`; Caddy — если настроен HTTPS.

**Фактический результат:** ______________________________________________

### 1.3. Свободное место и каталоги

```powershell
Get-PSDrive C | Select-Object Used,Free
Test-Path C:\NexusWorkspace
```

Ожидаемо: достаточно места для установки, `data` и будущих объектов;
`C:\NexusWorkspace` может отсутствовать до установки.

**Фактический результат:** ______________________________________________

### 1.4. Длинные пути Windows

```powershell
(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
  -Name LongPathsEnabled -ErrorAction SilentlyContinue).LongPathsEnabled
```

Ожидаемо: `1`. Если `0` — включите `LongPathsEnabled=1` и перезагрузите хост
до установки (`docs/DEPLOYMENT.md` §1.2). Установщик предупреждает об этом;
с `-RequireLongPaths` проверка становится обязательной.

**Фактический результат:** ______________________________________________

---

## 2. Артефакт релиза и контрольная сумма

```powershell
Get-FileHash .\nexus-<версия>-windows-x64.zip -Algorithm SHA256
Get-Content .\nexus-<версия>-windows-x64.zip.sha256
```

Ожидаемо: хеши совпадают; в архиве есть `nexus.exe`, `frontend`, `deployment`,
`docs`.

**Фактический результат:** ______________________________________________

---

## 3. Проверка портов (J3)

### 3.1. Автоматическая проверка

```powershell
powershell -ExecutionPolicy Bypass -File deployment\check-ports.ps1
```

Ожидаемо:

```text
Порт 80: free          (или listening — Caddy уже запущен)
Порт 443: free         (или listening — Caddy уже запущен)
Порт 8080: free
Порты пригодны для запуска.
```

Критично: `excluded` (диапазон Hyper-V/WSL) или `bind-denied`/`AccessDenied`.
`listening` на 80/443 допустим, если это Caddy; на 8080 до установки — нет.

**Фактический результат:** ______________________________________________

### 3.2. Исключённые диапазоны и HTTP.sys

```powershell
netsh interface ipv4 show excludedportrange protocol=tcp
netsh interface ipv6 show excludedportrange protocol=tcp
netsh http show servicestate view=session
Get-Service W3SVC -ErrorAction SilentlyContinue
```

Ожидаемо: 80/443/8080 не входят в исключённые диапазоны; если порт слушает
`System` (PID 4), найти сайт/службу HTTP.sys/IIS и освободить порт.

**Фактический результат:** ______________________________________________

### 3.3. Firewall

```powershell
powershell -ExecutionPolicy Bypass -File deployment\firewall-windows.ps1
Get-NetFirewallRule -DisplayName 'NEXUS WORKSPACE*' |
  Select-Object DisplayName,Enabled,Direction,Action
```

Ожидаемо: созданы `NEXUS WORKSPACE - HTTP 80` и `- HTTPS 443`, `Enabled=True`,
`Inbound`, `Allow`. При локальном прокси backend-порт 8080 не открывается; при
удалённом:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\firewall-windows.ps1 `
  -ExposeBackend -ProxyAddress 10.0.0.10
```

**Фактический результат:** ______________________________________________

---

## 4. Установка (J1)

### 4.1. Запуск установщика

`-DatabaseUrl`/`-Secret` видны в списке процессов и истории PowerShell.
Для одноразового стенда это допустимо; для повторяемых прогонов очищайте
историю (`Clear-History`) или заранее подготовьте `.env` (установщик не
перезаписывает существующий). Пароль администратора передаётся отдельно:

```powershell
$env:NEXUS_DB_URL = 'postgres://nexus:<пароль>@127.0.0.1:5432/nexus'
$env:NEXUS_SECRET = '<64 случайных hex>'

powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1 `
  -InstallDir C:\NexusWorkspace `
  -ServiceName NexusWorkspace `
  -ServiceAccountType Virtual `
  -DatabaseUrl $env:NEXUS_DB_URL `
  -Secret $env:NEXUS_SECRET `
  -ConfigureFirewall `
  -TestServiceControl `
  -SkipBuild
```

Для чистой машины без release-артефакта `-SkipBuild` не указывайте (нужны Rust
и Node) либо распакуйте архив и используйте `-SkipBuild`. Без
`-AdminPassword` установщик запросит пароль администратора интерактивно.

Ожидаемо (ключевые строки):

```text
==> Проверка портов
Порт 8080: free
==> Применение миграций
==> Регистрация службы Windows
    Таймаут остановки SCM: 5000 мс
    Зависимость: postgresql-x64-17
==> Запуск службы
    Служба NexusWorkspace: Running (NT SERVICE\NexusWorkspace)
==> Проверка Start/Stop/Restart-Service
    Restart-Service: ... мс
    Stop-Service:    ... мс
    Start-Service:   ... мс
```

Для `-ServiceAccountType Custom` дополнительно нужен
`-ServicePassword (Read-Host -AsSecureString)` и право «Вход в качестве службы».

**Фактический результат:** ______________________________________________

### 4.2. Учётная запись и права

```powershell
Get-CimInstance Win32_Service -Filter "Name='NexusWorkspace'" |
  Select-Object Name,StartName,StartMode,State
icacls C:\NexusWorkspace\data
icacls C:\NexusWorkspace\logs
```

Ожидаемо: `StartName=NT SERVICE\NexusWorkspace` (или выбранная учётная запись),
`StartMode=Auto`, `State=Running`; ACL содержит Modify для учётной записи
службы на `data`/`logs` и RX на каталог установки.

**Фактический результат:** ______________________________________________

### 4.3. Рабочий каталог и конфигурация

```powershell
sc.exe qc NexusWorkspace
Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\NexusWorkspace' |
  Select-Object Environment,AppDirectory
Test-Path C:\NexusWorkspace\.env
Test-Path C:\NexusWorkspace\nexus.toml
```

Ожидаемо:

- `BINARY_PATH_NAME` = `"...\nexus.exe" serve --service`;
- `START_TYPE` = `AUTO_START`, `SERVICE_START_NAME` = учётная запись службы;
- `Environment` содержит `NEXUS_SERVICE_NAME=NexusWorkspace` и
  `NEXUS_SERVICE_STOP_TIMEOUT_MS=5000`;
- `.env` и `nexus.toml` лежат рядом с `nexus.exe` (SCM стартует из `System32`,
  приложение само переходит в каталог установки).

**Фактический результат:** ______________________________________________

### 4.4. Политика восстановления

```powershell
sc.exe qfailure NexusWorkspace
```

Ожидаемо: три действия `restart` с задержками 5000/15000/60000 мс, сброс
счётчика 86400 с, `failureflag=1`.

**Фактический результат:** ______________________________________________

---

## 5. Служба: Start / Stop / Restart и таймаут (J1/J2)

### 5.1. Базовые команды

```powershell
Get-Service NexusWorkspace
Restart-Service NexusWorkspace
Start-Sleep -Seconds 3
(Get-Service NexusWorkspace).Status
Stop-Service NexusWorkspace -Force
(Get-Service NexusWorkspace).Status
Start-Service NexusWorkspace
```

Ожидаемо: `Running` → `Running`; `Stop-Service` → `Stopped` без ошибки 1053;
`Start-Service` → `Running`. `Stop-Service` не должен висеть дольше
`WaitToKillServiceTimeout` (на стенде 5000 мс).

**Фактический результат:** ______________________________________________

### 5.2. Автоматический тест остановки и C2

```powershell
powershell -ExecutionPolicy Bypass -File tests\service-stop.ps1
```

Ожидаемо:

```text
gracefulStopped: True
gracefulCancelRequested: True
gracefulRecovered: True
hardKillStopped: True
hardKillRecovered: True
orphanRunning: 0
PASS: Stop-Service и восстановление C2 подтверждены.
```

Тест создаёт временную службу на порту 18081 и удаляет её сам.

**Фактический результат:** ______________________________________________

### 5.3. Журнал

```powershell
Get-ChildItem C:\NexusWorkspace\logs |
  Sort-Object LastWriteTime -Descending | Select-Object -First 3 Name,Length
Get-Content (Get-ChildItem C:\NexusWorkspace\logs\nexus.log.* |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName -Tail 20
```

Ожидаемо: файл `nexus.log.<ГГГГ-ММ-ДД>`, запись `NEXUS WORKSPACE запущен`,
`остановка службы: SERVICE_CONTROL_STOP → graceful shutdown`.

**Фактический результат:** ______________________________________________

---

## 6. Проверка приложения

### 6.1. `nexus check`

```powershell
cd C:\NexusWorkspace
.\nexus.exe check
.\nexus.exe check --json
```

Ожидаемо: `Проверка пройдена`; JSON `"ok": true`, PostgreSQL и миграции
видны, статика и каталог логов доступны. Администратор должен быть создан
(установщик делает это, если его ещё нет).

**Фактический результат:** ______________________________________________

### 6.2. Health

```powershell
(Invoke-WebRequest -UseBasicParsing http://127.0.0.1:8080/api/v1/health).Content
```

Ожидаемо:

```json
{"data":{"database":"up","status":"ok","uptimeSeconds":...,"version":"..."},"success":true}
```

**Фактический результат:** ______________________________________________

### 6.3. Вход и хранилище

Откройте `http://127.0.0.1:8080` (или публичный URL), войдите администратором.
В **Система → Хранилища** зарегистрируйте каталог (например,
`D:\NexusStorage`); без тома загрузка вернёт `NO_VOLUME_AVAILABLE`.

Ожидаемо: вход успешен, том `online`, создаётся тестовая папка.

**Фактический результат:** ______________________________________________

---

## 7. HTTPS-прокси (J2, если используется)

### 7.1. Caddy for Windows

```powershell
caddy.exe validate --config C:\NexusWorkspace\Caddyfile
caddy.exe run --config C:\NexusWorkspace\Caddyfile
```

Ожидаемо: конфигурация валидна, сертификат получен/доверен, HTTP → HTTPS.

**Фактический результат:** ______________________________________________

### 7.2. HTTPS, WebSocket, Range

```powershell
curl.exe -I https://nexus.example.com/
curl.exe -I https://nexus.example.com/api/v1/health
```

Ожидаемо: `200`, HSTS, редирект с HTTP; cookie `Secure` при
`NEXUS_COOKIE_SECURE=true`; WebSocket `/api/v1/ws` подключается; скачивание
файла поддерживает `Range`/`206`.

**Фактический результат:** ______________________________________________

---

## 8. Загрузка, скачивание и целостность

1. Загрузите файл 10–100 МиБ через UI.
2. Скачайте его обратно.
3. Сравните SHA-256.

```powershell
Get-FileHash .\download.bin -Algorithm SHA256
Get-FileHash .\source.bin -Algorithm SHA256
```

Ожидаемо: хеши совпадают; в журнале нет `UPLOAD_DATA_MISSING`,
`STORAGE_ERROR`, `PATH_TOO_LONG`.

**Фактический результат:** ______________________________________________

---

## 9. Перезагрузка хоста (J4, P0)

> Раздел перезагружает машину. Убедитесь, что открытые приложения сохранили
> данные. Для автоматической проверки используется отдельная служба `NexusJ4`
> и временный каталог; production-служба `NexusWorkspace`, если она установлена,
> тоже перезагрузится и должна подняться автоматически.

### 9.1. Автоматический тест

```powershell
powershell -ExecutionPolicy Bypass -File tests\reboot-acceptance.ps1
# через ~20 с хост перезагрузится
```

После старта Windows задача `NexusJ4RebootVerify` (SYSTEM, At startup)
выполнит проверку и запишет:

```text
C:\ProgramData\NexusJ4\reboot-result.json
C:\ProgramData\NexusJ4\reboot-result.md
```

Ожидаемо: `ok: true`, `failures: []`; служба `Running`, `StartType=Automatic`;
running-задача восстановлена (`recoveryInLog=true`, `orphanRunning=0`);
публичная ссылка и resume-загрузка совпали по SHA-256.

**Фактический результат:** ______________________________________________

### 9.2. Проверка production-службы после reboot

```powershell
(Get-CimInstance Win32_OperatingSystem).LastBootUpTime
Get-Service NexusWorkspace
Get-Process nexus | Select-Object Id,StartTime
(Invoke-WebRequest -UseBasicParsing http://127.0.0.1:8080/api/v1/health).Content
```

Ожидаемо: служба `Running`, процесс стартовал после boot, health `ok`; тома
`online`; осиротевших `running` нет. Кратковременный прогрев PostgreSQL
(несколько секунд) допустим; повторяющийся `database: down` — дефект.

**Фактический результат:** ______________________________________________

---

## 10. Обновление, backup и restore (J3, отдельное окно)

### 10.1. Backup

```powershell
powershell -ExecutionPolicy Bypass -File deployment\backup.ps1 `
  -InstallDir C:\NexusWorkspace -BackupDir D:\NexusBackups
```

Ожидаемо: служба остановлена, создан архив БД и `data`, служба запущена,
`nexus check` зелёный.

**Фактический результат:** ______________________________________________

### 10.2. Update и откат

```powershell
powershell -ExecutionPolicy Bypass -File deployment\update-windows.ps1 `
  -InstallDir C:\NexusWorkspace -BackupDir D:\NexusBackups
```

Ожидаемо: backup → остановка → замена `nexus.exe` → миграции → запуск;
при ошибке миграции возвращается `nexus.previous.exe` и служба поднимается.

**Фактический результат:** ______________________________________________

### 10.3. Restore

```powershell
# BackupPath — каталог конкретной копии, созданный backup.ps1.
powershell -ExecutionPolicy Bypass -File deployment\restore.ps1 `
  -InstallDir C:\NexusWorkspace `
  -BackupPath D:\NexusBackups\20260101-120000
```

Ожидаемо: восстановление в тестовую БД/каталог, вход и скачивание старого
объекта работают, SHA совпадает.

**Фактический результат:** ______________________________________________

---

## 11. Итог приёмки

| Критерий | Результат |
|---|---|
| Чистая установка, миграции, администратор | `______` |
| Служба `Automatic`, Start/Stop/Restart без 1053 | `______` |
| Порты и firewall проверены | `______` |
| HTTPS/WebSocket/Range (если прокси) | `______` |
| Загрузка/скачивание и SHA | `______` |
| Перезагрузка: автозапуск, C2, целостность | `______` |
| `nexus check --json` → `ok=true` | `______` |
| Backup/restore/update (если выполнялись) | `______` |

Известные отклонения и причина:

```text
______________________________________________________________________
______________________________________________________________________
```

Подпись / «проверено»: `______________________` Дата: `______________`

---

## Приложение A. Очистка тестовых артефактов

```powershell
# Только для тестовой службы, не production.
Stop-Service NexusJ4 -Force -ErrorAction SilentlyContinue
sc.exe delete NexusJ4
Unregister-ScheduledTask -TaskName NexusJ4RebootVerify -Confirm:$false -ErrorAction SilentlyContinue
Remove-Item C:\ProgramData\NexusJ4 -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item C:\Users\$env:USERNAME\AppData\Local\Temp\nexus-* -Recurse -Force -ErrorAction SilentlyContinue
```

Проверить, что не осталось тестовых firewall-правил:

```powershell
Get-NetFirewallRule -DisplayName 'NEXUS TEST*' -ErrorAction SilentlyContinue
```

## Приложение B. Типовые ошибки

| Симптом | Причина | Действие |
|---|---|---|
| `Start-Service`: 1053 | binPath без `serve --service` или старый бинарник | `sc.exe qc`, переустановить |
| `bind`/`AccessDenied` на 80/443/8080 | HTTP.sys/IIS, Hyper-V/WSL excluded range | `check-ports.ps1`, освободить диапазон/порт |
| Служба стартует, затем `database: down` | PostgreSQL ещё прогревается или не запущен | проверить зависимость `sc.exe qc`, `pg_isready` |
| `NO_VOLUME_AVAILABLE` | нет тома `auto` | зарегистрировать каталог в «Хранилищах» |
| 1053 после обновления | служба не остановлена перед заменой exe | `update-windows.ps1` ждёт `Stopped` до 40 с |
| `UPLOAD_DATA_MISSING` | Defender удалил staging | повторить, проверить карантин, см. DEPLOYMENT §4.5 |
