# NEXUS WORKSPACE - восстановление из резервной копии (Windows).
#
# Восстанавливает custom-дамп через pg_restore в указанную базу (по умолчанию —
# в исходную), при необходимости создаёт новую БД. Служба останавливается на
# время восстановления. Пароль PostgreSQL — только через PGPASSFILE.
#
# Примеры:
#   # В новую БД, затем переключить установку на неё:
#   deployment\restore.ps1 -BackupPath C:\NexusBackups\20261120-120000 `
#       -TargetDatabase nexus_restore -SwitchInstall
#   # Вернуть исходную БД:
#   deployment\restore.ps1 -BackupPath ... -TargetDatabase nexus -DropExisting

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BackupPath,
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$PgBin = '',
    [string]$TargetDatabase = '',
    [switch]$DropExisting,
    [switch]$SwitchInstall,
    [switch]$SkipData,
    [switch]$SkipConfig,
    [switch]$SkipServiceStop,
    [switch]$SkipStart,
    [switch]$Force,
    [string]$TestUsername = '',
    [string]$TestPasswordEnv = 'NEXUS_TEST_PASSWORD',
    [string]$ResultPath = ''
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'nexus-common.ps1')

if (-not (Test-Path -LiteralPath $BackupPath)) { throw "Каталог резервной копии не найден: $BackupPath" }
$manifestPath = Join-Path $BackupPath 'manifest.json'
$manifest = $null
if (Test-Path -LiteralPath $manifestPath) {
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
}

$dumpFile = Join-Path $BackupPath 'database.dump'
$legacyFile = $null
if (-not (Test-Path -LiteralPath $dumpFile)) {
    $legacyFile = Join-Path $BackupPath 'database.sql'
    if (-not (Test-Path -LiteralPath $legacyFile)) {
        throw "В копии нет database.dump (или legacy database.sql)."
    }
    $dumpFile = $legacyFile
}

if (-not $Force) {
    $answer = Read-Host "Восстановление заменит данные базы. Продолжить? (yes/no)"
    if ($answer -ne 'yes') { Write-Host 'Отменено.'; return }
}

$result = [ordered]@{
    startedAt  = (Get-Date).ToString('o')
    backupPath = $BackupPath
    installDir = $InstallDir
    targetDatabase = $TargetDatabase
    switchInstall = [bool]$SwitchInstall
}
$serviceStopped = $false
$pgPassFile = $null
$pgBinPath = Resolve-NexusPgBin -PgBin $PgBin
$pgRestore = Join-Path $pgBinPath 'pg_restore.exe'
$psql = Join-Path $pgBinPath 'psql.exe'

try {
    # 1. Параметры базы: приоритет у .env установки, иначе — из копии.
    $installEnv = Join-Path $InstallDir '.env'
    $backupEnv = Join-Path $BackupPath '.env'
    $envSource = if (Test-Path -LiteralPath $installEnv) { $installEnv } elseif (Test-Path -LiteralPath $backupEnv) { $backupEnv } else { throw "Не найден .env ни в установке, ни в копии." }
    $config = Get-NexusEnvConfig -Path $envSource
    $source = ConvertFrom-NexusDatabaseUrl -DatabaseUrl $config['DATABASE_URL']
    $targetName = if ($TargetDatabase) { $TargetDatabase } else { $source.Database }
    $database = [pscustomobject]@{
        User     = $source.User
        Password = $source.Password
        Host     = $source.Host
        Port     = $source.Port
        Database = $targetName
    }
    $result.sourceDatabase = $source.Database
    $result.targetDatabase = $database.Database
    $sameDatabase = ($database.Database -eq $source.Database)
    if ($sameDatabase -and -not $DropExisting) {
        throw "Восстановление в активную базу $($database.Database) требует -DropExisting (данные будут заменены)."
    }

    if (-not $SkipServiceStop) {
        Write-NexusStep "Остановка службы $ServiceName"
        $serviceStopped = Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
        Write-Host ("    " + $(if ($serviceStopped) { 'остановлена' } else { 'уже остановлена' }))
    }

    $pgPassFile = New-NexusPgPassFile -Database $database

    # 2. Проверка SHA-256 дампа по манифесту.
    $expectedHash = $null
    if ($manifest -and $manifest.database -and $manifest.database.sha256) { $expectedHash = $manifest.database.sha256 }
    $actualHash = (Get-FileHash -LiteralPath $dumpFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expectedHash -and $expectedHash -ne $actualHash) {
        throw "SHA-256 дампа не совпадает с манифестом: $actualHash != $expectedHash"
    }
    $result.dumpSha256 = $actualHash

    # 3. Создание целевой базы (или пересоздание активной).
    Write-NexusStep "Подготовка базы $($database.Database)"
    $quotedDb = '"' + ($database.Database -replace '"', '""') + '"'
    if (-not $sameDatabase -or $DropExisting) {
        $terminate = "select pg_terminate_backend(pid) from pg_stat_activity where datname = '$($database.Database -replace "'", "''")' and pid <> pg_backend_pid();"
        & $psql --host $database.Host --port $database.Port --username $database.User --dbname postgres --set ON_ERROR_STOP=on --command $terminate | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Не удалось завершить соединения с БД ($LASTEXITCODE)." }
        $dropSql = "drop database if exists $quotedDb with (force);"
        & $psql --host $database.Host --port $database.Port --username $database.User --dbname postgres --set ON_ERROR_STOP=on --command $dropSql | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Не удалось удалить БД $($database.Database) ($LASTEXITCODE)." }
        & (Join-Path $pgBinPath 'createdb.exe') --host $database.Host --port $database.Port --username $database.User --owner $database.User $database.Database
        if ($LASTEXITCODE -ne 0) { throw "Не удалось создать БД $($database.Database) ($LASTEXITCODE)." }
        Write-Host "    создана $($database.Database) (owner $($database.User))"
    } else {
        Write-Host "    восстанавливаю поверх существующей $($database.Database)"
    }

    # 4. Восстановление.
    Write-NexusStep "pg_restore в $($database.Database)"
    $header = New-Object byte[] 5
    $stream = [System.IO.File]::OpenRead($dumpFile)
    try { [void]$stream.Read($header, 0, 5) } finally { $stream.Dispose() }
    $isCustom = ([System.Text.Encoding]::ASCII.GetString($header) -eq 'PGDMP')
    if ($isCustom) {
        & $pgRestore --host $database.Host --port $database.Port --username $database.User --dbname $database.Database --no-owner --no-privileges --exit-on-error $dumpFile
        if ($LASTEXITCODE -ne 0) { throw "pg_restore завершился с ошибкой ($LASTEXITCODE)." }
        $result.restoreMethod = 'pg_restore'
    } else {
        & $psql --host $database.Host --port $database.Port --username $database.User --dbname $database.Database --set ON_ERROR_STOP=on --file $dumpFile
        if ($LASTEXITCODE -ne 0) { throw "psql (legacy dump) завершился с ошибкой ($LASTEXITCODE)." }
        $result.restoreMethod = 'psql (legacy plain dump)'
    }
    Write-Host "    готово ($($result.restoreMethod))"

    # 5. Каталог данных.
    if (-not $SkipData) {
        $dataSource = Join-Path $BackupPath 'data'
        if (Test-Path -LiteralPath $dataSource) {
            Write-NexusStep "Восстановление каталога данных"
            $dataTarget = Join-Path $InstallDir 'data'
            New-Item -ItemType Directory -Force -Path $dataTarget | Out-Null
            & robocopy $dataSource $dataTarget /E /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
            if ($LASTEXITCODE -ge 8) { throw "robocopy завершился с кодом $LASTEXITCODE." }
            $bytes = (Get-ChildItem -LiteralPath $dataTarget -Recurse -File | Measure-Object Length -Sum).Sum
            $result.dataBytes = [int64]$bytes
            Write-Host ("    скопировано {0:N0} байт в $dataTarget" -f $bytes)
            Write-Host "    Каталоги хранения вне $InstallDir\data восстановите отдельно." -ForegroundColor Yellow
        } else {
            Write-Host "    в копии нет каталога data - пропускаю" -ForegroundColor Yellow
        }
    }

    # 6. Конфигурация.
    if (-not $SkipConfig) {
        Write-NexusStep "Конфигурация"
        $installEnv = Join-Path $InstallDir '.env'
        if (-not (Test-Path -LiteralPath $installEnv) -and (Test-Path -LiteralPath $backupEnv)) {
            Copy-Item -LiteralPath $backupEnv -Destination $installEnv -Force
            Write-Host "    .env скопирован из копии"
        }
        $tomlSource = Join-Path $BackupPath 'nexus.toml'
        if (Test-Path -LiteralPath $tomlSource) {
            Copy-Item -LiteralPath $tomlSource -Destination (Join-Path $InstallDir 'nexus.toml') -Force
            Write-Host "    nexus.toml"
        }
        if ($SwitchInstall) {
            $text = [System.IO.File]::ReadAllText($installEnv, [System.Text.Encoding]::UTF8)
            $pattern = '(?m)^DATABASE_URL=.*$'
            $replacement = 'DATABASE_URL=' + ($config['DATABASE_URL'] -replace '^(postgres(?:ql)?://[^/]+/)[^?]+', ('$1' + $database.Database))
            if ($text -match $pattern) {
                $text = [regex]::Replace($text, $pattern, $replacement)
            } else {
                $text = $text.TrimEnd() + [Environment]::NewLine + $replacement + [Environment]::NewLine
            }
            [System.IO.File]::WriteAllText($installEnv, $text, (New-Object System.Text.UTF8Encoding($false)))
            Write-Host "    .env переключён на $($database.Database)"
        }
    }

    # 7. Проверка и запуск.
    Write-NexusStep "nexus check"
    $checkEnv = Get-NexusEnvConfig -Path (Join-Path $InstallDir '.env')
    foreach ($key in $checkEnv.Keys) { Set-Item -Path ("Env:" + $key) -Value $checkEnv[$key] }
    $env:NEXUS_CONFIG = Join-Path $InstallDir 'nexus.toml'
    Push-Location $InstallDir
    try {
        $checkResult = Invoke-NexusProcess -ExePath (Join-Path $InstallDir 'nexus.exe') -Arguments @('check') -WorkingDirectory $InstallDir
        foreach ($line in ((($checkResult.StdOut, $checkResult.StdErr) -join "`n") -split "`r?`n")) {
            if ($line.Trim()) { Write-Host "    $line" }
        }
        $result.checkExit = $checkResult.Code
    } finally { Pop-Location }

    if (-not $SkipStart) {
        Write-NexusStep "Запуск службы $ServiceName"
        $status = Start-NexusService -Name $ServiceName
        Write-Host "    $status" -ForegroundColor Green
        $health = Wait-NexusHttp -Port 8080 -TimeoutSeconds 30
        $result.healthStatus = $health
        if ($health -ne 200) { throw "Служба не отвечает health=200 (получено $health)." }

        if ($TestUsername) {
            if (-not (Test-Path ("Env:" + $TestPasswordEnv))) {
                throw "Для -TestUsername задайте переменную $TestPasswordEnv."
            }
            $testPassword = (Get-Item ("Env:" + $TestPasswordEnv)).Value
            $auth = Test-NexusLogin -BaseUrl 'https://localhost' -Username $TestUsername -Password $testPassword
            $result.login = $auth
            Write-Host ("    login: {0}, /auth/me: {1}" -f $auth.LoginStatus, $auth.MeStatus)
            if ($auth.LoginStatus -ne 200 -or $auth.MeStatus -ne 200) { throw "Вход после восстановления не работает." }
        }
    }

    $result.pass = $true
    Write-NexusStep "Восстановление завершено"
} catch {
    $result.pass = $false
    $result.error = $_.Exception.Message
    Write-Host ("ОШИБКА: " + $_.Exception.Message) -ForegroundColor Red
    throw
} finally {
    if ($pgPassFile) { Remove-NexusPgPassFile -Path $pgPassFile }
    if ($serviceStopped -and -not $SkipStart) {
        try {
            if ((Get-Service -Name $ServiceName).Status -ne 'Running') {
                $status = Start-NexusService -Name $ServiceName
                Write-Host "    служба: $status" -ForegroundColor Green
            }
        } catch {
            Write-Host ("    служба не запущена: " + $_.Exception.Message) -ForegroundColor Red
        }
    }
    $result.finishedAt = (Get-Date).ToString('o')
    if ($ResultPath) {
        $dir = Split-Path -Parent $ResultPath
        if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
    }
}
