# NEXUS WORKSPACE - обновление (Windows) с откатом.
#
# 1. Останавливает службу.
# 2. Снимает backup.ps1 при остановленной службе (дамп + конфигурация).
# 3. Собирает новую версию (если не -SkipBuild), сохраняет nexus.previous.exe.
# 4. Применяет миграции. При ошибке: восстанавливает БД из дампа, возвращает
#    nexus.previous.exe и запускает службу — установка остаётся рабочей.
#
# Имитация неудачной миграции (проверка отката):
#   deployment\update-windows.ps1 -SkipBuild -SimulateMigrationFailure
#
# Запуск от имени администратора.

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$BackupDir = 'C:\NexusBackups',
    [string]$PgBin = '',
    [switch]$SkipBackup,
    [switch]$SkipBuild,
    [switch]$SimulateMigrationFailure,
    [string]$TestUsername = '',
    [string]$TestPasswordEnv = 'NEXUS_TEST_PASSWORD',
    [string]$ResultPath = ''
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'nexus-common.ps1')

Assert-NexusAdmin
$repo = Split-Path -Parent $PSScriptRoot
$exeTarget = Join-Path $InstallDir 'nexus.exe'
$exePrevious = Join-Path $InstallDir 'nexus.previous.exe'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupPath = Join-Path $BackupDir $stamp
if (-not $ResultPath) { $ResultPath = Join-Path $repo ("docs\reports\logs\update-{0}.json" -f $stamp) }

$result = [ordered]@{
    startedAt  = (Get-Date).ToString('o')
    installDir = $InstallDir
    service    = $ServiceName
    backupPath = $backupPath
    simulateMigrationFailure = [bool]$SimulateMigrationFailure
}
$serviceStopped = $false
$pgPassFile = $null
$migrationLog = New-Object System.Collections.Generic.List[string]

Write-NexusStep "Обновление NEXUS WORKSPACE"

try {
    # 1. Остановка службы до любого копирования.
    Write-NexusStep "Остановка службы $ServiceName"
    $serviceStopped = Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
    if ($serviceStopped) { Write-Host "    остановлена" } else { Write-Host "    уже остановлена" }

    # 2. Резервная копия при остановленной службе.
    if (-not $SkipBackup) {
        Write-NexusStep "Резервная копия"
        & (Join-Path $PSScriptRoot 'backup.ps1') -BackupDir $BackupDir -InstallDir $InstallDir `
            -ServiceName $ServiceName -PgBin $PgBin -Stamp $stamp -Label 'pre-update' `
            -SkipServiceStop -SkipRestart
        if (-not (Test-Path -LiteralPath (Join-Path $backupPath 'database.dump'))) {
            throw "Резервная копия не создана: $backupPath"
        }
        $result.backupReady = $true
    } else {
        $result.backupReady = $false
    }

    # 3. Сборка (или готовая сборка из репозитория).
    if (-not $SkipBuild) {
        Write-NexusStep "Сборка backend (release)"
        $env:PATH = "C:\Program Files\nodejs;" + $env:PATH
        $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
        if (-not ($env:PATH -like "*$cargoBin*")) { $env:PATH = "$cargoBin;" + $env:PATH }
        Push-Location (Join-Path $repo 'backend')
        try { cargo build --release } finally { Pop-Location }

        Write-NexusStep "Сборка frontend"
        Push-Location (Join-Path $repo 'frontend')
        try {
            if (-not (Test-Path 'node_modules')) { npm install }
            npm run build
        } finally { Pop-Location }
    }
    $exeSource = Join-Path $repo 'backend\target\release\nexus.exe'
    if (-not (Test-Path -LiteralPath $exeSource)) { throw "Нет сборки: $exeSource" }

    # 4. Замена файлов; предыдущий бинарник сохраняется для отката.
    Write-NexusStep "Замена файлов"
    if (Test-Path -LiteralPath $exeTarget) { Copy-Item -LiteralPath $exeTarget -Destination $exePrevious -Force }
    Copy-Item -LiteralPath $exeSource -Destination $exeTarget -Force
    Write-Host "    nexus.exe заменён; предыдущий: nexus.previous.exe"
    $result.previousExe = $exePrevious

    $frontendSource = Join-Path $repo 'frontend\dist'
    if (Test-Path -LiteralPath $frontendSource) {
        $frontendTarget = Join-Path $InstallDir 'frontend'
        New-Item -ItemType Directory -Force -Path $frontendTarget | Out-Null
        & robocopy $frontendSource $frontendTarget /MIR /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "robocopy frontend завершился с кодом $LASTEXITCODE." }
        Write-Host "    frontend обновлён"
    }

    # 5. Окружение и (опционально) имитация сбоя миграции.
    $envPath = Join-Path $InstallDir '.env'
    $config = Get-NexusEnvConfig -Path $envPath
    foreach ($key in $config.Keys) { Set-Item -Path ("Env:" + $key) -Value $config[$key] }
    $env:NEXUS_CONFIG = Join-Path $InstallDir 'nexus.toml'
    $database = ConvertFrom-NexusDatabaseUrl -DatabaseUrl $config['DATABASE_URL']
    $result.database = $database.Database

    if ($SimulateMigrationFailure) {
        Write-NexusStep "Имитация неудачной миграции: повреждение checksum в _sqlx_migrations"
        $pgPassFile = New-NexusPgPassFile -Database $database
        $psql = Join-Path (Resolve-NexusPgBin -PgBin $PgBin) 'psql.exe'
        $sql = "update _sqlx_migrations set checksum = decode(repeat('00', 48), 'hex') where version = (select min(version) from _sqlx_migrations);"
        & $psql --host $database.Host --port $database.Port --username $database.User --dbname $database.Database --set ON_ERROR_STOP=on --command $sql
        if ($LASTEXITCODE -ne 0) { throw "Не удалось подготовить имитацию сбоя ($LASTEXITCODE)." }
        Remove-NexusPgPassFile -Path $pgPassFile
        $pgPassFile = $null
        Write-Host "    checksum первой миграции изменён (будет восстановлен откатом)" -ForegroundColor Yellow
    }

    # 6. Миграции и проверка. Пишем полный вывод в журнал прогона.
    Write-NexusStep "Миграции"
    $migrateFailed = $false
    $migrateResult = Invoke-NexusProcess -ExePath $exeTarget -Arguments @('migrate') -WorkingDirectory $InstallDir
    $migrateExit = $migrateResult.Code
    $migrationText = (($migrateResult.StdOut, $migrateResult.StdErr) -join "`n")
    foreach ($line in ($migrationText -split "`r?`n")) {
        if ($line.Trim()) { $migrationLog.Add($line); Write-Host "    $line" }
    }
    $result.migrateExit = $migrateExit
    if ($migrateExit -ne 0) { $migrateFailed = $true }

    if (-not $migrateFailed) {
        $checkResult = Invoke-NexusProcess -ExePath $exeTarget -Arguments @('check') -WorkingDirectory $InstallDir
        foreach ($line in ((($checkResult.StdOut, $checkResult.StdErr) -join "`n") -split "`r?`n")) {
            if ($line.Trim()) { Write-Host "    $line" }
        }
        $result.checkExit = $checkResult.Code
    }

    # 7. Откат при ошибке миграции.
    if ($migrateFailed) {
        Write-NexusStep "Миграции не применились - откат" 
        $result.rollback = $true

        if ($result.backupReady) {
            Write-NexusStep "Откат базы $($database.Database) из $backupPath"
            & (Join-Path $PSScriptRoot 'restore.ps1') -BackupPath $backupPath -InstallDir $InstallDir `
                -ServiceName $ServiceName -PgBin $PgBin -TargetDatabase $database.Database `
                -DropExisting -SkipData -SkipConfig -SkipServiceStop -SkipStart -Force
            $result.databaseRestored = $true
        } else {
            Write-Host "    резервная копия отключена (-SkipBackup): восстанавливаю только бинарник" -ForegroundColor Yellow
            $result.databaseRestored = $false
        }

        if (Test-Path -LiteralPath $exePrevious) {
            Copy-Item -LiteralPath $exePrevious -Destination $exeTarget -Force
            Write-Host "    возвращён nexus.previous.exe"
            $result.binaryRestored = $true
        } else {
            $result.binaryRestored = $false
        }

        $status = Start-NexusService -Name $ServiceName
        $health = Wait-NexusHttp -Port 8080 -TimeoutSeconds 30
        $result.serviceAfterRollback = [string]$status
        $result.healthAfterRollback = $health
        if ($health -ne 200) { throw "Откат выполнен, но health=$health (ожидался 200)." }

        $checkAfter = Invoke-NexusProcess -ExePath $exeTarget -Arguments @('check') -WorkingDirectory $InstallDir
        foreach ($line in ((($checkAfter.StdOut, $checkAfter.StdErr) -join "`n") -split "`r?`n")) {
            if ($line.Trim()) { Write-Host "    $line" }
        }
        $result.checkAfterRollback = $checkAfter.Code
        Write-Host "    служба снова Running, health=200" -ForegroundColor Green
        throw ("Обновление отменено: миграция завершилась с кодом {0}; выполнен откат." -f $migrateExit)
    }

    # 8. Успешный запуск.
    Write-NexusStep "Запуск службы"
    $status = Start-NexusService -Name $ServiceName
    $health = Wait-NexusHttp -Port 8080 -TimeoutSeconds 30
    $result.serviceAfter = [string]$status
    $result.health = $health
    if ($health -ne 200) { throw "Служба не отвечает health=200 (получено $health)." }
    Write-Host "    $status, health=200" -ForegroundColor Green

    if ($TestUsername) {
        if (-not (Test-Path ("Env:" + $TestPasswordEnv))) {
            throw "Для -TestUsername задайте переменную $TestPasswordEnv."
        }
        $auth = Test-NexusLogin -BaseUrl 'https://localhost' -Username $TestUsername `
            -Password (Get-Item ("Env:" + $TestPasswordEnv)).Value
        $result.login = $auth
        Write-Host ("    login: {0}, /auth/me: {1}" -f $auth.LoginStatus, $auth.MeStatus)
    }

    $result.pass = $true
    Write-NexusStep "Обновление завершено"
} catch {
    $result.pass = $false
    $result.error = $_.Exception.Message
    Write-Host ("ОШИБКА: " + $_.Exception.Message) -ForegroundColor Red
    throw
} finally {
    if ($pgPassFile) { Remove-NexusPgPassFile -Path $pgPassFile }
    # После успешного обновления служба уже запущена. Если исключение возникло
    # до запуска — поднимаем её, чтобы установка не осталась выключенной.
    $current = Get-NexusServiceObject -Name $ServiceName
    if ($current -and $current.Status -ne 'Running') {
        try {
            $status = Start-NexusService -Name $ServiceName
            $result.serviceFinal = [string]$status
            Write-Host "    служба запущена: $status" -ForegroundColor Green
        } catch {
            Write-Host ("    служба не запущена: " + $_.Exception.Message) -ForegroundColor Red
        }
    }
    $result.finishedAt = (Get-Date).ToString('o')
    $result.migrateOutput = @($migrationLog)
    $dir = Split-Path -Parent $ResultPath
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
}
