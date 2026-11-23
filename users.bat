@echo off
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "LOG_DIR=%ROOT%\logs"
if not exist "%LOG_DIR%" mkdir "%LOG_DIR%" >nul 2>&1

for /f "delims=" %%I in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%I"
if "%STAMP%"=="" set "STAMP=%RANDOM%"
set "LOG=%LOG_DIR%\users-%STAMP%.log"
set "PREFER=auto"
set "EXE="
set "WORKDIR="
set "RC=0"
set "NO_PAUSE=0"

:parse
if "%~1"=="" goto menu_start
if /i "%~1"=="--help" goto usage
if /i "%~1"=="-h" goto usage
if /i "%~1"=="/?" goto usage
if /i "%~1"=="--list" goto list_now
if /i "%~1"=="--installed" (
    set "PREFER=installed"
    shift
    goto parse
)
if /i "%~1"=="--local" (
    set "PREFER=local"
    shift
    goto parse
)
if /i "%~1"=="--no-pause" (
    set "NO_PAUSE=1"
    shift
    goto parse
)
echo Неизвестный параметр: %~1
goto usage

:usage
echo.
echo NEXUS WORKSPACE - управление учётными записями
echo.
echo Использование:
echo   users.bat                интерактивное меню
echo   users.bat --list         показать пользователей и выйти
echo   users.bat --local        использовать сборку из этого каталога
echo   users.bat --installed    использовать C:\NexusWorkspace\nexus.exe
echo   users.bat --no-pause     не ждать нажатия клавиши
echo.
echo Пароли вводятся только внутри nexus.exe скрытым вводом.
set "NO_PAUSE=1"
goto finish

:menu_start
call :banner
call :find_exe
if not defined EXE goto no_exe
call :check_user_support
if errorlevel 1 goto old_exe
echo nexus.exe: %EXE%
echo Рабочий каталог: %WORKDIR%
echo Журнал: %LOG%
echo База данных берётся из .env в рабочем каталоге.
echo.

:menu
echo ============================================================
echo   1. Список пользователей
echo   2. Создать пользователя
echo   3. Сменить пароль пользователя
echo   4. Заблокировать пользователя
echo   5. Разблокировать пользователя
echo   6. Снять блокировку после неудачных входов
echo   7. Завершить все сессии пользователя
echo   8. Выдать роль
echo   9. Снять роль
echo   A. Создать суперпользователя
echo   B. Изменить данные суперпользователя (логин, имя, пароль)
echo   C. Удалить пользователя (user или superadmin)
echo   0. Выход
echo ============================================================
choice /C 0123456789ABC /N /M "Выбор: "
if errorlevel 13 goto delete_user
if errorlevel 12 goto update_super
if errorlevel 11 goto create_super
if errorlevel 10 goto remove_role
if errorlevel 9 goto add_role
if errorlevel 8 goto revoke_sessions
if errorlevel 7 goto unlock_user
if errorlevel 6 goto enable_user
if errorlevel 5 goto disable_user
if errorlevel 4 goto change_password
if errorlevel 3 goto create_user
if errorlevel 2 goto show_list
if errorlevel 1 goto finish
goto menu

:list_now
set "NO_PAUSE=1"
call :find_exe
if not defined EXE goto no_exe
call :check_user_support
if errorlevel 1 goto old_exe
call :run_captured user list
set "RC=%ERRORLEVEL%"
goto finish

:show_list
call :run_captured user list
if errorlevel 1 (echo ОШИБКА выполнения.) else (echo Готово.)
pause
goto menu

:create_user
echo.
set "U="
set /p "U=Логин нового пользователя: "
if not defined U goto menu
set "D="
set /p "D=Отображаемое имя [как логин]: "
set "R="
set /p "R=Роль [user]: "
if not defined R set "R=user"
if /i "%R%"=="administrator" (
    if defined D (
        call :run_interactive user create --username "%U%" --display-name "%D%" --role administrator --yes
    ) else (
        call :run_interactive user create --username "%U%" --role administrator --yes
    )
) else (
    if defined D (
        call :run_interactive user create --username "%U%" --display-name "%D%" --role "%R%"
    ) else (
        call :run_interactive user create --username "%U%" --role "%R%"
    )
)
if errorlevel 1 (echo ОШИБКА создания.) else (echo Пользователь создан.)
pause
goto menu

:change_password
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
choice /C YN /N /M "Завершить все его сессии после смены пароля? [Y/N]: "
if errorlevel 2 (
    call :run_interactive user passwd --username "%U%"
) else (
    call :run_interactive user passwd --username "%U%" --revoke-sessions
)
if errorlevel 1 (echo ОШИБКА смены пароля.) else (echo Пароль изменён.)
pause
goto menu

:disable_user
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
choice /C YN /N /M "Заблокировать %U%? [Y/N]: "
if errorlevel 2 goto menu
call :run_captured user disable --username "%U%" --yes
if errorlevel 1 (echo ОШИБКА блокировки.) else (echo Пользователь заблокирован.)
pause
goto menu

:enable_user
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
call :run_captured user enable --username "%U%"
if errorlevel 1 (echo ОШИБКА разблокировки.) else (echo Пользователь разблокирован.)
pause
goto menu

:unlock_user
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
call :run_captured user unlock --username "%U%"
if errorlevel 1 (echo ОШИБКА снятия блокировки.) else (echo Блокировка снята.)
pause
goto menu

:revoke_sessions
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
call :run_captured user sessions revoke --username "%U%"
if errorlevel 1 (echo ОШИБКА отзыва сессий.) else (echo Сессии завершены.)
pause
goto menu

:add_role
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
set "R="
set /p "R=Роль [user]: "
if not defined R set "R=user"
call :run_captured user role add --username "%U%" --role "%R%"
if errorlevel 1 (echo ОШИБКА выдачи роли.) else (echo Роль выдана.)
pause
goto menu

:remove_role
echo.
set "U="
set /p "U=Логин пользователя: "
if not defined U goto menu
set "R="
set /p "R=Роль [user]: "
if not defined R set "R=user"
call :run_captured user role remove --username "%U%" --role "%R%"
if errorlevel 1 (echo ОШИБКА снятия роли.) else (echo Роль снята.)
pause
goto menu

:create_super
echo.
echo Создание нового суперпользователя.
echo Пароль будет запрошен скрыто внутри nexus.exe.
set "U="
set /p "U=Логин [admin]: "
if not defined U set "U=admin"
call :run_interactive user create --username "%U%" --role administrator --yes
if errorlevel 1 (echo ОШИБКА создания суперпользователя.) else (echo Суперпользователь создан.)
pause
goto menu

:update_super
echo.
echo Изменение логина, отображаемого имени и пароля суперпользователя.
echo Пароль вводится скрыто. Пустой ввод оставляет текущее значение
echo для логина и имени; пароль нужно ввести обязательно.
echo.
set "U="
set /p "U=Логин суперпользователя (Enter - выбрать из списка): "
if defined U (
    call :run_interactive update-admin --username "%U%"
) else (
    call :run_interactive update-admin
)
if errorlevel 1 (echo ОШИБКА изменения.) else (echo Данные обновлены.)
pause
goto menu

:delete_user
echo.
echo Удаление учётной записи. Действие необратимо:
echo сессии и связанные данные пользователя будут удалены.
set "U="
set /p "U=Логин удаляемой учётной записи: "
if not defined U goto menu
echo.
set "C="
set /p "C=Для подтверждения введите логин ещё раз: "
if /i not "%C%"=="%U%" (
    echo Удаление отменено: логины не совпадают.
    pause
    goto menu
)
call :run_interactive user delete --username "%U%" --yes
if errorlevel 1 (echo ОШИБКА удаления.) else (echo Пользователь удалён.)
pause
goto menu

:run_captured
echo [%DATE% %TIME%] "%EXE%" %* >> "%LOG%"
"%EXE%" %* > "%TEMP%\nexus-users.tmp" 2>&1
set "RUN_CODE=%ERRORLEVEL%"
type "%TEMP%\nexus-users.tmp"
type "%TEMP%\nexus-users.tmp" >> "%LOG%"
echo [%DATE% %TIME%] exit=%RUN_CODE% >> "%LOG%"
exit /b %RUN_CODE%

:run_interactive
echo [%DATE% %TIME%] "%EXE%" %* >> "%LOG%"
"%EXE%" %*
set "RUN_CODE=%ERRORLEVEL%"
echo [%DATE% %TIME%] exit=%RUN_CODE% >> "%LOG%"
exit /b %RUN_CODE%

:check_user_support
"%EXE%" user list --help >nul 2>&1
if errorlevel 1 exit /b 1
exit /b 0

:find_exe
set "EXE="
set "WORKDIR=%ROOT%"
if /i "%PREFER%"=="installed" goto find_installed
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
:find_installed
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

:no_exe
echo.
echo ОШИБКА: nexus.exe не найден.
echo Соберите backend или распакуйте release-архив.
set "RC=1"
goto finish

:old_exe
echo.
echo ОШИБКА: этот nexus.exe не поддерживает команду "user".
echo Он старее текущего исходного кода.
echo Используйте сборку backend\target\release\nexus.exe
echo или обновите установленную службу.
set "RC=1"
goto finish

:banner
echo ============================================================
echo   NEXUS WORKSPACE - учётные записи
echo ============================================================
exit /b 0

:finish
echo.
if not "%NO_PAUSE%"=="1" pause
exit /b %RC%
