@echo off
rem NEXUS WORKSPACE - create a backup (database dump + storage + config).
rem If NEXUS is running, the backend is stopped for a consistent copy and
rem started again afterwards.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    pause
    exit /b 1
)

"%~dp0NEXUS.exe" --backup
set "RC=%ERRORLEVEL%"
echo.
echo Backups are stored in the backups folder.
pause
exit /b %RC%
