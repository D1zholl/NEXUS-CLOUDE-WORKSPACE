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
set "LOG=%LOG_DIR%\stop-%STAMP%.log"

set "STATUS_ONLY=0"
set "DRY_RUN=0"
set "ASSUME_YES=0"
set "NO_PAUSE=0"
set "STOP_PROXY=1"
set "STOP_DB=1"
set "TIMEOUT=40"
set "PORT=8080"
set "DB_URL="
set "DB_HOST="
set "DB_PORT=5432"
set "DB_NAME="
set "DB_SERVICE="
set "DB_STATE=не настроен"
set "DB_RUNNING=0"
set "NEXUS_SERVICE=NexusWorkspace"
set "PROXY_SERVICE=Caddy"
set "INSTALL_DIR=C:\NexusWorkspace"
set "SVC_STATE=MISSING"
set "SVC_NAME="
set "SVC_LABEL="
set "PIDS="
set "HEALTH=down"
set "IS_ADMIN=0"
set "NEED_ADMIN=0"
set "FAILED=0"
set "CODE=0"

for /f "delims=" %%A in ('powershell -NoProfile -Command "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"') do set "IS_ADMIN=%%A"
if /i "%IS_ADMIN%"=="True" set "IS_ADMIN=1"
if /i not "%IS_ADMIN%"=="1" set "IS_ADMIN=0"

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
if /i "%~1"=="/?" goto usage
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
if /i "%~1"=="--dry-run" (
    set "DRY_RUN=1"
    shift
    goto parse
)
if /i "%~1"=="--yes" (
    set "ASSUME_YES=1"
    shift
    goto parse
)
if /i "%~1"=="-y" (
    set "ASSUME_YES=1"
    shift
    goto parse
)
if /i "%~1"=="--keep-db" (
    set "STOP_DB=0"
    shift
    goto parse
)
if /i "%~1"=="--no-db" (
    set "STOP_DB=0"
    shift
    goto parse
)
if /i "%~1"=="--keep-proxy" (
    set "STOP_PROXY=0"
    shift
    goto parse
)
if /i "%~1"=="--no-proxy" (
    set "STOP_PROXY=0"
    shift
    goto parse
)
if /i "%~1"=="--no-pause" (
    set "NO_PAUSE=1"
    shift
    goto parse
)
if /i "%~1"=="--timeout" (
    if "%~2"=="" goto usage
    echo %~2| findstr /r "^[0-9][0-9]*$" >nul
    if errorlevel 1 (
        echo Некорректное значение --timeout: %~2
        goto usage
    )
    set "TIMEOUT=%~2"
    shift
    shift
    goto parse
)
echo Неизвестный параметр: %~1
goto usage

:usage
echo.
echo NEXUS WORKSPACE - полная остановка
echo.
echo Использование:
echo   stop.bat                  остановить все компоненты NEXUS
echo   stop.bat --status         показать состояние без остановки
echo   stop.bat --dry-run        показать план без изменений
echo   stop.bat --yes            не спрашивать подтверждение
echo   stop.bat --keep-db        не останавливать PostgreSQL
echo   stop.bat --keep-proxy     не останавливать Caddy
echo   stop.bat --timeout 60     ждать остановки службы до 60 секунд
echo   stop.bat --no-pause       не ждать нажатия клавиши
echo.
echo Останавливаются: консольные nexus.exe, служба NexusWorkspace,
echo служба Caddy и служба PostgreSQL из DATABASE_URL.
echo Сама Windows не выключается.
echo.
echo Коды возврата: 0 - успех, 1 - компоненты ещё работают,
echo 2 - нужны права администратора, 4 - ошибка, 5 - отменено.
set "CODE=0"
set "NO_PAUSE=1"
goto finish

:parsed
call :banner
call :note "Журнал: %LOG%"
call :read_env
call :resolve_db

if "%STATUS_ONLY%"=="1" goto status

call :show_plan
if "%DRY_RUN%"=="1" goto stop
if "%ASSUME_YES%"=="1" goto stop

echo.
choice /C YN /N /M "Остановить перечисленные компоненты? [Y/N]: "
if errorlevel 2 goto cancelled
if errorlevel 1 goto stop
goto cancelled

:cancelled
call :note "Остановка отменена."
set "CODE=5"
goto finish

:stop
if "%DRY_RUN%"=="1" call :note "Режим проверки: действия не выполняются."
call :note "Начинаю остановку."
call :stop_service "%NEXUS_SERVICE%" "NEXUS"
call :stop_console
if "%STOP_PROXY%"=="1" (
    call :stop_service "%PROXY_SERVICE%" "Caddy"
) else (
    call :note "Caddy: пропущено по параметру --keep-proxy."
)
if "%STOP_DB%"=="1" (
    call :stop_database
) else (
    call :note "PostgreSQL: пропущено по параметру --keep-db."
)
call :verify
goto summary

:status
call :resolve_db_service
call :service_state "%NEXUS_SERVICE%"
set "NX_STATE=%SVC_STATE%"
call :service_state "%PROXY_SERVICE%"
set "PROXY_STATE=%SVC_STATE%"
call :find_nexus
call :health
call :db_state

echo.
echo --- Состояние NEXUS ---
if defined PIDS (
    echo Консольный nexus.exe: работает, PID %PIDS%
) else (
    echo Консольный nexus.exe: не найден
)
echo Служба NexusWorkspace: %NX_STATE%
echo Служба Caddy:          %PROXY_STATE%
echo PostgreSQL:            %DB_STATE%
echo HTTP :%PORT%:           %HEALTH%
echo Журнал:                %LOG%
echo.

set "RUNNING=0"
if defined PIDS set "RUNNING=1"
if /i "%NX_STATE%"=="Running" set "RUNNING=1"
if /i "%PROXY_STATE%"=="Running" set "RUNNING=1"
if "%DB_RUNNING%"=="1" set "RUNNING=1"
if "%RUNNING%"=="1" (
    echo Система ещё работает.
    set "CODE=1"
) else (
    echo Система остановлена.
    set "CODE=0"
)
goto finish

:show_plan
echo.
echo --- План остановки ---
echo Консольные nexus.exe: %ROOT% и %INSTALL_DIR%
echo Служба NEXUS:        %NEXUS_SERVICE%
if "%STOP_PROXY%"=="1" (
    echo Служба прокси:      %PROXY_SERVICE%
) else (
    echo Служба прокси:      не останавливать
)
if "%STOP_DB%"=="1" (
    if defined DB_URL (
        echo PostgreSQL:         %DB_HOST%:%DB_PORT%/%DB_NAME%
    ) else (
        echo PostgreSQL:         DATABASE_URL не задан, будет пропущено
    )
) else (
    echo PostgreSQL:         не останавливать
)
if not "%IS_ADMIN%"=="1" (
    echo.
    echo ВНИМАНИЕ: нет прав администратора.
    echo Службы NexusWorkspace и Caddy не удастся остановить.
    echo Запустите stop.bat от имени администратора.
)
echo.
exit /b 0

:stop_console
call :find_nexus
if not defined PIDS (
    call :note "Консольный nexus.exe: не найден."
    exit /b 0
)
call :note "Консольный nexus.exe: найден, PID %PIDS%."
if "%DRY_RUN%"=="1" (
    call :note "Консольный nexus.exe: будет остановлен."
    exit /b 0
)
for %%P in (%PIDS%) do taskkill /PID %%P >nul 2>&1
ping -n 4 127.0.0.1 >nul 2>&1
call :find_nexus
if defined PIDS (
    call :note "Не завершились штатно, завершаю принудительно: PID %PIDS%."
    for %%P in (%PIDS%) do taskkill /F /T /PID %%P >nul 2>&1
    ping -n 2 127.0.0.1 >nul 2>&1
)
call :find_nexus
if defined PIDS (
    call :note "ОШИБКА: nexus.exe всё ещё работает, PID %PIDS%."
    set /a FAILED+=1
    exit /b 1
)
call :note "Консольный nexus.exe остановлен."
exit /b 0

:stop_service
set "SVC_NAME=%~1"
set "SVC_LABEL=%~2"
call :service_state "%SVC_NAME%"
if /i "%SVC_STATE%"=="MISSING" (
    call :note "Служба %SVC_LABEL% - %SVC_NAME%: не зарегистрирована, пропущено."
    exit /b 0
)
if /i "%SVC_STATE%"=="Stopped" (
    call :note "Служба %SVC_LABEL% - %SVC_NAME%: уже остановлена."
    exit /b 0
)
if not "%IS_ADMIN%"=="1" (
    call :note "Служба %SVC_LABEL% - %SVC_NAME%: нужны права администратора."
    set "NEED_ADMIN=1"
    exit /b 1
)
if "%DRY_RUN%"=="1" (
    call :note "Служба %SVC_LABEL% - %SVC_NAME%: будет остановлена, сейчас %SVC_STATE%."
    exit /b 0
)

call :note "Останавливаю службу %SVC_LABEL% - %SVC_NAME%..."
powershell -NoProfile -Command "try { $s=Get-Service -Name '%SVC_NAME%' -ErrorAction Stop; if ($s.Status -ne 'Stopped') { Stop-Service -Name '%SVC_NAME%' -Force -ErrorAction Stop; $s.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(%TIMEOUT%)) }; exit 0 } catch { Write-Output ('ERROR: ' + $_.Exception.Message); exit 1 }" > "%TEMP%\nexus-stop-service.txt" 2>&1
set "SVC_RC=%ERRORLEVEL%"
type "%TEMP%\nexus-stop-service.txt"
type "%TEMP%\nexus-stop-service.txt" >> "%LOG%"
call :service_state "%SVC_NAME%"
if not "%SVC_RC%"=="0" (
    call :note "ОШИБКА: служба %SVC_LABEL% - %SVC_NAME% не остановлена, состояние %SVC_STATE%."
    set /a FAILED+=1
    exit /b 1
)
if /i not "%SVC_STATE%"=="Stopped" (
    call :note "ОШИБКА: служба %SVC_LABEL% - %SVC_NAME% осталась в состоянии %SVC_STATE%."
    set /a FAILED+=1
    exit /b 1
)
call :note "Служба %SVC_LABEL% - %SVC_NAME% остановлена."
exit /b 0

:stop_database
if not defined DB_URL (
    call :note "PostgreSQL: DATABASE_URL не задан, пропущено."
    exit /b 0
)
if "%DB_HOST%"=="INVALID" (
    call :note "PostgreSQL: не удалось разобрать DATABASE_URL, пропущено."
    exit /b 0
)
call :note "PostgreSQL: %DB_HOST%:%DB_PORT%/%DB_NAME%"
if not "%DB_LOCAL%"=="1" (
    call :note "PostgreSQL: база внешняя, останавливать нечего."
    exit /b 0
)
call :resolve_db_service
if /i "%DB_SERVICE%"=="NOT_LISTENING" (
    call :note "PostgreSQL: порт %DB_PORT% не слушается, уже остановлен."
    exit /b 0
)
if /i "%DB_SERVICE%"=="NO_SERVICE" (
    call :note "PostgreSQL: не удалось определить службу для порта %DB_PORT%."
    call :note "Остановите PostgreSQL вручную или используйте --keep-db."
    set /a FAILED+=1
    exit /b 1
)
if /i "%DB_SERVICE%"=="UNKNOWN" (
    call :note "PostgreSQL: состояние службы неизвестно, пропущено."
    exit /b 0
)
call :stop_service "%DB_SERVICE%" "PostgreSQL"
exit /b 0

:verify
if "%DRY_RUN%"=="1" exit /b 0

call :find_nexus
if defined PIDS (
    call :note "ПРОВЕРКА: nexus.exe всё ещё работает, PID %PIDS%."
    set /a FAILED+=1
)
call :service_state "%NEXUS_SERVICE%"
if /i not "%SVC_STATE%"=="Stopped" if /i not "%SVC_STATE%"=="MISSING" (
    call :note "ПРОВЕРКА: служба NexusWorkspace в состоянии %SVC_STATE%."
    set /a FAILED+=1
)
if "%STOP_PROXY%"=="1" (
    call :service_state "%PROXY_SERVICE%"
    if /i not "%SVC_STATE%"=="Stopped" if /i not "%SVC_STATE%"=="MISSING" (
        call :note "ПРОВЕРКА: служба Caddy в состоянии %SVC_STATE%."
        set /a FAILED+=1
    )
)
if "%STOP_DB%"=="1" if defined DB_SERVICE if /i not "%DB_SERVICE%"=="NOT_LISTENING" if /i not "%DB_SERVICE%"=="NO_SERVICE" (
    call :service_state "%DB_SERVICE%"
    if /i not "%SVC_STATE%"=="Stopped" if /i not "%SVC_STATE%"=="MISSING" (
        call :note "ПРОВЕРКА: служба %DB_SERVICE% в состоянии %SVC_STATE%."
        set /a FAILED+=1
    )
)
call :health
if /i not "%HEALTH%"=="down" (
    call :note "ПРОВЕРКА: HTTP :%PORT% ещё отвечает - %HEALTH%."
    set /a FAILED+=1
)
exit /b 0

:summary
echo.
echo ============================================================
if "%DRY_RUN%"=="1" (
    echo Проверка завершена, изменения не вносились.
    set "CODE=0"
    goto finish
)
if "%NEED_ADMIN%"=="1" (
    echo Остановка выполнена не полностью: нужны права администратора.
    echo Запустите stop.bat от имени администратора.
    set "CODE=2"
    goto finish
)
if not "%FAILED%"=="0" (
    echo Остановка выполнена не полностью. Смотрите сообщения выше.
    set "CODE=4"
    goto finish
)
echo Все компоненты NEXUS остановлены.
echo Для запуска используйте start.bat.
set "CODE=0"
goto finish

:read_env
if not exist "%ENVFILE%" exit /b 0
for /f "usebackq tokens=1,* delims==" %%A in ("%ENVFILE%") do (
    if /i "%%A"=="NEXUS_PORT" set "PORT=%%B"
    if /i "%%A"=="DATABASE_URL" set "DB_URL=%%B"
)
if defined PORT set "PORT=%PORT:"=%"
if defined DB_URL set "DB_URL=%DB_URL:"=%"
exit /b 0

:resolve_db
set "DB_HOST="
set "DB_PORT=5432"
set "DB_NAME="
set "DB_LOCAL=0"
if not defined DB_URL exit /b 0
set "NEXUS_STOP_DB_URL=%DB_URL%"
for /f "tokens=1,2,3 delims=|" %%A in ('powershell -NoProfile -Command "$u=$null; try { $u=New-Object System.Uri($env:NEXUS_STOP_DB_URL) } catch { 'INVALID'; exit 0 }; if (-not $u) { 'INVALID'; exit 0 }; $port=5432; if ($u.Port -gt 0) { $port=$u.Port }; Write-Output ($u.Host + '|' + $port + '|' + $u.AbsolutePath.Trim('/'))"') do (
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

:resolve_db_service
set "DB_SERVICE=UNKNOWN"
if not defined DB_URL exit /b 0
if not defined DB_PORT set "DB_PORT=5432"
if not "%DB_LOCAL%"=="1" exit /b 0
for /f "delims=" %%S in ('powershell -NoProfile -Command "$port=%DB_PORT%; $c=Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1; if (-not $c) { 'NOT_LISTENING'; exit 0 }; $services=Get-CimInstance Win32_Service -ErrorAction SilentlyContinue; $ownerProcess=[uint32]$c.OwningProcess; $found=''; for ($i=0; $i -lt 8 -and $ownerProcess -gt 0; $i++) { $svc=$services | Where-Object { $_.ProcessId -eq $ownerProcess } | Select-Object -First 1; if ($svc) { $found=$svc.Name; break }; $proc=Get-CimInstance Win32_Process -Filter ('ProcessId=' + $ownerProcess) -ErrorAction SilentlyContinue; if (-not $proc) { break }; $ownerProcess=[uint32]$proc.ParentProcessId }; if ($found) { $found } else { 'NO_SERVICE' }"') do set "DB_SERVICE=%%S"
exit /b 0

:db_state
set "DB_STATE=не настроен"
set "DB_RUNNING=0"
if not defined DB_URL exit /b 0
if /i "%DB_HOST%"=="INVALID" (
    set "DB_STATE=DATABASE_URL некорректен"
    exit /b 0
)
if not "%DB_LOCAL%"=="1" (
    set "DB_STATE=внешняя: %DB_HOST%:%DB_PORT%/%DB_NAME%"
    exit /b 0
)
if /i "%DB_SERVICE%"=="NOT_LISTENING" (
    set "DB_STATE=остановлен"
    exit /b 0
)
if /i "%DB_SERVICE%"=="NO_SERVICE" (
    set "DB_STATE=работает вне службы, порт %DB_PORT%"
    set "DB_RUNNING=1"
    exit /b 0
)
if /i "%DB_SERVICE%"=="UNKNOWN" (
    set "DB_STATE=неизвестно, порт %DB_PORT%"
    exit /b 0
)
call :service_state "%DB_SERVICE%"
if /i "%SVC_STATE%"=="Running" (
    set "DB_STATE=работает, служба %DB_SERVICE%"
    set "DB_RUNNING=1"
) else (
    set "DB_STATE=%SVC_STATE%, служба %DB_SERVICE%"
)
exit /b 0

:find_nexus
set "PIDS="
set "NEXUS_STOP_ROOT=%ROOT%\"
set "NEXUS_STOP_INSTALL=%INSTALL_DIR%\"
set "NEXUS_STOP_HOME="
if defined NEXUS_HOME set "NEXUS_STOP_HOME=%NEXUS_HOME%\"
set "NEXUS_STOP_SERVICE_NAME=%NEXUS_SERVICE%"
for /f "delims=" %%P in ('powershell -NoProfile -Command "$svc=Get-CimInstance Win32_Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -eq $env:NEXUS_STOP_SERVICE_NAME } | Select-Object -First 1; $svcProcess=0; if ($svc) { $svcProcess=$svc.ProcessId }; Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -eq 'nexus.exe' -and $_.ProcessId -ne $svcProcess -and $_.ExecutablePath -and (($_.ExecutablePath.StartsWith($env:NEXUS_STOP_ROOT,[StringComparison]::OrdinalIgnoreCase)) -or ($_.ExecutablePath.StartsWith($env:NEXUS_STOP_INSTALL,[StringComparison]::OrdinalIgnoreCase)) -or ($env:NEXUS_STOP_HOME -and $_.ExecutablePath.StartsWith($env:NEXUS_STOP_HOME,[StringComparison]::OrdinalIgnoreCase))) } | ForEach-Object { $_.ProcessId }"') do call :append_pid %%P
exit /b 0

:append_pid
if defined PIDS (
    set "PIDS=%PIDS% %1"
) else (
    set "PIDS=%1"
)
exit /b 0

:service_state
set "SVC_STATE=MISSING"
for /f "delims=" %%S in ('powershell -NoProfile -Command "try { (Get-Service -Name '%~1' -ErrorAction Stop).Status } catch { 'MISSING' }"') do set "SVC_STATE=%%S"
exit /b 0

:health
set "HEALTH=down"
for /f "delims=" %%H in ('powershell -NoProfile -Command "try { $r=Invoke-WebRequest 'http://127.0.0.1:%PORT%/api/v1/health' -UseBasicParsing -TimeoutSec 3; Write-Output ('HTTP ' + [int]$r.StatusCode) } catch { Write-Output 'down' }"') do set "HEALTH=%%H"
exit /b 0

:note
echo %~1
>>"%LOG%" echo %~1
exit /b 0

:banner
echo ============================================================
echo   NEXUS WORKSPACE - полная остановка
echo ============================================================
exit /b 0

:finish
echo.
if not "%NO_PAUSE%"=="1" pause
exit /b %CODE%
