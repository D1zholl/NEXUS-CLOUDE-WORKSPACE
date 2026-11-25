@echo off
rem NEXUS WORKSPACE - component status.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    if /i not "%~1"=="--no-pause" pause
    exit /b 1
)

"%~dp0NEXUS.exe" --status %*
set "RC=%ERRORLEVEL%"
if /i not "%~1"=="--no-pause" pause
exit /b %RC%
