@echo off
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "ENVFILE=%ROOT%\.env"
set "LOG_DIR=%ROOT%\logs"
if not exist "%LOG_DIR%" mkdir "%LOG_DIR%" >nul 2>&1

for /f "delims=" %%I in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%I"
if "%STAMP%"=="" set "STAMP=%RANDOM%"
set "LOG=%LOG_DIR%\start-%STAMP%.log"

set "MODE="
set "NO_BROWSER=0"
set "STATUS_ONLY=0"
set "PREFER_LOCAL=0"
set "NO_PAUSE=0"
set "FORCE_CONSOLE=0"
set "CODE=0"
set "PUBLIC_URL="
set "PORT=8080"
set "WORKDIR=%ROOT%"
set "EXE="
set "ENSURE_DB=1"
set "DB_URL="
set "DB_HOST="
set "DB_PORT=5432"
set "DB_NAME="
set "DB_LOCAL=0"
set "DB_SERVICE="
set "DB_LISTENING=0"
set "DB_STATE=не настроен"
set "SVC_STATE=MISSING"
set "LOCAL_EXE="
set "INSTALLED_EXE="
set "LOCAL_IS_NEWER=0"

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
if /i "%~1"=="/?" goto usage
if /i "%~1"=="--service" (
    set "MODE=service"
    shift
    goto parse
)
if /i "%~1"=="--console" (
    set "MODE=console"
    shift
    goto parse
)
if /i "%~1"=="--local" (
    set "PREFER_LOCAL=1"
    shift
    goto parse
)
if /i "%~1"=="--status" (
    set "STATUS_ONLY=1"
    shift
    goto parse
)
if /i "%~1"=="--check" (
    set "STATUS_ONLY=1"
    shift
    goto parse
)
if /i "%~1"=="--no-browser" (
    set "NO_BROWSER=1"
    shift
    goto parse
)
if /i "%~1"=="--force" (
    set "FORCE_CONSOLE=1"
    shift
    goto parse
)
if /i "%~1"=="--no-pause" (
    set "NO_PAUSE=1"
    shift
    goto parse
)
if /i "%~1"=="--no-db" (
    set "ENSURE_DB=0"
    shift
    goto parse
)
echo Неизвестный параметр: %~1
goto usage

:usage
echo.
echo NEXUS WORKSPACE - обычный запуск
echo.
echo Использование:
echo   start.bat                 запустить самую свежую сборку и открыть браузер
echo   start.bat --service       только служба Windows
echo   start.bat --console       запустить nexus.exe в этом окне
echo   start.bat --local         для консоли использовать сборку из этого каталога
echo   start.bat --status        показать состояние без запуска
echo   start.bat --no-browser    не открывать браузер
echo   start.bat --force         разрешить консоль при запущенной службе
echo   start.bat --no-db         не запускать PostgreSQL автоматически
echo.
echo Если локальная сборка новее установленной, запускается она, а устаревшая
echo служба останавливается. Если локальной сборки нет, используется служба.
echo Если служба NexusWorkspace не зарегистрирована, запускается консольный режим.
echo Локальный PostgreSQL из DATABASE_URL запускается автоматически, если остановлен.
echo Код возврата: 0 - успех, 1 - нет exe, 2 - конфигурация/порт, 3 - сборка, 4 - запуск.
goto finish

:parsed
call :banner
echo Журнал: %LOG%
echo.

if "%STATUS_ONLY%"=="1" goto status

call :read_env
call :service_state
call :detect_builds

if /i "%MODE%"=="console" goto console
if /i "%MODE%"=="service" goto service
if "%PREFER_LOCAL%"=="1" goto prefer_local_console

if /i "%SVC_STATE%"=="MISSING" (
    echo Служба NexusWorkspace не найдена - запускаю консольный режим.
    goto console
)

if "%LOCAL_IS_NEWER%"=="1" (
    echo Локальная сборка новее установленной в C:\NexusWorkspace.
    echo Чтобы обновить службу этой сборкой: deployment\update-windows.ps1
    call :stop_stale_service
    if errorlevel 1 (
        echo.
        echo ОШИБКА: служба NexusWorkspace работает со старой сборкой.
        echo Запустите start.bat от имени администратора или обновите службу:
        echo   deployment\update-windows.ps1
        set "CODE=4"
        goto finish
    )
    set "PREFER_LOCAL=1"
    goto console
)
goto service

:prefer_local_console
if /i "%SVC_STATE%"=="Running" if "%FORCE_CONSOLE%"=="0" (
    echo Локальная сборка выбрана, но служба NexusWorkspace уже занимает порт %PORT%.
    echo Остановите службу или запустите start.bat --force.
    set "CODE=2"
    goto finish
)
goto console

:status
call :service_state
call :detect_builds
if /i not "%SVC_STATE%"=="MISSING" if not "%LOCAL_IS_NEWER%"=="1" if exist "C:\NexusWorkspace\.env" set "ENVFILE=C:\NexusWorkspace\.env"
call :read_env
if "%LOCAL_IS_NEWER%"=="1" set "PREFER_LOCAL=1"
echo.
echo --- Конфигурация ---
echo Файл .env: %ENVFILE%
if defined PUBLIC_URL echo Публичный URL: %PUBLIC_URL%
if not defined PUBLIC_URL echo Публичный URL: http://127.0.0.1:%PORT%
echo Порт backend: %PORT%
echo Служба NexusWorkspace: %SVC_STATE%
call :db_status
echo PostgreSQL:            %DB_STATE%
echo.
echo --- Сборки ---
if defined LOCAL_EXE echo Локальная:      %LOCAL_EXE%
if not defined LOCAL_EXE echo Локальная:      не найдена
if defined INSTALLED_EXE echo Установленная: %INSTALLED_EXE%
if not defined INSTALLED_EXE echo Установленная: не найдена
if "%LOCAL_IS_NEWER%"=="1" echo Актуальная:     локальная сборка
if not "%LOCAL_IS_NEWER%"=="1" echo Актуальная:     установленная сборка
echo.
echo --- Health ---
set "HEALTH_STATE=down"
powershell -NoProfile -Command "try { $r=Invoke-WebRequest 'http://127.0.0.1:%PORT%/api/v1/health' -UseBasicParsing -TimeoutSec 5; Write-Output ('HTTP ' + $r.StatusCode); Write-Output $r.Content; try { $j=$r.Content | ConvertFrom-Json; Write-Output ('STATE=' + $j.data.status) } catch { Write-Output 'STATE=degraded' } } catch { Write-Output ('НЕДОСТУПНО: ' + $_.Exception.Message); Write-Output 'STATE=down' }" > "%TEMP%\nexus-health.txt" 2>&1
type "%TEMP%\nexus-health.txt"
for /f "tokens=2 delims==" %%H in ('findstr /B "STATE=" "%TEMP%\nexus-health.txt"') do set "HEALTH_STATE=%%H"
echo.
echo --- nexus check ---
call :find_exe
if not defined EXE (
    echo nexus.exe не найден.
    set "CODE=1"
    goto finish
)
call :set_local_env
pushd "%WORKDIR%"
"%EXE%" check --json
set "CODE=%ERRORLEVEL%"
popd
if /i not "%HEALTH_STATE%"=="ok" set "CODE=1"
goto finish

:service
echo Режим: служба Windows
if "%LOCAL_IS_NEWER%"=="1" (
    echo ВНИМАНИЕ: локальная сборка новее установленной. Служба запускает старую версию.
    echo Обновите её: deployment\update-windows.ps1
)
if exist "C:\NexusWorkspace\.env" set "ENVFILE=C:\NexusWorkspace\.env"
call :read_env
call :ensure_database
if errorlevel 1 (
    echo ОШИБКА: PostgreSQL недоступен. Смотрите сообщения выше.
    set "CODE=2"
    goto finish
)
call :service_state
if /i "%SVC_STATE%"=="MISSING" (
    echo ОШИБКА: служба NexusWorkspace не зарегистрирована.
    echo Запустите first-run.bat или start.bat --console.
    set "CODE=4"
    goto finish
)
if /i not "%SVC_STATE%"=="Running" (
    echo Запускаю службу...
    powershell -NoProfile -Command "Start-Service NexusWorkspace"
    if errorlevel 1 (
        echo Не удалось запустить службу. Запустите start.bat от администратора.
        set "CODE=4"
        goto finish
    )
)
echo Ожидаю готовности HTTP...
call :wait_health
if errorlevel 1 (
    echo Сервер не ответил за 30 секунд.
    echo Проверьте журнал: C:\NexusWorkspace\logs\nexus.log.*
    set "CODE=4"
    goto finish
)
echo Сервер готов.
if "%NO_BROWSER%"=="1" goto service_done
call :open_browser
:service_done
call :service_state
echo Служба: %SVC_STATE%
set "CODE=0"
goto finish

:console
echo Режим: консольный запуск
call :find_exe
if not defined EXE (
    echo ОШИБКА: nexus.exe не найден.
    set "CODE=1"
    goto finish
)
call :set_local_env
echo Использую: %EXE%
echo Рабочий каталог: %WORKDIR%
call :service_state
if /i "%SVC_STATE%"=="Running" if "%FORCE_CONSOLE%"=="0" (
    echo.
    echo Служба NexusWorkspace уже запущена и держит порт %PORT%.
    echo Остановите её перед консольным запуском:
    echo   Stop-Service NexusWorkspace
    echo Или запустите с --force, если используете другой порт.
    set "CODE=2"
    goto finish
)
if /i "%SVC_STATE%"=="Running" echo ВНИМАНИЕ: служба запущена, но выбран режим --force.
powershell -NoProfile -Command "$c=Get-NetTCPConnection -LocalPort %PORT% -State Listen -ErrorAction SilentlyContinue; if ($c) { $c | Select-Object -First 1 LocalAddress,LocalPort,OwningProcess | Format-Table -AutoSize; exit 1 } else { exit 0 }"
if errorlevel 1 (
    echo Порт %PORT% уже занят. Освободите его.
    set "CODE=2"
    goto finish
)
call :ensure_database
if errorlevel 1 (
    echo ОШИБКА: PostgreSQL недоступен. Смотрите сообщения выше.
    set "CODE=2"
    goto finish
)
pushd "%WORKDIR%"
echo Применяю миграции...
"%EXE%" migrate > "%TEMP%\nexus-start-migrate.log" 2>&1
set "MIGRATE_CODE=%ERRORLEVEL%"
type "%TEMP%\nexus-start-migrate.log"
type "%TEMP%\nexus-start-migrate.log" >> "%LOG%"
if not "%MIGRATE_CODE%"=="0" (
    popd
    echo ОШИБКА миграций. Проверьте DATABASE_URL и PostgreSQL.
    set "CODE=2"
    goto finish
)
echo.
echo Запускаю NEXUS. Для остановки нажмите Ctrl+C.
if not defined PUBLIC_URL set "PUBLIC_URL=http://127.0.0.1:%PORT%"
echo Адрес: %PUBLIC_URL%
if "%NO_BROWSER%"=="1" goto console_serve
start "" powershell -NoProfile -WindowStyle Hidden -Command "for ($i=0; $i -lt 60; $i++) { try { $r=Invoke-WebRequest 'http://127.0.0.1:%PORT%/api/v1/health' -UseBasicParsing -TimeoutSec 2; if ($r.StatusCode -eq 200) { Start-Process '%PUBLIC_URL%'; break } } catch {}; Start-Sleep -Seconds 1 }"
:console_serve
"%EXE%" serve
set "CODE=%ERRORLEVEL%"
popd
goto finish

:wait_health
powershell -NoProfile -Command "for ($i=0; $i -lt 30; $i++) { try { $r=Invoke-WebRequest 'http://127.0.0.1:%PORT%/api/v1/health' -UseBasicParsing -TimeoutSec 2; $j=$r.Content | ConvertFrom-Json; if ($r.StatusCode -eq 200 -and $j.data.status -eq 'ok') { exit 0 } } catch {}; Start-Sleep -Seconds 1 }; exit 1"
exit /b %ERRORLEVEL%

:open_browser
if defined PUBLIC_URL (
    start "" "%PUBLIC_URL%"
) else (
    start "" "http://127.0.0.1:%PORT%"
)
exit /b 0

:service_state
set "SVC_STATE=MISSING"
for /f "delims=" %%S in ('powershell -NoProfile -Command "try { (Get-Service NexusWorkspace -ErrorAction Stop).Status } catch { 'MISSING' }"') do set "SVC_STATE=%%S"
exit /b 0

:set_local_env
if /i not "%WORKDIR%"=="%ROOT%" exit /b 0
if exist "%ROOT%\frontend\dist\index.html" set "NEXUS_STATIC_DIR=%ROOT%\frontend\dist"
if exist "%ROOT%\logs" set "NEXUS_LOG_DIR=%ROOT%\logs"
exit /b 0

:read_env
if not exist "%ENVFILE%" exit /b 0
for /f "usebackq tokens=1,* delims==" %%A in ("%ENVFILE%") do (
    if /i "%%A"=="NEXUS_PUBLIC_URL" set "PUBLIC_URL=%%B"
    if /i "%%A"=="NEXUS_PORT" set "PORT=%%B"
    if /i "%%A"=="DATABASE_URL" set "DB_URL=%%B"
)
if defined PUBLIC_URL set "PUBLIC_URL=%PUBLIC_URL:"=%"
if defined PORT set "PORT=%PORT:"=%"
if defined DB_URL set "DB_URL=%DB_URL:"=%"
exit /b 0

:detect_builds
set "LOCAL_EXE="
set "INSTALLED_EXE="
set "LOCAL_IS_NEWER=0"
if exist "%ROOT%\backend\target\release\nexus.exe" set "LOCAL_EXE=%ROOT%\backend\target\release\nexus.exe"
if not defined LOCAL_EXE if exist "%ROOT%\nexus.exe" set "LOCAL_EXE=%ROOT%\nexus.exe"
if exist "C:\NexusWorkspace\nexus.exe" set "INSTALLED_EXE=C:\NexusWorkspace\nexus.exe"
if not defined LOCAL_EXE exit /b 0
if not defined INSTALLED_EXE (
    set "LOCAL_IS_NEWER=1"
    exit /b 0
)
set "NEXUS_LOCAL_EXE=%LOCAL_EXE%"
set "NEXUS_INSTALLED_EXE=%INSTALLED_EXE%"
set "NEXUS_LOCAL_STATIC=%ROOT%\frontend\dist\index.html"
set "NEXUS_INSTALLED_STATIC=C:\NexusWorkspace\frontend\index.html"
for /f "delims=" %%R in ('powershell -NoProfile -Command "$newer='0'; $local=Get-Item -LiteralPath $env:NEXUS_LOCAL_EXE -ErrorAction SilentlyContinue; $installed=Get-Item -LiteralPath $env:NEXUS_INSTALLED_EXE -ErrorAction SilentlyContinue; if ($local -and -not $installed) { $newer='1' } elseif ($local -and $installed -and $local.LastWriteTimeUtc -gt $installed.LastWriteTimeUtc) { $newer='1' } elseif ($local -and (Test-Path -LiteralPath $env:NEXUS_LOCAL_STATIC)) { $localStatic=Get-Item -LiteralPath $env:NEXUS_LOCAL_STATIC; $installedStatic=Get-Item -LiteralPath $env:NEXUS_INSTALLED_STATIC -ErrorAction SilentlyContinue; if (-not $installedStatic -or $localStatic.LastWriteTimeUtc -gt $installedStatic.LastWriteTimeUtc) { $newer='1' } }; $newer"') do set "LOCAL_IS_NEWER=%%R"
exit /b 0

:stop_stale_service
if /i not "%SVC_STATE%"=="Running" exit /b 0
echo Останавливаю службу NexusWorkspace со старой сборкой...
powershell -NoProfile -Command "try { Stop-Service NexusWorkspace -Force -ErrorAction Stop; exit 0 } catch { exit 1 }"
if errorlevel 1 exit /b 1
exit /b 0

:find_exe
set "EXE="
set "WORKDIR=%ROOT%"
if "%PREFER_LOCAL%"=="1" goto find_local
if "%LOCAL_IS_NEWER%"=="1" goto find_local
call :service_state
if /i not "%SVC_STATE%"=="MISSING" (
    if exist "C:\NexusWorkspace\nexus.exe" (
        set "EXE=C:\NexusWorkspace\nexus.exe"
        set "WORKDIR=C:\NexusWorkspace"
        exit /b 0
    )
)
:find_local
if exist "%ROOT%\backend\target\release\nexus.exe" (
    set "EXE=%ROOT%\backend\target\release\nexus.exe"
    set "WORKDIR=%ROOT%"
    exit /b 0
)
if exist "%ROOT%\nexus.exe" (
    set "EXE=%ROOT%\nexus.exe"
    set "WORKDIR=%ROOT%"
    exit /b 0
)
if defined NEXUS_HOME if exist "%NEXUS_HOME%\nexus.exe" (
    set "EXE=%NEXUS_HOME%\nexus.exe"
    set "WORKDIR=%NEXUS_HOME%"
    exit /b 0
)
if exist "C:\NexusWorkspace\nexus.exe" (
    set "EXE=C:\NexusWorkspace\nexus.exe"
    set "WORKDIR=C:\NexusWorkspace"
    exit /b 0
)
exit /b 1

:resolve_db
set "DB_HOST="
set "DB_PORT=5432"
set "DB_NAME="
set "DB_LOCAL=0"
if not defined DB_URL exit /b 0
set "NEXUS_START_DB_URL=%DB_URL%"
for /f "tokens=1,2,3 delims=|" %%A in ('powershell -NoProfile -Command "$u=$null; try { $u=New-Object System.Uri($env:NEXUS_START_DB_URL) } catch { 'INVALID'; exit 0 }; if (-not $u) { 'INVALID'; exit 0 }; $port=5432; if ($u.Port -gt 0) { $port=$u.Port }; Write-Output ($u.Host + '|' + $port + '|' + $u.AbsolutePath.Trim('/'))"') do (
    set "DB_HOST=%%A"
    set "DB_PORT=%%B"
    set "DB_NAME=%%C"
)
if /i "%DB_HOST%"=="INVALID" set "DB_PORT=5432"
if /i "%DB_HOST%"=="127.0.0.1" set "DB_LOCAL=1"
if /i "%DB_HOST%"=="localhost" set "DB_LOCAL=1"
if /i "%DB_HOST%"=="::1" set "DB_LOCAL=1"
if /i "%DB_HOST%"=="[::1]" set "DB_LOCAL=1"
exit /b 0

:db_listening
set "DB_LISTENING=0"
for /f "delims=" %%L in ('powershell -NoProfile -Command "$c=Get-NetTCPConnection -LocalPort %DB_PORT% -State Listen -ErrorAction SilentlyContinue; if ($c) { '1' } else { '0' }"') do set "DB_LISTENING=%%L"
exit /b 0

:db_resolve_service
rem Ищет службу PostgreSQL, чей postgresql.conf слушает нужный порт.
set "DB_SERVICE=NOT_FOUND"
if not defined DB_PORT set "DB_PORT=5432"
for /f "delims=" %%S in ('powershell -NoProfile -Command "$port=%DB_PORT%; $all=@(Get-CimInstance Win32_Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'postgresql*' }); $match=''; foreach ($s in $all) { $m=[regex]::Match([string]$s.PathName, '-D\s+\x22([^\x22]+)\x22'); if (-not $m.Success) { continue }; $conf=Join-Path $m.Groups[1].Value 'postgresql.conf'; if (-not (Test-Path -LiteralPath $conf)) { continue }; $line=Select-String -LiteralPath $conf -Pattern '^\s*port\s*=\s*(\d+)' | Select-Object -First 1; if ($line -and [int]$line.Matches[0].Groups[1].Value -eq $port) { $match=$s.Name; break } }; if (-not $match -and $all.Count -eq 1) { $match=$all[0].Name }; if ($match) { $match } else { 'NOT_FOUND' }"') do set "DB_SERVICE=%%S"
exit /b 0

:db_service_state
set "SVC_STATE=MISSING"
for /f "delims=" %%S in ('powershell -NoProfile -Command "try { (Get-Service -Name '%~1' -ErrorAction Stop).Status } catch { 'MISSING' }"') do set "SVC_STATE=%%S"
exit /b 0

:ensure_database
if "%ENSURE_DB%"=="0" (
    call :note "PostgreSQL: автозапуск отключён параметром --no-db."
    exit /b 0
)
call :resolve_db
if not defined DB_URL (
    call :note "PostgreSQL: DATABASE_URL не задан, автозапуск пропущен."
    exit /b 0
)
if /i "%DB_HOST%"=="INVALID" (
    call :note "PostgreSQL: не удалось разобрать DATABASE_URL."
    exit /b 1
)
if not "%DB_LOCAL%"=="1" (
    call :note "PostgreSQL: внешняя база %DB_HOST%:%DB_PORT%, автозапуск не требуется."
    exit /b 0
)
call :db_listening
if "%DB_LISTENING%"=="1" (
    call :note "PostgreSQL: порт %DB_PORT% слушается."
    exit /b 0
)
call :db_resolve_service
if /i "%DB_SERVICE%"=="NOT_FOUND" (
    call :note "PostgreSQL: порт %DB_PORT% не слушается и служба не найдена."
    call :note "Запустите PostgreSQL вручную или проверьте DATABASE_URL."
    exit /b 1
)
call :db_service_state "%DB_SERVICE%"
if /i not "%SVC_STATE%"=="Running" (
    call :note "PostgreSQL: служба %DB_SERVICE% остановлена, запускаю..."
    call :db_start_service "%DB_SERVICE%"
    if errorlevel 1 (
        call :note "PostgreSQL: не удалось запустить службу %DB_SERVICE%."
        call :note "Запустите start.bat от имени администратора."
        exit /b 1
    )
) else (
    call :note "PostgreSQL: служба %DB_SERVICE% уже работает, ожидаю порт %DB_PORT%..."
)
call :db_wait
if errorlevel 1 (
    call :note "PostgreSQL: порт %DB_PORT% не открылся за 30 секунд."
    exit /b 1
)
call :note "PostgreSQL: готов, порт %DB_PORT%."
exit /b 0

:db_start_service
set "DB_SVC_NAME=%~1"
set "IS_ADMIN=0"
for /f "delims=" %%A in ('powershell -NoProfile -Command "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"') do set "IS_ADMIN=%%A"
if /i "%IS_ADMIN%"=="True" (
    powershell -NoProfile -Command "try { Start-Service -Name '%DB_SVC_NAME%' -ErrorAction Stop; exit 0 } catch { Write-Output ('ERROR: ' + $_.Exception.Message); exit 1 }" > "%TEMP%\nexus-start-db.txt" 2>&1
) else (
    call :note "PostgreSQL: нужны права администратора, запрашиваю повышение прав..."
    powershell -NoProfile -Command "try { $p=Start-Process -FilePath 'powershell.exe' -ArgumentList '-NoProfile','-WindowStyle','Hidden','-Command','Start-Service -Name ''%DB_SVC_NAME%'' -ErrorAction Stop' -Verb RunAs -Wait -PassThru; if (-not $p -or $p.ExitCode -ne 0) { Write-Output 'ERROR: elevated Start-Service failed'; exit 1 }; exit 0 } catch { Write-Output ('ERROR: ' + $_.Exception.Message); exit 1 }" > "%TEMP%\nexus-start-db.txt" 2>&1
)
if errorlevel 1 (
    type "%TEMP%\nexus-start-db.txt"
    type "%TEMP%\nexus-start-db.txt" >> "%LOG%"
    exit /b 1
)
type "%TEMP%\nexus-start-db.txt"
exit /b 0

:db_wait
powershell -NoProfile -Command "for ($i=0; $i -lt 30; $i++) { $c=Get-NetTCPConnection -LocalPort %DB_PORT% -State Listen -ErrorAction SilentlyContinue; if ($c) { exit 0 }; Start-Sleep -Seconds 1 }; exit 1"
if errorlevel 1 exit /b 1
exit /b 0

:db_status
set "DB_STATE=не настроен"
call :resolve_db
if not defined DB_URL exit /b 0
if /i "%DB_HOST%"=="INVALID" (
    set "DB_STATE=DATABASE_URL некорректен"
    exit /b 0
)
if not "%DB_LOCAL%"=="1" (
    set "DB_STATE=внешняя: %DB_HOST%:%DB_PORT%/%DB_NAME%"
    exit /b 0
)
call :db_listening
if "%DB_LISTENING%"=="1" (
    set "DB_STATE=работает, порт %DB_PORT%"
    exit /b 0
)
call :db_resolve_service
if /i "%DB_SERVICE%"=="NOT_FOUND" (
    set "DB_STATE=остановлен, порт %DB_PORT%"
    exit /b 0
)
call :db_service_state "%DB_SERVICE%"
if /i "%SVC_STATE%"=="Running" (
    set "DB_STATE=запускается, служба %DB_SERVICE%"
) else (
    set "DB_STATE=%SVC_STATE%, служба %DB_SERVICE%"
)
exit /b 0

:note
echo %~1
>>"%LOG%" echo %~1
exit /b 0

:banner
echo ============================================================
echo   NEXUS WORKSPACE - запуск
echo ============================================================
exit /b 0

:finish
echo.
if not "%NO_PAUSE%"=="1" pause
exit /b %CODE%
