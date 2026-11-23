# Установка NEXUS WORKSPACE из архива (Windows 11 x64)

Архив: `nexus-1.0.0-windows-x64.zip`  
Состав: `nexus.exe`, `frontend\`, `deployment\` (включая шаблон
`nexus.production.toml`), `docs\INSTALL-WINDOWS.md`, `README-RELEASE.txt`,
`VERSION`, `SHA256SUMS`. Файл `nexus.toml` установщик создаёт из
`deployment\nexus.production.toml` в каталоге установки.

## Коротко: установка, запуск и вход

1. Установите PostgreSQL 17 и создайте отдельную роль и базу `nexus`:

    ```sql
    CREATE ROLE nexus LOGIN PASSWORD '<пароль-БД>';
    CREATE DATABASE nexus OWNER nexus;
    ```

2. Распакуйте release ZIP. Откройте PowerShell **от имени администратора** в
    распакованном каталоге и выполните команду, подставив URL-кодированный
    пароль PostgreSQL:

    ```powershell
    .\deployment\install-windows.ps1 -SkipBuild `
         -DatabaseUrl "postgres://nexus:<пароль-БД>@127.0.0.1:5432/nexus"
    ```

    Установщик применит миграции, создаст службу `NexusWorkspace` и запустит её.
    Если администраторов ещё нет, он создаст логин `admin` и попросит задать
    пароль; ввод не отображается. Пароля по умолчанию нет.

3. Проверьте службу и базу:

    ```powershell
    Get-Service NexusWorkspace
    & "C:\NexusWorkspace\nexus.exe" check --json
    ```

    Если служба остановлена: `Start-Service NexusWorkspace`.

4. На серверном ПК откройте `http://127.0.0.1:8080` и войдите как `admin` с
    паролем, который задали при установке. Чтобы входить с других ПК, настройте
    HTTPS через Caddy по разделу 4; не открывайте порт 8080 в интернет.

**Если пароль существующей учётной записи неизвестен:** его нельзя извлечь
из БД. Из PowerShell на сервере создайте второго superadmin с новым логином:

```powershell
Set-Location C:\NexusWorkspace
.\nexus.exe create-admin --username newadmin
```

CLI запросит пароль скрытым вводом; команда создаёт новую учётную запись и не
сбрасывает пароль существующей.

## Запуск из исходников: first-run.bat, start.bat, stop.bat, users.bat

Если вы развернули исходники (каталоги `backend`, `frontend`, `deployment`,
`scripts`), используйте BAT-файлы в корне:

1. `first-run.bat` — проверяет Rust 1.85+, Node.js 20+, PostgreSQL 14+,
   создаёт `.env` со случайным `NEXUS_SECRET`, собирает release, применяет
   миграции, создаёт администратора и предлагает запуск.
2. `start.bat` — обычный запуск. Флаги: `--service` (только служба),
   `--console` (только консоль), `--no-browser`. Ищет `nexus.exe` в
   `%NEXUS_HOME%`, `C:\NexusWorkspace`, `backend\target\release` и рядом с BAT.
3. `users.bat` — меню управления пользователями (создание, пароль, блокировка,
   разблокировка, удаление, сессии, роли). Пароли вводит только `nexus.exe` скрытым
   вводом.
4. `stop.bat` — полная остановка: консольные `nexus.exe`, служба
   `NexusWorkspace`, Caddy и PostgreSQL из `DATABASE_URL`. Флаги: `--status`,
   `--dry-run`, `--yes`, `--keep-db`, `--keep-proxy`. Для остановки служб
   запускайте от имени администратора.

BAT-файлы сохранены в UTF-8 без BOM: BOM в первой строке ломает `@echo off`
в cmd.exe. Внутри выполняется `chcp 65001`. Журналы — в `logs\`; пароли и
`NEXUS_SECRET` туда не пишутся. Коды возврата: `0` успех, `1` нет зависимостей,
`2` конфигурация/БД, `3` сборка, `4` запуск.
`stop.bat --status` возвращает `1`, если компоненты ещё работают; `stop.bat`
возвращает `2`, если для остановки служб нужны права администратора.

## 1. Требования

| Компонент | Версия / примечание |
|---|---|
| Windows | 11 Pro/Enterprise x64 или Server 2022+ |
| PostgreSQL | 17.x, служба `postgresql-x64-17`, БД и роль `nexus` |
| Порты | 8080 (backend, только localhost), 80/443 (Caddy) |
| Права | локальный администратор |
| Свободно на диске | ≥ 2 ГиБ + место под данные |

Проверьте целостность архива:

```powershell
Get-FileHash .\nexus-1.0.0-windows-x64.zip -Algorithm SHA256
Get-Content .\nexus-1.0.0-windows-x64.zip.sha256
```

Хэши должны совпасть.

## 2. Распаковка

```powershell
Expand-Archive .\nexus-1.0.0-windows-x64.zip -DestinationPath C:\
cd C:\nexus-1.0.0-windows-x64
```

`SHA256SUMS` содержит хэши всех файлов (`Get-FileHash`):

```powershell
Get-Content .\SHA256SUMS | ForEach-Object {
    $hash, $path = $_ -split '  ', 2
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $hash) { Write-Host "FAIL $path" -ForegroundColor Red }
}
```

## 3. Установка

```powershell
powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1 `
    -SkipBuild `
    -InstallDir C:\NexusWorkspace `
    -DatabaseUrl "postgres://nexus:ПАРОЛЬ@127.0.0.1:5432/nexus" `
    -AdminPassword "ПарольАдминистратора" `
    -Domain localhost `
    -TestServiceControl
```

`-Domain` задаёт `NEXUS_PUBLIC_URL=https://<домен>` и `NEXUS_COOKIE_SECURE=true`.
Без него установщик создаёт `.env` с `http://localhost:8080` и
`NEXUS_COOKIE_SECURE=false` — поправьте вручную после настройки HTTPS.

Скрипт:

1. проверяет порты (8080 свободен; 80/443 вне исключённых диапазонов Hyper-V/WSL);
2. копирует `nexus.exe`, `frontend\`, конфиги в `C:\NexusWorkspace`;
3. создаёт `.env` (секрет, пути, `NEXUS_TRUST_PROXY=true`);
4. применяет миграции и выполняет `nexus check`;
5. регистрирует службу `NexusWorkspace` (`nexus.exe serve --service`, встроенный SCM dispatcher),
   автозапуск, recovery, зависимость от PostgreSQL;
6. запускает службу и проверяет `Start/Stop/Restart-Service`.

Пароль администратора передаётся через переменную окружения внутри скрипта,
в командную строку и журналы не попадает.

Проверка:

```powershell
Get-Service NexusWorkspace
cd C:\NexusWorkspace
.\nexus.exe check --json
Invoke-WebRequest http://127.0.0.1:8080/api/v1/health -UseBasicParsing
```

Ожидается `Status=Running`, `"ok": true`, HTTP 200.

## 4. HTTPS через Caddy for Windows

Публичный домен в этом стенде недоступен. Используется внутренний CA Caddy.

```powershell
# 1. Caddy и NSSM
New-Item -ItemType Directory -Force C:\Caddy,C:\Caddy\logs,C:\Caddy\data,C:\Caddy\config
# caddy.exe: https://caddyserver.com/download (windows/amd64)
Copy-Item .\deployment\Caddyfile.windows C:\Caddy\Caddyfile
# отредактируйте localhost/192.168.1.10 на свои адреса

# 2. Служба
C:\Caddy\nssm.exe install Caddy C:\Caddy\caddy.exe
C:\Caddy\nssm.exe set Caddy AppParameters "run --config C:\Caddy\Caddyfile --adapter caddyfile"
C:\Caddy\nssm.exe set Caddy AppDirectory C:\Caddy
C:\Caddy\nssm.exe set Caddy AppEnvironmentExtra "XDG_DATA_HOME=C:\Caddy\data" "XDG_CONFIG_HOME=C:\Caddy\config"
C:\Caddy\nssm.exe set Caddy Start SERVICE_AUTO_START
C:\Caddy\nssm.exe start Caddy

# 3. Доверие внутреннему CA (LocalMachine\Root)
certutil -addstore -f Root C:\Caddy\data\pki\authorities\local\root.crt

# 4. Firewall
powershell -ExecutionPolicy Bypass -File deployment\firewall-windows.ps1
```

Проверка: `https://localhost/api/v1/health` → 200, HTTP → 301 на HTTPS,
заголовок `Strict-Transport-Security`, WebSocket `/api/v1/ws` работает.

После включения HTTPS в `C:\NexusWorkspace\.env` должно быть:

```dotenv
NEXUS_PUBLIC_URL=https://localhost
NEXUS_COOKIE_SECURE=true
NEXUS_TRUST_PROXY=true
```

Затем `Restart-Service NexusWorkspace`.

`NEXUS_TRUST_PROXY=true` допустим **только** при доступе через Caddy. При прямом
доступе к `:8080` (без прокси) установите `NEXUS_TRUST_PROXY=false` и перезапустите
службу, иначе клиент сможет подставить `X-Forwarded-For` в аудит.

## 5. Обслуживание

Все скрипты — из каталога `deployment\`, запуск от администратора.

### Резервная копия (служба останавливается)

```powershell
.\deployment\backup.ps1 -BackupDir C:\NexusBackups
```

Создаёт `C:\NexusBackups\<stamp>\database.dump` (custom format), `data\`, `.env`,
`nexus.toml`, `manifest.json`, `SHA256SUMS`. Пароль PostgreSQL передаётся через
временный `.pgpass`-файл, не через командную строку.

### Восстановление в новую БД

```powershell
.\deployment\restore.ps1 -BackupPath C:\NexusBackups\<stamp> `
    -TargetDatabase nexus_restore -SwitchInstall -TestUsername admin
```

`-SwitchInstall` переключает `.env` на новую БД. Проверяется `nexus check`,
health=200 и вход. Для восстановления поверх активной БД добавьте `-DropExisting`.

### Обновление

```powershell
.\deployment\update-windows.ps1 -SkipBuild
```

Порядок: остановка службы → backup → замена `nexus.exe` (старый —
`nexus.previous.exe`) → миграции → проверка → запуск. При ошибке миграции база
восстанавливается из дампа, бинарник откатывается, служба поднимается.

## 6. Удаление

```powershell
Stop-Service NexusWorkspace
sc.exe delete NexusWorkspace
Remove-Item C:\NexusWorkspace -Recurse -Force   # данные и конфигурация
C:\Caddy\nssm.exe remove Caddy confirm
```

База данных удаляется отдельно:

```powershell
& "C:\Program Files\PostgreSQL\17\bin\psql.exe" -U nexus -d postgres -c "drop database nexus;"
```

## 7. Типовые ошибки

| Симптом | Причина / решение |
|---|---|
| `Start-Service` → 1053 | запущен не тот binPath; должен быть `"...\nexus.exe" serve --service` |
| `nexus check` → БД недоступна | служба PostgreSQL не запущена, неверный `DATABASE_URL` |
| 502 от Caddy | служба `NexusWorkspace` остановлена |
| Браузер не доверяет сертификату | не установлен корневой CA Caddy (`certutil -addstore -f Root`) |
| 413/обрыв на большом файле | проверьте `request_body max_size` в Caddyfile |
| В аудите `127.0.0.1` | запрос идёт напрямую в `:8080`, а не через Caddy |
