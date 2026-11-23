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
set "LOG=%LOG_DIR%\first-run-%STAMP%.log"

set "CHECK_ONLY=0"
set "ENV_ONLY=0"
set "SETUP_ONLY=0"
set "NO_BUILD=0"
set "NO_PAUSE=0"
set "START_MODE="
set "DB_URL="
set "PUBLIC_URL="
set "INSTALL_DIR=C:\NexusWorkspace"
set "CODE=0"
set "IS_ADMIN=0"

for /f "delims=" %%A in ('powershell -NoProfile -Command "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"') do set "IS_ADMIN=%%A"
if /i "%IS_ADMIN%"=="True" set "IS_ADMIN=1"

:parse
if "%~1"=="" goto parsed
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
if /i "%~1"=="/?" goto usage
if /i "%~1"=="--check" (
    set "CHECK_ONLY=1"
    shift
    goto parse
)
if /i "%~1"=="--env-only" (
    set "ENV_ONLY=1"
    set "NO_BUILD=1"
    shift
    goto parse
)
if /i "%~1"=="--setup-only" (
    set "SETUP_ONLY=1"
    shift
    goto parse
)
if /i "%~1"=="--no-build" (
    set "NO_BUILD=1"
    shift
    goto parse
)
if /i "%~1"=="--no-pause" (
    set "NO_PAUSE=1"
    shift
    goto parse
)
if /i "%~1"=="--db-url" (
    if "%~2"=="" goto usage
    set "DB_URL=%~2"
    shift
    shift
    goto parse
)
if /i "%~1"=="--public-url" (
    if "%~2"=="" goto usage
    set "PUBLIC_URL=%~2"
    shift
    shift
    goto parse
)
if /i "%~1"=="--install-dir" (
    if "%~2"=="" goto usage
    set "INSTALL_DIR=%~2"
    shift
    shift
    goto parse
)
if /i "%~1"=="--console" (
    set "START_MODE=console"
    shift
    goto parse
)
if /i "%~1"=="--service" (
    set "START_MODE=service"
    shift
    goto parse
)
echo Неизвестный параметр: %~1
goto usage

:usage
echo.
echo NEXUS WORKSPACE - первый запуск и настройка
echo.
echo Использование:
echo   first-run.bat                 интерактивная настройка
echo   first-run.bat --check         только проверить окружение и .env
echo   first-run.bat --env-only      только создать .env
echo   first-run.bat --setup-only    .env + миграции + администратор + проверка
echo.
echo Параметры:
echo   --no-build                    не собирать frontend/backend
echo   --db-url URL                  строка PostgreSQL для создания .env
echo   --public-url URL              публичный URL для создания .env
echo   --console                     после настройки запустить в консоли
echo   --service                     после настройки установить службу
echo   --install-dir PATH            каталог службы (по умолчанию C:\NexusWorkspace)
echo   --no-pause                    не ждать нажатия клавиши в конце
echo.
echo Код возврата: 0 - успех, 1 - нет зависимостей, 2 - БД/конфигурация,
echo               3 - сборка, 4 - запуск.
goto finish

:parsed
call :banner
echo Журнал: %LOG%
echo.

if "%CHECK_ONLY%"=="1" goto check_only
if not exist "%ENVFILE%" goto create_env

echo Файл .env уже существует - не изменяю:
echo   %ENVFILE%
if "%ENV_ONLY%"=="1" (
    echo Режим --env-only: .env уже есть, выхожу.
    goto finish
)
goto after_env

:create_env
echo Файл .env не найден. Создаю новый.
echo.
if "%DB_URL%"=="" set /p "DB_URL=Строка PostgreSQL [postgres://nexus:nexus@127.0.0.1:5432/nexus]: "
if "%DB_URL%"=="" set "DB_URL=postgres://nexus:nexus@127.0.0.1:5432/nexus"
if "%PUBLIC_URL%"=="" set /p "PUBLIC_URL=Публичный URL [http://localhost:8080]: "
if "%PUBLIC_URL%"=="" set "PUBLIC_URL=http://localhost:8080"

set "STATIC_DIR="
if exist "%ROOT%\frontend\dist\index.html" set "STATIC_DIR=%ROOT%\frontend\dist"
if not defined STATIC_DIR if exist "%ROOT%\frontend\index.html" set "STATIC_DIR=%ROOT%\frontend"
if not defined STATIC_DIR set "STATIC_DIR=%ROOT%\frontend\dist"
set "STATIC_DIR=%STATIC_DIR:\=/%"
set "LOG_DIR_ENV=%ROOT%\logs"
set "LOG_DIR_ENV=%LOG_DIR_ENV:\=/%"

set "COOKIE_VALUE=false"
set "PROXY_VALUE=false"
echo %PUBLIC_URL% | findstr /I /C:"https://" >nul
if not errorlevel 1 (
    set "COOKIE_VALUE=true"
    set "PROXY_VALUE=true"
)

set "NEXUS_ENV_FILE=%ENVFILE%"
set "NEXUS_DB_URL=%DB_URL%"
set "NEXUS_PUBLIC_VALUE=%PUBLIC_URL%"
set "NEXUS_STATIC_VALUE=%STATIC_DIR%"
set "NEXUS_LOG_VALUE=%LOG_DIR_ENV%"
set "NEXUS_COOKIE_VALUE=%COOKIE_VALUE%"
set "NEXUS_PROXY_VALUE=%PROXY_VALUE%"

echo Создаю .env...
powershell -NoProfile -ExecutionPolicy Bypass -Command "$b=New-Object byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b); $secret=([BitConverter]::ToString($b) -replace '-','').ToLower(); $lines=@('# Создано first-run.bat', ('DATABASE_URL=' + $env:NEXUS_DB_URL), ('NEXUS_SECRET=' + $secret), 'NEXUS_HOST=127.0.0.1', 'NEXUS_PORT=8080', ('NEXUS_PUBLIC_URL=' + $env:NEXUS_PUBLIC_VALUE), ('NEXUS_COOKIE_SECURE=' + $env:NEXUS_COOKIE_VALUE), ('NEXUS_STATIC_DIR=' + $env:NEXUS_STATIC_VALUE), ('NEXUS_TRUST_PROXY=' + $env:NEXUS_PROXY_VALUE), ('NEXUS_LOG_DIR=' + $env:NEXUS_LOG_VALUE), 'NEXUS_LOG_FILE=nexus.log', 'NEXUS_LOG_KEEP=7', 'NEXUS_PASSWORD_MIN_LENGTH=12', 'RUST_LOG=nexus=info,tower_http=warn,sqlx=warn'); [IO.File]::WriteAllLines($env:NEXUS_ENV_FILE, $lines, (New-Object System.Text.UTF8Encoding($false)))"
if errorlevel 1 (
    echo Не удалось создать .env.
    set "CODE=2"
    goto finish
)

echo.
echo .env создан.
echo ВАЖНО: проверьте DATABASE_URL и при необходимости поправьте NEXUS_PUBLIC_URL.
echo Пароль БД с символами @ : / ? # лучше заменить на пароль без них
echo или создать .env вручную с URL-кодированием.
echo.
if "%ENV_ONLY%"=="1" (
    echo Режим --env-only: выхожу без запуска сборки и миграций.
    goto finish
)

:after_env
call :read_env
if "%NO_BUILD%"=="1" goto skip_build
if not exist "%ROOT%\backend\Cargo.toml" goto skip_build

echo Проверяю инструменты сборки...
where cargo >nul 2>&1
if errorlevel 1 (
    echo ОШИБКА: не найден cargo. Установите Rust 1.85+ или используйте release-архив.
    set "CODE=1"
    goto finish
)
where npm >nul 2>&1
if errorlevel 1 (
    echo ОШИБКА: не найден npm. Установите Node.js 20+ или используйте release-архив.
    set "CODE=1"
    goto finish
)

echo.
echo Сборка frontend...
pushd "%ROOT%\frontend"
call npm run build >> "%LOG%" 2>&1
set "BUILD_CODE=%ERRORLEVEL%"
popd
powershell -NoProfile -Command "Get-Content -LiteralPath '%LOG%' -Tail 30"
if not "%BUILD_CODE%"=="0" (
    echo ОШИБКА сборки frontend. Полный журнал: %LOG%
    set "CODE=3"
    goto finish
)

echo.
echo Сборка backend (release, -j 2)...
pushd "%ROOT%\backend"
call cargo build --release -j 2 >> "%LOG%" 2>&1
set "BUILD_CODE=%ERRORLEVEL%"
popd
powershell -NoProfile -Command "Get-Content -LiteralPath '%LOG%' -Tail 30"
if not "%BUILD_CODE%"=="0" (
    echo ОШИБКА сборки backend. Полный журнал: %LOG%
    set "CODE=3"
    goto finish
)

:skip_build
call :find_exe
if not defined EXE (
    echo ОШИБКА: nexus.exe не найден.
    echo Соберите backend или распакуйте release-архив рядом с этим BAT.
    set "CODE=3"
    goto finish
)
call :set_local_env
echo Использую: %EXE%
echo Рабочий каталог: %WORKDIR%

echo.
echo Применяю миграции...
pushd "%WORKDIR%"
"%EXE%" migrate > "%TEMP%\nexus-first-migrate.log" 2>&1
set "MIGRATE_CODE=%ERRORLEVEL%"
type "%TEMP%\nexus-first-migrate.log"
type "%TEMP%\nexus-first-migrate.log" >> "%LOG%"
popd
if not "%MIGRATE_CODE%"=="0" (
    echo ОШИБКА миграций. Проверьте DATABASE_URL и доступность PostgreSQL.
    set "CODE=2"
    goto finish
)

echo.
echo Проверяю наличие администратора...
pushd "%WORKDIR%"
"%EXE%" check --json > "%TEMP%\nexus-first-check.json" 2>&1
set "CHECK_CODE=%ERRORLEVEL%"
type "%TEMP%\nexus-first-check.json"
type "%TEMP%\nexus-first-check.json" >> "%LOG%"
popd
set "HAS_ADMIN=yes"
findstr /C:"\"admin_present\":false" "%TEMP%\nexus-first-check.json" >nul 2>&1
if not errorlevel 1 set "HAS_ADMIN=no"
if /i "%HAS_ADMIN%"=="no" (
    echo.
    echo Администратора нет. Создаю пользователя admin.
    echo Пароль вводится скрыто и не сохраняется в BAT.
    if defined NEXUS_ADMIN_PASSWORD (
        "%EXE%" create-admin --username admin --password-env NEXUS_ADMIN_PASSWORD
    ) else (
        "%EXE%" create-admin --username admin
    )
    if errorlevel 1 (
        echo Не удалось создать администратора.
        set "CODE=2"
        goto finish
    )
)

echo.
echo Итоговая проверка...
pushd "%WORKDIR%"
"%EXE%" check
set "CHECK_CODE=%ERRORLEVEL%"
popd
if not "%CHECK_CODE%"=="0" (
    echo Проверка нашла проблемы. Исправьте их перед запуском.
    set "CODE=2"
    goto finish
)

echo.
echo Настройка завершена.
if "%SETUP_ONLY%"=="1" (
    echo Режим --setup-only: сервер не запускаю.
    goto finish
)
if "%START_MODE%"=="console" goto start_console
if "%START_MODE%"=="service" goto install_service

echo.
echo 1. Запустить в консоли сейчас
echo 2. Установить как службу Windows
echo 3. Выйти
choice /C 123 /N /M "Выбор [3]: "
if errorlevel 3 goto finish
if errorlevel 2 goto install_service
set "START_MODE=console"
goto start_console

:check_only
if not exist "%ENVFILE%" (
    echo ОШИБКА: %ENVFILE% не найден.
    set "CODE=2"
    goto finish
)
call :find_exe
if not defined EXE (
    echo ОШИБКА: nexus.exe не найден.
    set "CODE=3"
    goto finish
)
echo Файл .env: %ENVFILE%
call :set_local_env
echo nexus.exe: %EXE%
echo.
pushd "%WORKDIR%"
"%EXE%" check --json
set "CODE=%ERRORLEVEL%"
popd
goto finish

:start_console
set "SVC_STATE="
for /f "delims=" %%S in ('powershell -NoProfile -Command "try { (Get-Service NexusWorkspace -ErrorAction Stop).Status } catch { 'MISSING' }"') do set "SVC_STATE=%%S"
if /i "%SVC_STATE%"=="Running" (
    echo.
    echo Служба NexusWorkspace уже запущена. Остановите её перед консольным запуском:
    echo   Stop-Service NexusWorkspace
    set "CODE=4"
    goto finish
)
echo.
echo Запускаю NEXUS в консоли. Для остановки нажмите Ctrl+C.
if defined PUBLIC_URL echo Адрес: %PUBLIC_URL%
if not defined PUBLIC_URL echo Адрес: http://127.0.0.1:%PORT%
echo.
pushd "%WORKDIR%"
"%EXE%" serve
set "CODE=%ERRORLEVEL%"
popd
goto finish

:install_service
if not "%IS_ADMIN%"=="1" (
    echo.
    echo Для установки службы запустите first-run.bat от имени администратора.
    set "CODE=4"
    goto finish
)
if not exist "%ROOT%\deployment\install-windows.ps1" (
    echo ОШИБКА: не найден deployment\install-windows.ps1.
    set "CODE=3"
    goto finish
)
if exist "%INSTALL_DIR%\.env" (
    echo.
    echo В %INSTALL_DIR% уже есть .env.
    echo Автоматическая установка остановлена, чтобы не перезаписать рабочую БД.
    echo Используйте deployment\install-windows.ps1 или update-windows.ps1 вручную.
    set "CODE=2"
    goto finish
)

echo.
echo Копирую .env в %INSTALL_DIR% и подставляю пути установки...
powershell -NoProfile -ExecutionPolicy Bypass -Command "$src='%ENVFILE%'; $dst='%INSTALL_DIR%\.env'; $install=('%INSTALL_DIR%' -replace '\\','/'); $lines=Get-Content -LiteralPath $src; $lines=$lines | ForEach-Object { if ($_ -match '^NEXUS_STATIC_DIR=') { 'NEXUS_STATIC_DIR=' + $install + '/frontend' } elseif ($_ -match '^NEXUS_LOG_DIR=') { 'NEXUS_LOG_DIR=' + $install + '/logs' } else { $_ } }; New-Item -ItemType Directory -Force -Path '%INSTALL_DIR%' | Out-Null; [IO.File]::WriteAllLines($dst, $lines, (New-Object System.Text.UTF8Encoding($false)))"
if errorlevel 1 (
    echo Не удалось подготовить .env для установки.
    set "CODE=2"
    goto finish
)

echo.
echo Устанавливаю службу NexusWorkspace...
powershell -ExecutionPolicy Bypass -File "%ROOT%\deployment\install-windows.ps1" -SkipBuild -InstallDir "%INSTALL_DIR%" -TestServiceControl
if errorlevel 1 (
    echo Установка службы завершилась с ошибкой.
    set "CODE=4"
    goto finish
)
echo.
powershell -NoProfile -Command "Get-Service NexusWorkspace | Format-Table Name,Status,StartType -AutoSize"
set "CODE=0"
goto finish

:set_local_env
if /i not "%WORKDIR%"=="%ROOT%" exit /b 0
if exist "%ROOT%\frontend\dist\index.html" set "NEXUS_STATIC_DIR=%ROOT%\frontend\dist"
if exist "%ROOT%\logs" set "NEXUS_LOG_DIR=%ROOT%\logs"
exit /b 0

:read_env
set "PORT=8080"
if not exist "%ENVFILE%" exit /b 0
for /f "usebackq tokens=1,* delims==" %%A in ("%ENVFILE%") do (
    if /i "%%A"=="NEXUS_PUBLIC_URL" if not defined PUBLIC_URL set "PUBLIC_URL=%%B"
    if /i "%%A"=="NEXUS_PORT" set "PORT=%%B"
)
if defined PUBLIC_URL set "PUBLIC_URL=%PUBLIC_URL:"=%"
if defined PORT set "PORT=%PORT:"=%"
exit /b 0

:find_exe
set "EXE="
set "WORKDIR="
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

:banner
echo ============================================================
echo   NEXUS WORKSPACE - первый запуск и настройка
echo ============================================================
exit /b 0

:finish
echo.
if not "%NO_PAUSE%"=="1" pause
exit /b %CODE%
