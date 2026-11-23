# NEXUS WORKSPACE - проверка остановки службы Windows и восстановления C2.
#
# Что проверяется:
#   1. Stop-Service отправляет SERVICE_CONTROL_STOP (не Ctrl+C); служба
#      завершается graceful и укладывается в WaitToKillServiceTimeout.
#   2. Незавершённая running-задача не теряется: после Stop-Service она
#      остаётся running с cancel_requested, а следующий старт возвращает её в
#      очередь (в error — причина «прервана перезапуском»).
#   3. Жёсткий kill процесса (Stop-Process -Force) имитирует убийство службы
#      по таймауту SCM; следующий старт так же восстанавливает задачи.
#
# Требования: администратор, PostgreSQL, собранный release-бинарник, psql.
#
# Запуск:
#   powershell -ExecutionPolicy Bypass -File tests\service-stop.ps1

[CmdletBinding()]
param(
    [string]$ServerExe = '',
    [string]$ServiceName = ('NexusStopTest' + (Get-Date -Format 'HHmmss')),
    [int]$Port = 18081,
    [string]$DatabaseUrl = '',
    [string]$PsqlPath = '',
    [string]$TempRoot = (Join-Path $env:TEMP 'nexus-stop-test'),
    [switch]$KeepArtifacts
)

$ErrorActionPreference = 'Stop'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Запустите скрипт от имени администратора.'
}

$repo = Split-Path -Parent $PSScriptRoot
if (-not $ServerExe) {
    $ServerExe = Join-Path $repo 'backend\target\release\nexus.exe'
}
$created = $false
$jobIds = New-Object System.Collections.Generic.List[string]
$failures = New-Object System.Collections.Generic.List[string]
$result = [ordered]@{
    service = $ServiceName
    port = $Port
    scmTimeoutMs = $null
    gracefulStopMs = $null
    gracefulStopped = $false
    gracefulCancelRequested = $false
    gracefulRecovered = $false
    hardKillStopped = $false
    hardKillRecovered = $false
    orphanRunning = $null
    log = $null
}

function Write-Step([string]$text) {
    Write-Host ''
    Write-Host "==> $text" -ForegroundColor Cyan
}

function Resolve-Psql {
    if ($PsqlPath -and (Test-Path $PsqlPath)) { return $PsqlPath }
    $command = Get-Command psql -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $found = Get-ChildItem 'C:\Program Files\PostgreSQL\*\bin\psql.exe' -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($found) { return $found.FullName }
    throw 'psql.exe не найден: укажите -PsqlPath.'
}

function Invoke-Psql([string]$sql) {
    $output = & $script:psql $script:dbUrl -v ON_ERROR_STOP=1 -q -t -A -c $sql
    if ($LASTEXITCODE -ne 0) { throw "psql завершился с кодом $LASTEXITCODE" }
    return ($output | Out-String).Trim()
}

function New-RunningJob([string]$jobType) {
    # jsonb_build_object вместо строки с кавычками: PowerShell 5.1 иначе
    # «съедает» двойные кавычки при передаче аргумента в psql.exe.
    $id = Invoke-Psql ("insert into background_jobs (job_type, status, payload, cancellable, started_at) " +
        "values ('$jobType', 'running', jsonb_build_object('kind', '$jobType'), true, now()) returning id;")
    $jobIds.Add($id)
    return $id
}

function Get-JobState([string]$id) {
    return Invoke-Psql ("select status || '|' || coalesce(error, '') || '|' || cancel_requested::text " +
        "from background_jobs where id = '$id';")
}

function Wait-ServiceState([string]$state, [int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        $current = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
        if (-not $current) { return }
        if ([string]$current.Status -eq $state) { return }
        Start-Sleep -Milliseconds 100
    }
}

function Wait-Health([int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    $url = "http://127.0.0.1:$Port/api/v1/health"
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 3
            if ($response.StatusCode -eq 200) { return }
        } catch { }
        Start-Sleep -Milliseconds 200
    }
    throw "health не ответил за $seconds с"
}

function Get-LogText {
    $file = Get-ChildItem (Join-Path $TempRoot 'logs') -Filter '*.log*' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $file) { return '' }
    return (Get-Content -LiteralPath $file.FullName -Encoding UTF8 -Raw)
}

try {
    Write-Step 'Подготовка временной установки'
    if (-not (Test-Path $ServerExe)) { throw "Не найден $ServerExe (соберите cargo build --release)." }
    if (Test-Path $TempRoot) { Remove-Item -LiteralPath $TempRoot -Recurse -Force }
    foreach ($dir in @($TempRoot, (Join-Path $TempRoot 'logs'), (Join-Path $TempRoot 'data'), (Join-Path $TempRoot 'frontend'))) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    Copy-Item $ServerExe (Join-Path $TempRoot 'nexus.exe') -Force
    $dist = Join-Path $repo 'frontend\dist'
    if (Test-Path $dist) { Copy-Item (Join-Path $dist '*') (Join-Path $TempRoot 'frontend') -Recurse -Force }

    $envMap = @{}
    $envFile = Join-Path $repo '.env'
    if (Test-Path $envFile) {
        foreach ($line in Get-Content -LiteralPath $envFile) {
            if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
                $envMap[$Matches[1]] = $Matches[2].Trim('"', "'")
            }
        }
    }
    $script:dbUrl = if ($DatabaseUrl) { $DatabaseUrl } elseif ($envMap['DATABASE_URL']) { $envMap['DATABASE_URL'] } else { $env:DATABASE_URL }
    if (-not $script:dbUrl) { throw 'DATABASE_URL не задан: передайте -DatabaseUrl или заполните .env.' }
    if (-not $envMap['NEXUS_SECRET']) { throw 'NEXUS_SECRET не найден в .env.' }
    $script:psql = Resolve-Psql

    $static = (Join-Path $TempRoot 'frontend') -replace '\\', '/'
    $logs = (Join-Path $TempRoot 'logs') -replace '\\', '/'
    $lines = @(
        "DATABASE_URL=$script:dbUrl",
        "NEXUS_SECRET=$($envMap['NEXUS_SECRET'])",
        'NEXUS_HOST=127.0.0.1',
        "NEXUS_PORT=$Port",
        "NEXUS_PUBLIC_URL=http://127.0.0.1:$Port",
        'NEXUS_COOKIE_SECURE=false',
        "NEXUS_STATIC_DIR=$static",
        'NEXUS_TRUST_PROXY=false',
        "NEXUS_LOG_DIR=$logs",
        'NEXUS_LOG_FILE=nexus.log',
        'NEXUS_LOG_KEEP=7',
        'RUST_LOG=nexus=info,tower_http=warn,sqlx=warn'
    )
    [IO.File]::WriteAllLines((Join-Path $TempRoot '.env'), $lines, (New-Object Text.UTF8Encoding($false)))

    $scm = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control' -Name WaitToKillServiceTimeout -ErrorAction SilentlyContinue
    $result.scmTimeoutMs = if ($scm -and $scm.WaitToKillServiceTimeout) { [int]$scm.WaitToKillServiceTimeout } else { 5000 }

    Write-Step "Регистрация службы $ServiceName"
    $binPath = ('"{0}" serve --service' -f (Join-Path $TempRoot 'nexus.exe'))
    & sc.exe create $ServiceName binPath= $binPath start= demand DisplayName= 'NEXUS Stop Test' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc create: exit $LASTEXITCODE" }
    $created = $true
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
    New-ItemProperty -Path $key -Name Environment -PropertyType MultiString -Force -Value @(
        "NEXUS_SERVICE_NAME=$ServiceName",
        "NEXUS_SERVICE_STOP_TIMEOUT_MS=$($result.scmTimeoutMs)"
    ) | Out-Null

    Write-Step 'Запуск службы'
    Start-Service -Name $ServiceName
    Wait-ServiceState 'Running' 30
    Wait-Health 30
    $result.log = (Get-LogText)

    # --- 1. Graceful Stop-Service с running-задачей -------------------------
    Write-Step 'Stop-Service с running-задачей (graceful, C2)'
    $jobA = New-RunningJob 'system.metrics.prune'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Stop-Service -Name $ServiceName -Force -ErrorAction Stop
    Wait-ServiceState 'Stopped' 40
    $result.gracefulStopMs = $watch.ElapsedMilliseconds
    $stateA = Get-JobState $jobA
    $parts = $stateA -split '\|'
    $result.gracefulStopped = ((Get-Service -Name $ServiceName).Status -eq 'Stopped')
    $result.gracefulCancelRequested = ($parts[0] -eq 'running' -and $parts[2] -eq 'true')
    if (-not $result.gracefulStopped) { $failures.Add('служба не перешла в Stopped') }
    if (-not $result.gracefulCancelRequested) {
        $failures.Add("после graceful stop ожидалось running|cancel_requested=true, получено: $stateA")
    }

    Write-Step 'Повторный старт: восстановление running → pending (C2)'
    $logBefore = (Get-LogText)
    Start-Service -Name $ServiceName
    Wait-ServiceState 'Running' 30
    Wait-Health 30
    $deadline = (Get-Date).AddSeconds(10)
    $stateA = Get-JobState $jobA
    while ((Get-Date) -lt $deadline -and $stateA -notmatch 'перезапуск') {
        Start-Sleep -Milliseconds 100
        $stateA = Get-JobState $jobA
    }
    $logAfter = (Get-LogText)
    # Текст из psql может приходить в другой кодировке консоли, поэтому
    # проверяем структуру (не running и error непустой) + запись в журнале.
    $result.gracefulRecovered = ($stateA -notmatch '^running\|') -and
        ($stateA -notmatch '\|\|') -and
        ($logAfter -match 'возвращены в очередь')
    if (-not $result.gracefulRecovered) {
        $failures.Add("graceful C2 не подтверждён: state=$stateA, log recovery=$($logAfter -match 'возвращены в очередь')")
    }

    # --- 2. Жёсткий kill процесса (аналог убийства по таймауту SCM) ---------
    Write-Step 'Stop-Process -Force: восстановление после жёсткого падения'
    $jobB = New-RunningJob 'system.metrics.prune'
    $serviceInfo = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
    if (-not $serviceInfo.ProcessId) { throw 'не удалось получить PID службы' }
    Stop-Process -Id $serviceInfo.ProcessId -Force -ErrorAction Stop
    Wait-ServiceState 'Stopped' 20
    $result.hardKillStopped = ((Get-Service -Name $ServiceName -ErrorAction SilentlyContinue).Status -eq 'Stopped')

    $logBefore = (Get-LogText)
    Start-Service -Name $ServiceName
    Wait-ServiceState 'Running' 30
    Wait-Health 30
    Start-Sleep -Seconds 2
    $logAfter = (Get-LogText)
    $newRecovery = ([regex]::Matches($logAfter, 'возвращены в очередь').Count -
        [regex]::Matches($logBefore, 'возвращены в очередь').Count)
    $stateB = Get-JobState $jobB
    $result.hardKillRecovered = ($newRecovery -ge 1) -and ($stateB -notmatch '^running\|')
    if (-not $result.hardKillStopped) { $failures.Add('служба не остановилась после Stop-Process -Force') }
    if (-not $result.hardKillRecovered) {
        $failures.Add("hard kill C2 не подтверждён: новые recovery=$newRecovery, state=$stateB")
    }

    $result.orphanRunning = [int](Invoke-Psql "select count(*) from background_jobs where status = 'running';")
    if ($result.orphanRunning -ne 0) {
        $failures.Add("после старта осталось running-задач: $($result.orphanRunning)")
    }

    Write-Step 'Результат'
    $result.GetEnumerator() | ForEach-Object {
        Write-Host ("  {0}: {1}" -f $_.Key, $_.Value)
    }
    Write-Host ''
    if ($result.gracefulStopMs -gt $result.scmTimeoutMs) {
        Write-Host ("  внимание: Stop-Service {0} мс > WaitToKillServiceTimeout {1} мс — SCM мог убить процесс; C2 обязателен." -f `
            $result.gracefulStopMs, $result.scmTimeoutMs) -ForegroundColor Yellow
    } else {
        Write-Host ("  Stop-Service уложился в таймаут SCM ({0} <= {1} мс)." -f `
            $result.gracefulStopMs, $result.scmTimeoutMs) -ForegroundColor Green
    }
} catch {
    $failures.Add($_.Exception.Message)
    Write-Host ('ОШИБКА: ' + $_.Exception.Message) -ForegroundColor Red
} finally {
    Write-Step 'Очистка'
    foreach ($id in $jobIds) {
        try { [void](Invoke-Psql "delete from background_jobs where id = '$id';") } catch { }
    }
    if ($created) {
        try { Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue } catch { }
        Start-Sleep -Milliseconds 500
        & sc.exe delete $ServiceName | Out-Null
    }
    if (-not $KeepArtifacts -and (Test-Path $TempRoot)) {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($failures.Count -gt 0) {
    Write-Host ''
    $failures | ForEach-Object { Write-Host ('FAIL: ' + $_) -ForegroundColor Red }
    exit 1
}
Write-Host ''
Write-Host 'PASS: Stop-Service и восстановление C2 подтверждены.' -ForegroundColor Green
exit 0
