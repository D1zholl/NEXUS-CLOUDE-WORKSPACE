@echo off
rem NEXUS WORKSPACE - restore database and storage from a backup folder.
rem Usage: Restore.bat "backups\2026-11-24-120000"
rem config\secrets.json of the current installation is preserved.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if "%~1"=="" (
    echo.
    echo Usage: Restore.bat "backups\YYYY-MM-DD-HHMMSS"
    echo.
    echo Available backups:
    if exist "%~dp0backups" dir /b /ad "%~dp0backups"
    echo.
    pause
    exit /b 2
)

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    pause
    exit /b 1
)

"%~dp0NEXUS.exe" --restore "%~1"
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
    echo.
    echo Restore failed. The current data was not intentionally deleted.
    echo See logs\launcher.log and logs\backend.log.
)
pause
exit /b %RC%
