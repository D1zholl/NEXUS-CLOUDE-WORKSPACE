@echo off
rem NEXUS WORKSPACE - graceful stop of backend and portable PostgreSQL.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    if /i not "%~1"=="--no-pause" pause
    exit /b 1
)

"%~dp0NEXUS.exe" --stop
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
    echo.
    echo NEXUS could not be stopped cleanly. See logs\launcher.log and logs\backend.log.
)
if /i not "%~1"=="--no-pause" pause
exit /b %RC%
