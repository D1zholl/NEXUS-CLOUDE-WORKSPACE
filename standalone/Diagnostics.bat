@echo off
rem NEXUS WORKSPACE - diagnostics report.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo ERROR: NEXUS.exe was not found.
    pause
    exit /b 1
)

"%~dp0NEXUS.exe" --diagnostics
set "RC=%ERRORLEVEL%"
echo.
echo The report is saved to logs\diagnostics-latest.txt
pause
exit /b %RC%
