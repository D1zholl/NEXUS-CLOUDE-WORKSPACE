@echo off
rem NEXUS WORKSPACE - start the portable launcher.
rem No administrator rights and no terminal commands are required.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

if not exist "%~dp0NEXUS.exe" (
    echo.
    echo ERROR: NEXUS.exe was not found next to this file.
    echo Keep the release files together and run Start.bat from the release folder.
    echo.
    pause
    exit /b 1
)

start "" "%~dp0NEXUS.exe" --gui
exit /b 0
