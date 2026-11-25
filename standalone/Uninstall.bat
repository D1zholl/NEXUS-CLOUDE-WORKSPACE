@echo off
rem NEXUS WORKSPACE - remove PROGRAM FILES only.
rem User data is never deleted automatically:
rem   config\   database password and NEXUS secret
rem   data\     PostgreSQL cluster
rem   storage\  user files
rem   backups\  backups
rem   logs\     journals
rem Delete the whole folder manually only if you really want to erase the data.
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"

echo ============================================================
echo   NEXUS WORKSPACE - uninstall program files
echo ============================================================
echo.
echo This will stop NEXUS and remove application binaries:
echo   NEXUS.exe, backend\, frontend\, database\, runtime\, docs\,
echo   LICENSES\, tools\, README files and manifest
echo.
echo It will KEEP:
echo   config\ (all configuration and secrets), data\, storage\,
echo   logs\, backups\, cache
echo.
echo To erase everything, stop NEXUS and delete the whole folder yourself.
echo.

if /i "%~1"=="--yes" goto uninstall
choice /C YN /N /M "Continue? [Y/N]: "
if errorlevel 2 goto cancelled

:uninstall
if exist "%~dp0NEXUS.exe" (
    "%~dp0NEXUS.exe" --stop --no-pause
)

for %%D in (backend frontend database runtime docs LICENSES tools app) do (
    if exist "%~dp0%%D" rmdir /s /q "%~dp0%%D"
)
for %%F in (NEXUS.exe Start.bat Stop.bat Restart.bat Status.bat First-Run.bat Diagnostics.bat Backup.bat Restore.bat Uninstall.bat README-FIRST-RUN.txt README.txt VERSION manifest.json SHA256SUMS checksums.sha256) do (
    if exist "%~dp0%%F" del /f /q "%~dp0%%F"
)
echo.
echo Program files removed. User data was preserved.
echo Delete this folder manually to remove data, configuration and backups.
if /i not "%~1"=="--yes" pause
exit /b 0

:cancelled
echo Cancelled.
exit /b 1
