# NEXUS WORKSPACE - приёмка J4 после перезагрузки Windows.
#
# Фаза Prepare:
#   * устанавливает/обновляет службу NexusJ4 (LocalSystem, автозапуск,
#     зависимость от локального PostgreSQL);
#   * создаёт временного администратора, загружает файл 2 МиБ, публикует
#     публичную ссылку и проверяет SHA до перезагрузки;
#   * создаёт незавершённую chunked-загрузку (512 КиБ из 1 МиБ);
#   * оставляет running-задачу (имитация прерванной единицы работы);
#   * записывает состояние и регистрирует задачу SYSTEM «At startup»,
#     которая после перезагрузки запускает этот же скрипт с -Phase Verify;
#   * по умолчанию перезагружает хост (отключается -NoReboot).
#
# Фаза Verify (запускается автоматически после старта):
#   * проверяет автозапуск службы, StartType=Automatic, health;
#   * проверяет восстановление running-задачи по журналу и БД;
#   * скачивает файл по публичной ссылке и сверяет SHA;
#   * продолжает незавершённую загрузку и сверяет SHA;
#   * проверяет тома и отсутствие осиротевших running-задач;
#   * пишет JSON/Markdown-отчёт.
#
# Пример:
#   powershell -File tests\reboot-acceptance.ps1 -NoReboot
#   powershell -File tests\reboot-acceptance.ps1          # перезагрузка
# После перезагрузки отчёт: C:\ProgramData\NexusJ4\reboot-result.{json,md}

[CmdletBinding()]
param(
    [ValidateSet('Prepare', 'Verify')]
    [string]$Phase = 'Prepare',
    [string]$InstallDir = 'C:\ProgramData\NexusJ4',
    [string]$ServiceName = 'NexusJ4',
    [int]$Port = 8080,
    [string]$DatabaseUrl = '',
    [string]$Secret = '',
    [string]$StateFile = 'C:\ProgramData\NexusJ4\reboot-state.json',
    [string]$ResultFile = 'C:\ProgramData\NexusJ4\reboot-result.json',
    [switch]$NoReboot,
    [switch]$DryRun,
    [switch]$KeepService
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$base = "http://127.0.0.1:$Port"
$tempAdmin = 'j4reboot'
$taskName = 'NexusJ4RebootVerify'

function Write-Step([string]$text) {
    Write-Host ''
    Write-Host "==> $text" -ForegroundColor Cyan
}

function Resolve-Psql {
    $command = Get-Command psql -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $found = Get-ChildItem 'C:\Program Files\PostgreSQL\*\bin\psql.exe' -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($found) { return $found.FullName }
    throw 'psql.exe не найден.'
}

function Invoke-Psql([string]$sql, [string]$dbUrl) {
    $output = & (Resolve-Psql) $dbUrl -v ON_ERROR_STOP=1 -q -t -A -c $sql
    if ($LASTEXITCODE -ne 0) { throw "psql завершился с кодом $LASTEXITCODE" }
    return ($output | Out-String).Trim()
}

function Wait-Health([int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri "$base/api/v1/health" -UseBasicParsing -TimeoutSec 3
            if ($response.StatusCode -eq 200) { return ($response.Content | ConvertFrom-Json) }
        } catch { }
        Start-Sleep -Milliseconds 500
    }
    throw "health не ответил за $seconds с"
}

$script:CurrentAuth = $null

function Invoke-Api {
    param(
        [string]$Method,
        [string]$Path,
        $Body = $null,
        [hashtable]$Headers = @{},
        [byte[]]$Bytes = $null,
        [string]$OutFile = ''
    )
    $allHeaders = @{}
    if ($script:CurrentAuth) {
        $allHeaders['X-CSRF-Token'] = $script:CurrentAuth.csrf
    }
    foreach ($key in $Headers.Keys) { $allHeaders[$key] = $Headers[$key] }
    $params = @{
        Uri                = ($base + $Path)
        Method             = $Method
        UseBasicParsing    = $true
        TimeoutSec         = 120
        Headers            = $allHeaders
        MaximumRedirection = 5
    }
    if ($script:CurrentAuth) { $params.WebSession = $script:CurrentAuth.session }
    if ($OutFile) { $params.OutFile = $OutFile }
    if ($null -ne $Bytes) {
        $params.Body = $Bytes
        $params.ContentType = 'application/octet-stream'
    } elseif ($null -ne $Body) {
        $params.Body = ($Body | ConvertTo-Json -Depth 6)
        $params.ContentType = 'application/json'
    }
    return Invoke-WebRequest @params
}

function New-TestBytes([int]$size, [int]$seed) {
    $bytes = New-Object byte[] $size
    (New-Object Random $seed).NextBytes($bytes)
    return $bytes
}

function Get-ShaBytes([byte[]]$bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString('x2') }) -join '')
    } finally { $sha.Dispose() }
}

function Get-ShaFile([string]$path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
}

function Connect-Api([string]$username, [string]$password) {
    $login = Invoke-WebRequest -UseBasicParsing -Uri "$base/api/v1/auth/login" -Method POST `
        -ContentType 'application/json' -Body (@{ username = $username; password = $password } | ConvertTo-Json) `
        -SessionVariable ws -TimeoutSec 30
    $data = $login.Content | ConvertFrom-Json
    if (-not $data.success) { throw 'login не удался' }
    $sessionCookie = $ws.Cookies.GetCookies([uri]$base) | Where-Object { $_.Name -eq 'nexus_session' } | Select-Object -First 1
    if (-not $sessionCookie) { throw 'login не вернул cookie nexus_session' }
    return [pscustomobject]@{
        session  = $ws
        token    = $sessionCookie.Value
        csrf     = [string]$data.data.csrfToken
        username = $username
        password = $password
    }
}

function Get-AuthHeaders($auth, [hashtable]$extra = @{}) {
    # Cookie передаётся через WebSession в Invoke-Api; здесь только
    # дополнительные заголовки (Upload-Offset, Idempotency-Key).
    $headers = @{}
    foreach ($key in $extra.Keys) { $headers[$key] = $extra[$key] }
    return $headers
}

# ---------------------------------------------------------------------------
function Invoke-Prepare {
    Write-Step 'J4 Prepare: установка и подготовка данных'
    if (-not $DatabaseUrl -or -not $Secret) {
        $envFile = Join-Path $repo '.env'
        if (Test-Path $envFile) {
            foreach ($line in Get-Content -LiteralPath $envFile) {
                if ($line -match '^\s*DATABASE_URL\s*=\s*(.*)$' -and -not $DatabaseUrl) { $DatabaseUrl = $Matches[1].Trim('"', "'") }
                if ($line -match '^\s*NEXUS_SECRET\s*=\s*(.*)$' -and -not $Secret) { $Secret = $Matches[1].Trim('"', "'") }
            }
        }
    }
    if (-not $DatabaseUrl -or -not $Secret) { throw 'Нужны DATABASE_URL и NEXUS_SECRET (.env или параметры).' }

    if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    }

    $install = Join-Path $repo 'deployment\install-windows.ps1'
    & $install -InstallDir $InstallDir -ServiceName $ServiceName `
        -ServiceAccountType LocalSystem -SkipBuild -SkipAdmin `
        -DatabaseUrl $DatabaseUrl -Secret $Secret
    if ($LASTEXITCODE -ne 0) { throw 'install-windows.ps1 завершился с ошибкой' }

    Wait-Health 60 | Out-Null
    $dbUrl = (Get-Content (Join-Path $InstallDir '.env') |
        Where-Object { $_ -match '^DATABASE_URL=' } |
        Select-Object -First 1) -replace '^DATABASE_URL=', ''
    $dbUrl = $dbUrl.Trim('"', "'")
    $psql = Resolve-Psql

    # Чистый временный администратор.
    try { [void](Invoke-Psql "delete from users where username = '$tempAdmin';" $dbUrl) } catch { }
    $password = 'J4-' + [guid]::NewGuid().ToString('N').Substring(0, 20) + '!aA1'
    $env:NEXUS_J4_PASSWORD = $password
    Push-Location $InstallDir
    try {
        & (Join-Path $InstallDir 'nexus.exe') create-admin --username $tempAdmin --password-env NEXUS_J4_PASSWORD | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'create-admin завершился с ошибкой' }
    } finally {
        Pop-Location
        Remove-Item Env:NEXUS_J4_PASSWORD -ErrorAction SilentlyContinue
    }
    $auth = Connect-Api $tempAdmin $password
    $script:CurrentAuth = $auth
    Write-Host "    временный администратор: $tempAdmin"

    # 1. Завершённая загрузка + публичная ссылка.
    $fileBytes = New-TestBytes (2 * 1024 * 1024) 42
    $fileSha = Get-ShaBytes $fileBytes
    $begin = Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Headers (Get-AuthHeaders $auth) -Body @{
        name = 'j4-reboot.bin'; parentId = $null; size = $fileBytes.Length; contentType = 'application/octet-stream'
    }
    $sessionId = [string]($begin.Content | ConvertFrom-Json).data.id
    [void](Invoke-Api -Method PATCH -Path "/api/v1/cloud/uploads/$sessionId" `
            -Headers (Get-AuthHeaders $auth @{ 'Upload-Offset' = '0' }) -Bytes $fileBytes)
    $complete = Invoke-Api -Method POST -Path "/api/v1/cloud/uploads/$sessionId/complete" `
        -Headers (Get-AuthHeaders $auth @{ 'Idempotency-Key' = [guid]::NewGuid().ToString() }) -Body @{ strategy = 'auto' }
    $entryId = [string]($complete.Content | ConvertFrom-Json).data.id
    $share = Invoke-Api -Method POST -Path '/api/v1/cloud/shares' `
        -Headers (Get-AuthHeaders $auth @{ 'Idempotency-Key' = [guid]::NewGuid().ToString() }) `
        -Body @{ entryId = $entryId; expiresInHours = 2; maxDownloads = 20; authOnly = $false }
    $shareData = ($share.Content | ConvertFrom-Json).data
    $shareToken = [string]$shareData.token
    $probe = Join-Path $env:TEMP ('j4-probe-' + [guid]::NewGuid().ToString('N') + '.bin')
    Invoke-Api -Method GET -Path "/api/v1/public/shares/$shareToken/download" -OutFile $probe | Out-Null
    $probeSha = Get-ShaFile $probe
    Remove-Item $probe -Force
    if ($probeSha -ne $fileSha) { throw 'SHA публичной ссылки до перезагрузки не совпал' }
    Write-Host "    файл $entryId, SHA $($fileSha.Substring(0, 16))…, публичная ссылка проверена"

    # 2. Незавершённая chunked-загрузка: 512 КиБ из 1 МиБ.
    $partialBytes = New-TestBytes (1024 * 1024) 77
    $partialSha = Get-ShaBytes $partialBytes
    $partialSize = 512 * 1024
    $begin2 = Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Headers (Get-AuthHeaders $auth) -Body @{
        name = 'j4-resume.bin'; parentId = $null; size = $partialBytes.Length; contentType = 'application/octet-stream'
    }
    $resumeSession = [string]($begin2.Content | ConvertFrom-Json).data.id
    $chunk = New-Object byte[] $partialSize
    [Array]::Copy($partialBytes, 0, $chunk, 0, $partialSize)
    [void](Invoke-Api -Method PATCH -Path "/api/v1/cloud/uploads/$resumeSession" `
            -Headers (Get-AuthHeaders $auth @{ 'Upload-Offset' = '0' }) -Bytes $chunk)

    # 3. Running-задача: после перезагрузки recover_interrupted вернёт её в очередь.
    $jobId = Invoke-Psql ("insert into background_jobs (job_type, status, payload, cancellable, started_at) " +
        "values ('system.metrics.prune', 'running', jsonb_build_object('kind', 'system.metrics.prune'), true, now()) returning id;") $dbUrl

    $volumeRows = Invoke-Psql "select id || '|' || name || '|' || state || '|' || enabled from storage_volumes order by id;" $dbUrl
    $volumes = @()
    foreach ($row in ($volumeRows -split "`r?`n")) {
        $row = $row.Trim()
        if ($row -match '^([^|]+)\|([^|]*)\|([^|]+)\|([^|]+)$') {
            $volumes += [pscustomobject]@{ id = $Matches[1]; name = $Matches[2]; state = $Matches[3]; enabled = $Matches[4] }
        }
    }

    $state = [ordered]@{
        preparedAt       = (Get-Date).ToString('o')
        repo             = $repo
        installDir       = $InstallDir
        serviceName      = $ServiceName
        port             = $Port
        baseUrl          = $base
        dbUrl            = $dbUrl
        psql             = $psql
        logDir           = (Join-Path $InstallDir 'logs')
        username         = $tempAdmin
        password         = $password
        cookieToken      = $auth.token
        csrf             = $auth.csrf
        fileEntryId      = $entryId
        fileSha          = $fileSha
        shareToken       = $shareToken
        resumeSession    = $resumeSession
        resumeSize       = $partialBytes.Length
        resumePartial    = $partialSize
        resumeSha        = $partialSha
        jobId            = $jobId
        volumes          = $volumes
        taskName         = $taskName
        resultFile       = $ResultFile
    }
    $state | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $StateFile -Encoding UTF8
    # SID вместо локализованного имени группы.
    & icacls $StateFile /inheritance:r /grant:r 'SYSTEM:F' '*S-1-5-32-544:F' | Out-Null

    $action = New-ScheduledTaskAction -Execute 'powershell.exe' `
        -Argument ("-NoProfile -ExecutionPolicy Bypass -File `"{0}`" -Phase Verify -StateFile `"{1}`"" -f (Join-Path $PSScriptRoot 'reboot-acceptance.ps1'), $StateFile)
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings -Force | Out-Null
    Write-Host "    задача $taskName зарегистрирована (At startup, SYSTEM)"

    Write-Step 'J4 Prepare завершён'
    Write-Host "  Состояние: $StateFile"
    Write-Host "  Файл:      $entryId (SHA $($fileSha.Substring(0, 16))…)"
    Write-Host "  Resume:    $resumeSession (512 КиБ из 1 МиБ)"
    Write-Host "  Задача:    $jobId (running)"

    if ($NoReboot) {
        Write-Host ''
        Write-Host 'Перезагрузка отключена (-NoReboot). Запустите Verify вручную или перезагрузите хост.' -ForegroundColor Yellow
    } else {
        Write-Host ''
        Write-Host 'Перезагрузка через 20 секунд. Отменить: shutdown /a' -ForegroundColor Yellow
        & shutdown.exe /r /t 20 /f /c 'NEXUS J4 reboot acceptance'
    }
}

# ---------------------------------------------------------------------------
function Get-LogText {
    param($state)
    $files = Get-ChildItem -LiteralPath $state.logDir -Filter 'nexus.log.*' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime
    $text = ''
    foreach ($file in $files) {
        $text += (Get-Content -LiteralPath $file.FullName -Encoding UTF8 -Raw)
    }
    return $text
}

function Invoke-Verify {
    Write-Step 'J4 Verify: проверка после перезагрузки'
    if (-not (Test-Path $StateFile)) { throw "Нет файла состояния $StateFile" }
    $state = Get-Content -LiteralPath $StateFile -Raw | ConvertFrom-Json
    $failures = New-Object System.Collections.Generic.List[string]

    $boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
    $prepared = [datetime]::Parse($state.preparedAt)
    if (-not $DryRun -and $boot -le $prepared.AddSeconds(10)) {
        $failures.Add("хост не перезагружался: LastBootUpTime=$boot, preparedAt=$prepared")
    }

    $service = Get-Service -Name $state.serviceName -ErrorAction SilentlyContinue
    $serviceOk = $false
    if ($service) {
        $deadline = (Get-Date).AddSeconds(180)
        while ((Get-Date) -lt $deadline -and $service.Status -ne 'Running') {
            Start-Sleep -Seconds 2
            $service = Get-Service -Name $state.serviceName -ErrorAction SilentlyContinue
        }
        $serviceOk = $service -and $service.Status -eq 'Running'
        if (-not $serviceOk) { $failures.Add("служба не Running: $($service.Status)") }
    } else {
        $failures.Add('служба не найдена после перезагрузки')
    }
    $startType = if ($service) { [string]$service.StartType } else { 'missing' }
    if ($startType -ne 'Automatic') { $failures.Add("StartType=$startType, ожидался Automatic") }
    $health = $null
    try { $health = Wait-Health 240 } catch { $failures.Add($_.Exception.Message) }

    $processStart = $null
    if (Get-Command Get-CimInstance -ErrorAction SilentlyContinue) {
        $win32 = Get-CimInstance Win32_Service -Filter ("Name='{0}'" -f $state.serviceName) -ErrorAction SilentlyContinue
        if ($win32 -and $win32.ProcessId) {
            $processStart = (Get-Process -Id $win32.ProcessId -ErrorAction SilentlyContinue).StartTime
        }
    }
    if (-not $DryRun -and $processStart -and $processStart -lt $boot) {
        $failures.Add("процесс службы стартовал раньше загрузки: $processStart < $boot")
    }

    # Восстановление running-задачи.
    $jobState = ''
    try {
        $jobId = [string]$state.jobId
        $deadline = (Get-Date).AddSeconds(30)
        do {
            $row = (Invoke-Psql ("select status || '|' || coalesce(error, '') from background_jobs where id = '$jobId';") $state.dbUrl).Trim()
            if ($DryRun -or $row -notmatch '^running\|') { break }
            Start-Sleep -Seconds 1
        } while ((Get-Date) -lt $deadline)
        $jobState = $row
        if (-not $DryRun -and $row -match '^running\|') { $failures.Add("задача $jobId осталась running через 30 с") }
    } catch { $failures.Add('не удалось прочитать состояние задачи: ' + $_.Exception.Message) }

    $log = Get-LogText $state
    $recoveryInLog = $log -match 'возвращены в очередь'
    if (-not $DryRun -and -not $recoveryInLog) { $failures.Add('в журнале нет записи о восстановлении задач') }
    $orphans = 0
    try {
        # Не считаем штатные обслуживающие задачи, запущенные только что.
        $orphans = [int](Invoke-Psql (
            "select count(*) from background_jobs " +
            "where status = 'running' and id <> '$($state.jobId)' " +
            "and started_at < now() - interval '1 minute';") $state.dbUrl)
        if (-not $DryRun -and $orphans -ne 0) { $failures.Add("осиротевших running-задач: $orphans") }
    } catch { $failures.Add('не удалось посчитать running-задачи') }

    # Публичная ссылка: SHA после перезагрузки.
    $downloadSha = ''
    $probe = Join-Path $env:TEMP ('j4-verify-' + [guid]::NewGuid().ToString('N') + '.bin')
    try {
        Invoke-WebRequest -Uri "$($state.baseUrl)/api/v1/public/shares/$($state.shareToken)/download" -UseBasicParsing -OutFile $probe -TimeoutSec 120
        $downloadSha = Get-ShaFile $probe
        if ($downloadSha -ne $state.fileSha) {
            $failures.Add("SHA публичной ссылки после перезагрузки не совпал: $downloadSha")
        }
    } catch { $failures.Add('скачивание публичной ссылки: ' + $_.Exception.Message) }
    finally { Remove-Item $probe -Force -ErrorAction SilentlyContinue }

    # Незавершённая загрузка: offset + продолжение.
    $resumeState = ''
    $resumeSha = ''
    $ws = New-Object Microsoft.PowerShell.Commands.WebRequestSession
    $ws.Cookies.Add((New-Object System.Net.Cookie('nexus_session', [string]$state.cookieToken, '/', '127.0.0.1')))
    $auth = [pscustomobject]@{
        session = $ws; csrf = [string]$state.csrf; username = $state.username; password = $state.password
    }
    $script:CurrentAuth = $auth
    try {
        try {
            $status = Invoke-Api -Method GET -Path "/api/v1/cloud/uploads/$($state.resumeSession)" -Headers (Get-AuthHeaders $auth)
        } catch {
            $auth = Connect-Api $state.username $state.password
            $script:CurrentAuth = $auth
            $status = Invoke-Api -Method GET -Path "/api/v1/cloud/uploads/$($state.resumeSession)" -Headers (Get-AuthHeaders $auth)
        }
        $statusData = ($status.Content | ConvertFrom-Json).data
        $offset = [int64]$statusData.offset
        $resumeState = "$($statusData.status)|offset=$offset"
        if ($offset -lt [int64]$state.resumePartial) {
            $failures.Add("offset незавершённой загрузки уменьшился: $offset < $($state.resumePartial)")
        }
        if ($DryRun) {
            $resumeState += ' (dry-run: без продолжения)'
        } else {
            if ($offset -lt [int64]$state.resumeSize) {
                $all = New-TestBytes ([int]$state.resumeSize) 77
                $remaining = New-Object byte[] ([int]($state.resumeSize - $offset))
                [Array]::Copy($all, $offset, $remaining, 0, $remaining.Length)
                [void](Invoke-Api -Method PATCH -Path "/api/v1/cloud/uploads/$($state.resumeSession)" `
                        -Headers (Get-AuthHeaders $auth @{ 'Upload-Offset' = [string]$offset }) -Bytes $remaining)
            }
            $done = Invoke-Api -Method POST -Path "/api/v1/cloud/uploads/$($state.resumeSession)/complete" `
                -Headers (Get-AuthHeaders $auth @{ 'Idempotency-Key' = [guid]::NewGuid().ToString() }) -Body @{ strategy = 'auto' }
            $doneData = ($done.Content | ConvertFrom-Json).data
            $resumeEntry = [string]$doneData.id
            $share2 = Invoke-Api -Method POST -Path '/api/v1/cloud/shares' `
                -Headers (Get-AuthHeaders $auth @{ 'Idempotency-Key' = [guid]::NewGuid().ToString() }) `
                -Body @{ entryId = $resumeEntry; expiresInHours = 2; maxDownloads = 20; authOnly = $false }
            $token2 = [string](($share2.Content | ConvertFrom-Json).data.token)
            Invoke-WebRequest -Uri "$($state.baseUrl)/api/v1/public/shares/$token2/download" -UseBasicParsing -OutFile $probe -TimeoutSec 120
            $resumeSha = Get-ShaFile $probe
            if ($resumeSha -ne $state.resumeSha) {
                $failures.Add("SHA продолженной загрузки не совпал: $resumeSha")
            }
            $resumeState += "; продолжено, SHA ok=$($resumeSha -eq $state.resumeSha)"
        }
    } catch { $failures.Add('продолжение загрузки: ' + $_.Exception.Message) }
    finally { Remove-Item $probe -Force -ErrorAction SilentlyContinue }

    # Тома.
    $online = 0
    $volumeReport = @()
    try {
        $rows = Invoke-Psql "select name || '|' || state || '|' || enabled from storage_volumes order by name;" $state.dbUrl
        foreach ($row in ($rows -split "`r?`n")) {
            $row = $row.Trim()
            if ($row -match '^([^|]+)\|([^|]+)\|([^|]+)$') {
                $volumeReport += $row
                if ($Matches[2] -eq 'online' -and $Matches[3] -eq 'true') { $online++ }
            }
        }
        if ($online -eq 0) { $failures.Add('нет ни одного online-тома после перезагрузки') }
    } catch { $failures.Add('тома: ' + $_.Exception.Message) }

    $ok = $failures.Count -eq 0
    $result = [ordered]@{
        ok             = $ok
        dryRun         = [bool]$DryRun
        preparedAt     = $state.preparedAt
        bootTime       = $boot.ToString('o')
        service        = [ordered]@{
            name = $state.serviceName; status = if ($service) { [string]$service.Status } else { 'missing' }
            startType = $startType; processStart = if ($processStart) { $processStart.ToString('o') } else { $null }
            health = if ($health) { [string]$health.data.status } else { $null }
            uptimeSeconds = if ($health) { $health.data.uptimeSeconds } else { $null }
        }
        job            = [ordered]@{ id = $state.jobId; db = $jobState; recoveryInLog = $recoveryInLog; orphanRunning = $orphans }
        download       = [ordered]@{ sha = $downloadSha; expected = $state.fileSha; ok = ($downloadSha -eq $state.fileSha) }
        resume         = [ordered]@{ state = $resumeState; sha = $resumeSha; expected = $state.resumeSha; ok = if ($DryRun) { $null } else { ($resumeSha -eq $state.resumeSha) } }
        volumes        = $volumeReport
        failures       = @($failures)
    }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultFile -Encoding UTF8

    $md = @()
    $md += '# J4: приёмка после перезагрузки Windows'
    $md += ''
    $md += ('Дата: ' + (Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))
    $md += ''
    if ($DryRun) {
        $md += '> **DRY-RUN:** проверка выполнена без перезагрузки; автозапуск и'
        $md += '> восстановление задач после reboot этим прогоном не подтверждены.'
        $md += ''
    }
    $md += '| Проверка | Результат |'
    $md += '|---|---|'
    $md += ('| Перезагрузка | boot=' + $result.bootTime + ', prepared=' + $result.preparedAt + ' |')
    $md += ('| Служба | ' + $result.service.status + ', StartType=' + $result.service.startType + ', health=' + $result.service.health + ', uptime=' + $result.service.uptimeSeconds + ' с |')
    $md += ('| Running-задача | ' + $result.job.db + '; recovery в журнале=' + $result.job.recoveryInLog + '; orphan running=' + $result.job.orphanRunning + ' |')
    $md += ('| Публичная ссылка SHA | ' + $result.download.sha + ' (ожидался ' + $result.download.expected + ') |')
    $md += ('| Продолжение загрузки | ' + $result.resume.state + '; SHA=' + $result.resume.sha + ' |')
    $md += ('| Тома | ' + ($volumeReport -join '; ') + ' |')
    $md += ''
    $verdict = if (-not $ok) { 'FAIL' } elseif ($DryRun) { 'PASS (dry-run)' } else { 'PASS' }
    $md += ('Итог: **' + $verdict + '**')
    if ($failures.Count -gt 0) {
        $md += ''
        $md += 'Ошибки:'
        foreach ($failure in $failures) { $md += ('* ' + $failure) }
    }
    $mdText = ($md -join "`r`n")
    Set-Content -LiteralPath ([IO.Path]::ChangeExtension($ResultFile, '.md')) -Value $mdText -Encoding UTF8
    if (-not $DryRun) {
        try {
            $target = Join-Path $state.repo 'docs\reports\reboot-20261120-j4.md'
            Set-Content -LiteralPath $target -Value $mdText -Encoding UTF8
        } catch {
            Write-Host ('  не удалось записать отчёт в репозиторий: ' + $_.Exception.Message) -ForegroundColor Yellow
        }
    }

    # Уборка временного администратора и задачи; сервис/данные оставляем для разбора.
    try { [void](Invoke-Psql "delete from users where username = '$tempAdmin';" $state.dbUrl) } catch { }
    try { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue } catch { }

    Write-Step 'J4 Verify завершён'
    Write-Host ($result | ConvertTo-Json -Depth 8)
    Write-Host "  Отчёт: $ResultFile"
    if (-not $ok) { exit 1 }
}

if ($Phase -eq 'Prepare') { Invoke-Prepare } else { Invoke-Verify }
