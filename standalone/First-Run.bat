@echo off
rem NEXUS WORKSPACE - first run / initial setup.
rem The launcher creates the database, applies migrations and opens the
rem administrator console wizard when no administrator exists yet.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    echo Keep all release files together.
    pause
    exit /b 1
)

start "" "%~dp0NEXUS.exe" --first-run
exit /b 0
