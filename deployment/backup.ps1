# NEXUS WORKSPACE - резервное копирование (Windows).
#
# Служба останавливается на время дампа и копирования: «на живую» backup не
# снимается. Пароль PostgreSQL передаётся через временный PGPASSFILE, а не
# аргументом командной строки.
#
# Запуск от администратора:
#   powershell -ExecutionPolicy Bypass -File deployment\backup.ps1 -BackupDir D:\Backups
#
# Для update-windows.ps1 (служба уже остановлена):
#   ... -SkipServiceStop -SkipRestart -Stamp 20261120-120000

[CmdletBinding()]
param(
    [string]$BackupDir = 'C:\NexusBackups',
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$PgBin = '',
    [switch]$SkipData,
    [switch]$SkipServiceStop,
    [switch]$SkipRestart,
    [string]$Stamp = '',
    [string]$Label = ''
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'nexus-common.ps1')

if (-not $Stamp) { $Stamp = Get-Date -Format 'yyyyMMdd-HHmmss' }
$target = Join-Path $BackupDir $Stamp
$serviceStopped = $false
$pgPassFile = $null
$result = [ordered]@{
    createdAt     = (Get-Date).ToString('o')
    backupPath    = $target
    installDir    = $InstallDir
    service       = $ServiceName
    serviceStopped = $false
    label         = $Label
}

Write-NexusStep "Резервное копирование в $target"

try {
    $envPath = Join-Path $InstallDir '.env'
    $config = Get-NexusEnvConfig -Path $envPath
    if (-not $config['DATABASE_URL']) { throw "В .env нет DATABASE_URL." }
    $database = ConvertFrom-NexusDatabaseUrl -DatabaseUrl $config['DATABASE_URL']
    $result.database = $database.Database
    $result.host = $database.Host
    $result.port = $database.Port

    $dumpTool = Join-Path (Resolve-NexusPgBin -PgBin $PgBin) 'pg_dump.exe'
    if (-not (Test-Path -LiteralPath $dumpTool)) { throw "pg_dump.exe не найден: $dumpTool" }

    if (-not $SkipServiceStop) {
        Write-NexusStep "Остановка службы $ServiceName"
        $serviceStopped = Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
        $result.serviceStopped = $serviceStopped
        if (-not $serviceStopped) {
            $existing = Get-NexusServiceObject -Name $ServiceName
            if ($existing) { Write-Host "    служба уже остановлена" }
            else { Write-Host "    служба не зарегистрирована - продолжаю" -ForegroundColor Yellow }
        } else {
            Write-Host "    служба остановлена"
        }
    } else {
        $result.serviceStopped = $true
    }

    New-Item -ItemType Directory -Force -Path $target | Out-Null
    $pgPassFile = New-NexusPgPassFile -Database $database

    # 1. Дамп в custom-формате: pg_restore, произвольный порядок объектов.
    Write-NexusStep "Дамп базы $($database.Database) (custom format)"
    $dumpFile = Join-Path $target 'database.dump'
    $dumpArgs = @(
        '--host', $database.Host,
        '--port', $database.Port,
        '--username', $database.User,
        '--dbname', $database.Database,
        '--format=custom',
        '--compress=9',
        '--no-owner',
        '--no-privileges',
        '--lock-wait-timeout=30s',
        ('--file=' + $dumpFile)
    )
    & $dumpTool @dumpArgs
    if ($LASTEXITCODE -ne 0) { throw "pg_dump завершился с ошибкой ($LASTEXITCODE)." }
    $dump = Get-Item -LiteralPath $dumpFile
    if ($dump.Length -le 0) { throw "pg_dump создал пустой файл." }
    $dumpHash = (Get-FileHash -LiteralPath $dumpFile -Algorithm SHA256).Hash.ToLowerInvariant()
    $result.dumpFile = $dumpFile
    $result.dumpBytes = $dump.Length
    $result.dumpSha256 = $dumpHash
    $result.pgDumpVersion = (& $dumpTool --version) -join ' '
    Write-Host ("    {0} байт, SHA-256 {1}" -f $dump.Length, $dumpHash)

    # 2. Каталог данных (только при остановленной службе).
    if (-not $SkipData) {
        Write-NexusStep "Копирование каталога данных"
        $dataDir = Join-Path $InstallDir 'data'
        if (Test-Path -LiteralPath $dataDir) {
            $dataTarget = Join-Path $target 'data'
            New-Item -ItemType Directory -Force -Path $dataTarget | Out-Null
            $rc = 0
            & robocopy $dataDir $dataTarget /E /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
            $rc = $LASTEXITCODE
            if ($rc -ge 8) { throw "robocopy завершился с кодом $rc." }
            $bytes = (Get-ChildItem -LiteralPath $dataTarget -Recurse -File | Measure-Object Length -Sum).Sum
            $result.dataIncluded = $true
            $result.dataBytes = [int64]$bytes
            Write-Host ("    скопировано {0:N0} байт из $dataDir" -f $bytes)
        } else {
            Write-Host "    каталог $dataDir отсутствует - пропускаю" -ForegroundColor Yellow
            $result.dataIncluded = $false
        }
        Write-Host "    каталоги хранения вне $dataDir копируйте отдельно (Robocopy)." -ForegroundColor Yellow
    } else {
        $result.dataIncluded = $false
    }

    # 3. Конфигурация и версия приложения.
    Write-NexusStep "Копирование конфигурации"
    foreach ($name in @('.env', 'nexus.toml')) {
        $source = Join-Path $InstallDir $name
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination $target -Force
            Write-Host "    $name"
        }
    }
    $exe = Join-Path $InstallDir 'nexus.exe'
    if (Test-Path -LiteralPath $exe) {
        $versionText = (& $exe --version 2>&1) -join ' '
        $result.appVersion = $versionText.Trim()
    }

    # 4. Манифест с контрольными суммами.
    $manifest = [ordered]@{
        format         = 'nexus-backup/2'
        createdAt      = $result.createdAt
        label          = $Label
        appVersion     = $result.appVersion
        database       = [ordered]@{
            name  = $database.Database
            host  = $database.Host
            port  = $database.Port
            user  = $database.User
            dump  = 'database.dump'
            bytes = $dump.Length
            sha256 = $dumpHash
        }
        pgDumpVersion  = $result.pgDumpVersion
        serviceStopped = $result.serviceStopped
        data           = [ordered]@{
            included = [bool]$result.dataIncluded
            bytes    = $result.dataBytes
        }
        files          = @('database.dump', '.env', 'nexus.toml')
    }
    $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $target 'manifest.json') -Encoding UTF8

    # 5. Контрольная сумма всего каталога (для переноса на другой носитель).
    $checksumLines = Get-ChildItem -LiteralPath $target -Recurse -File |
        Where-Object { $_.Name -ne 'SHA256SUMS' } |
        ForEach-Object {
            $relative = $_.FullName.Substring($target.Length).TrimStart('\').Replace('\', '/')
            '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $relative
        }
    $checksumLines | Set-Content -LiteralPath (Join-Path $target 'SHA256SUMS') -Encoding ASCII

    $result.pass = $true
    Write-NexusStep "Готово: $target"
    Write-Host "  Дамп:    database.dump ($($dump.Length) байт)"
    Write-Host "  Манифест: manifest.json, SHA256SUMS"
    Write-Host "  Восстановление: deployment\restore.ps1 -BackupPath $target -TargetDatabase nexus_restore -SwitchInstall"
} catch {
    $result.pass = $false
    $result.error = $_.Exception.Message
    Write-Host ("ОШИБКА: " + $_.Exception.Message) -ForegroundColor Red
    throw
} finally {
    if ($pgPassFile) { Remove-NexusPgPassFile -Path $pgPassFile }
    if ($serviceStopped -and -not $SkipRestart) {
        Write-NexusStep "Запуск службы $ServiceName"
        try {
            $status = Start-NexusService -Name $ServiceName
            $result.serviceAfter = [string]$status
            Write-Host "    $status" -ForegroundColor Green
        } catch {
            Write-Host ("    не удалось запустить службу: " + $_.Exception.Message) -ForegroundColor Red
        }
    }
    $result.finishedAt = (Get-Date).ToString('o')
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $target 'backup-result.json') -Encoding UTF8 -ErrorAction SilentlyContinue
}
