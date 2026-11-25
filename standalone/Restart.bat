@echo off
rem NEXUS WORKSPACE - stop and start again.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    if /i not "%~1"=="--no-pause" pause
    exit /b 1
)

"%~dp0NEXUS.exe" --restart
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" echo Restart request failed. See logs\launcher.log.
if /i not "%~1"=="--no-pause" pause
exit /b %RC%
