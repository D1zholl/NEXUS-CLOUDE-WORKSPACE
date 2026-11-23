#Requires -Version 5.1
<#
.SYNOPSIS
    I5: нагрузка и проверка утечек памяти на живом сервере NEXUS (Windows).

.DESCRIPTION
    Сценарий состоит из четырёх фаз и проверяет именно те пути, которые
    нагружают сервер в реальной работе:

      1. список файлов   - серия GET /cloud/entries с пагинацией;
      2. загрузка        - N файлов заданного размера, параллельно, чанками;
      3. скачивание      - параллельные полные скачивания с проверкой SHA-256;
      4. Range           - начало, середина и хвост каждого файла (206).

    Память измеряется по PrivateMemorySize64 процесса NEXUS. Baseline берётся
    после прогрева (список + загрузка/скачивание двух маленьких файлов),
    затем в течение всей нагрузки отдельный sampler пишет private bytes каждые
    250 мс. Провал - прирост peak/final больше порога (по умолчанию +20 МБ).

    Скрипт ничего не оставляет после себя: загруженные записи отправляются в
    корзину и очищаются, созданный временный том удаляется (если -CleanupVolume).

.PARAMETER BaseUrl
    Базовый URL API. По умолчанию http://127.0.0.1:8080.

.PARAMETER Username
    Администратор NEXUS. Пароль берётся из NEXUS_ADMIN_PASSWORD, чтобы не
    попадать в историю команд и логи.

.PARAMETER FileCount
    Сколько файлов загружать в фазе нагрузки (по умолчанию 16).

.PARAMETER FileSizeMb
    Размер одного файла в МиБ (по умолчанию 4).

.PARAMETER ParallelUploads
    Одновременных загрузок (по умолчанию 4).

.PARAMETER ParallelDownloads
    Одновременных полных скачиваний (по умолчанию 4).

.PARAMETER DownloadRounds
    Сколько раз скачать весь набор (по умолчанию 2).

.PARAMETER ListRequests
    Сколько запросов списка выполнить (по умолчанию 200).

.PARAMETER MemoryGrowthLimitMb
    Допустимый прирост PrivateMemorySize64, МБ (по умолчанию 20).

.PARAMETER ReportDir
    Каталог отчётов (по умолчанию docs\reports).

.PARAMETER ReportName
    Имя отчёта без расширения. По умолчанию load-YYYYMMDD-HHmmss.

.PARAMETER CleanupVolume
    Удалить временный том, если он был создан скриптом.

.EXAMPLE
    $env:NEXUS_ADMIN_PASSWORD = '***'
    powershell -ExecutionPolicy Bypass -File tests/load.ps1 `
      -FileCount 16 -FileSizeMb 4 -ReportDir docs\reports
#>
param(
    [string]$BaseUrl = 'http://127.0.0.1:8080',
    [string]$Username = 'admin',
    [int]$FileCount = 16,
    [int]$FileSizeMb = 4,
    [int]$ParallelUploads = 4,
    [int]$ParallelDownloads = 4,
    [int]$DownloadRounds = 2,
    [int]$ListRequests = 200,
    [int]$MemoryGrowthLimitMb = 20,
    [int]$SampleIntervalMs = 250,
    [string]$ReportDir = 'docs\reports',
    [string]$ReportName = ('load-' + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$VolumeRoot,
    [switch]$CleanupVolume,
    [int]$ServerProcessId = 0
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Net.Http -ErrorAction Stop

$script:MiB = [int64]1048576
$script:BaseUrl = $BaseUrl.TrimEnd('/')
$script:Log = New-Object System.Collections.Generic.List[string]
$script:StartedAt = Get-Date
$script:Csrf = $null
$script:Client = $null
$script:RunId = [guid]::NewGuid()
$script:Samples = New-Object System.Collections.Generic.List[object]
$script:SamplerJob = $null
$script:ExitCode = 0
$script:ReportsSaved = $false
$script:VolumeId = $null
$script:CreatedVolume = $false

function Write-Log {
    param([string]$Message)
    $line = '{0} {1}' -f (Get-Date).ToString('HH:mm:ss'), $Message
    $script:Log.Add($line)
    Write-Host $line
}

function New-HttpClient {
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $handler.AutomaticDecompression = [System.Net.DecompressionMethods]::GZip -bor [System.Net.DecompressionMethods]::Deflate
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(30)
    $script:ClientHandler = $handler
    return $client
}

function New-JsonContent {
    param($Body)
    $json = $Body | ConvertTo-Json -Depth 20 -Compress
    return New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
}

function ConvertFrom-JsonSafe {
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return $null }
    try { return ($Text | ConvertFrom-Json) } catch { return $null }
}

function Get-ApiErrorText {
    param([int]$Status, [string]$Text)
    $parsed = ConvertFrom-JsonSafe $Text
    if ($null -ne $parsed -and $null -ne $parsed.error) {
        return ('{0}: {1}' -f $parsed.error.code, $parsed.error.message)
    }
    if ($Text.Length -gt 400) { return $Text.Substring(0, 400) }
    return $Text
}

function Invoke-Api {
    param(
        [string]$Method,
        [string]$Path,
        $Body = $null,
        [hashtable]$Headers = $null,
        [byte[]]$Bytes = $null,
        [int]$MaxAttempts = 5,
        [switch]$AllowError,
        [switch]$Measure
    )
    $uri = $script:BaseUrl + $Path
    $attempt = 0
    while ($true) {
        $attempt++
        $httpMethod = New-Object System.Net.Http.HttpMethod -ArgumentList $Method
        $request = New-Object System.Net.Http.HttpRequestMessage
        $request.Method = $httpMethod
        $request.RequestUri = ([uri]$uri)
        if ($script:Csrf) {
            [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $script:Csrf)
        }
        if ($null -ne $Headers) {
            foreach ($key in $Headers.Keys) {
                [void]$request.Headers.TryAddWithoutValidation($key, [string]$Headers[$key])
            }
        }
        if ($null -ne $Bytes) {
            $request.Content = New-Object System.Net.Http.ByteArrayContent -ArgumentList (,$Bytes)
            $request.Content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue('application/octet-stream')
        } elseif ($null -ne $Body) {
            $request.Content = New-JsonContent $Body
        }

        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $response = $null
        try {
            $response = $script:Client.SendAsync($request).GetAwaiter().GetResult()
        } catch {
            $sw.Stop()
            $request.Dispose()
            if ($attempt -lt $MaxAttempts) {
                $delay = [math]::Min(4000, 200 * [math]::Pow(2, $attempt - 1))
                Write-Log ("  retry {0}/{1} {2} {3}: {4}" -f $attempt, $MaxAttempts, $Method, $Path, $_.Exception.Message)
                Start-Sleep -Milliseconds $delay
                continue
            }
            throw ("HTTP {0} {1}: {2}" -f $Method, $Path, $_.Exception.Message)
        }

        $status = [int]$response.StatusCode
        $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $sw.Stop()
        $response.Dispose()
        $request.Dispose()

        if ($status -ge 200 -and $status -lt 300) {
            $parsed = ConvertFrom-JsonSafe $text
            if ($Measure) {
                return [pscustomobject]@{ status = $status; ms = $sw.Elapsed.TotalMilliseconds; body = $parsed; text = $text }
            }
            return $parsed
        }
        if (($status -ge 500 -or $status -eq 408 -or $status -eq 429) -and $attempt -lt $MaxAttempts) {
            $delay = [math]::Min(5000, 250 * [math]::Pow(2, $attempt - 1))
            Write-Log ("  retry {0}/{1} {2} {3} -> HTTP {4}: {5}" -f $attempt, $MaxAttempts, $Method, $Path, $status, (Get-ApiErrorText $status $text))
            Start-Sleep -Milliseconds $delay
            continue
        }
        if ($AllowError) {
            return [pscustomobject]@{ __error = $true; status = $status; ms = $sw.Elapsed.TotalMilliseconds; body = (ConvertFrom-JsonSafe $text); text = $text }
        }
        throw ("HTTP {0} {1} {2}: {3}" -f $status, $Method, $Path, (Get-ApiErrorText $status $text))
    }
}

# --- Генератор детерминированного содержимого --------------------------------
if (-not ('NexusLoadContent' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Security.Cryptography;
using System.Text;

public static class NexusLoadContent
{
    private static ulong Seed(Guid id, int index)
    {
        ulong h = 1469598103934665603UL;
        foreach (byte b in id.ToByteArray()) { h ^= b; h *= 1099511628211UL; }
        h ^= (ulong)(uint)index;
        h *= 1099511628211UL;
        return h == 0 ? 0x9E3779B97F4A7C15UL : h;
    }

    public static byte[] Generate(Guid id, int index, long size)
    {
        byte[] buffer = new byte[size];
        ulong state = Seed(id, index);
        int pos = 0;
        byte[] header = new byte[64];
        Encoding.ASCII.GetBytes("NEXUS-LOAD").CopyTo(header, 0);
        id.ToByteArray().CopyTo(header, 16);
        BitConverter.GetBytes(index).CopyTo(header, 32);
        BitConverter.GetBytes(size).CopyTo(header, 36);
        Encoding.ASCII.GetBytes("I5").CopyTo(header, 60);
        int head = Math.Min(64, buffer.Length);
        Array.Copy(header, 0, buffer, 0, head);
        pos = head;
        while (pos < buffer.Length)
        {
            state += 0x9E3779B97F4A7C15UL;
            ulong z = state;
            z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
            z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
            z ^= z >> 31;
            int n = Math.Min(8, buffer.Length - pos);
            for (int i = 0; i < n; i++) { buffer[pos + i] = (byte)(z >> (8 * i)); }
            pos += n;
        }
        return buffer;
    }

    public static string Sha256Hex(byte[] bytes)
    {
        using (SHA256 sha = SHA256.Create())
        {
            return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
        }
    }
}
'@
}

function Get-Sha256Hex {
    param([byte[]]$Bytes)
    return [NexusLoadContent]::Sha256Hex($Bytes)
}

function Get-MemoryMb {
    param([int]$ProcessId)
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if (-not $process) { return $null }
    return [math]::Round($process.PrivateMemorySize64 / $script:MiB, 1)
}

function Resolve-NexusProcess {
    param([int]$Port, [int]$ExplicitId)
    if ($ExplicitId -gt 0) {
        $process = Get-Process -Id $ExplicitId -ErrorAction SilentlyContinue
        if ($process) { return $process }
    }
    try {
        $connection = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop | Select-Object -First 1
        if ($connection) {
            return (Get-Process -Id $connection.OwningProcess -ErrorAction Stop)
        }
    } catch { }
    return (Get-Process -Name 'nexus' -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1)
}

function Start-MemorySampler {
    param([int]$ProcessId, [int]$IntervalMs, [string]$Path)
    if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force }
    return Start-Job -ScriptBlock {
        param($ProcessId, $IntervalMs, $Path)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalMinutes -lt 45) {
            $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
            if (-not $process) { break }
            $sample = [pscustomobject]@{
                at = (Get-Date).ToString('o')
                privateMb = [math]::Round($process.PrivateMemorySize64 / 1MB, 1)
                workingSetMb = [math]::Round($process.WorkingSet64 / 1MB, 1)
            }
            $sample | ConvertTo-Json -Compress | Add-Content -LiteralPath $Path -Encoding UTF8
            Start-Sleep -Milliseconds $IntervalMs
        }
    } -ArgumentList $ProcessId, $IntervalMs, $Path -Name 'nexus-load-memory'
}

function Stop-MemorySampler {
    param($Job, [string]$Path)
    if ($null -eq $Job) { return @() }
    try {
        Stop-Job -Job $Job -ErrorAction SilentlyContinue
        Remove-Job -Job $Job -Force -ErrorAction SilentlyContinue
    } catch { }
    $samples = New-Object System.Collections.Generic.List[object]
    if (Test-Path -LiteralPath $Path) {
        foreach ($line in (Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue)) {
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            try { $samples.Add(($line | ConvertFrom-Json)) } catch { }
        }
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
    return $samples.ToArray()
}

# --- Параллельный запуск в runspace pool -------------------------------------
function Invoke-InParallel {
    param(
        [object[]]$Items,
        [scriptblock]$Script,
        [int]$Throttle,
        [hashtable]$Parameters = @{}
    )
    $results = New-Object System.Collections.Generic.List[object]
    $errors = New-Object System.Collections.Generic.List[string]
    if ($Items.Count -eq 0) { return [pscustomobject]@{ Results = @(); Errors = @() } }
    $pool = [runspacefactory]::CreateRunspacePool(1, [math]::Max(1, $Throttle))
    $pool.Open()
    $jobs = @()
    try {
        foreach ($item in $Items) {
            $ps = [powershell]::Create()
            $ps.RunspacePool = $pool
            [void]$ps.AddScript($Script.ToString())
            [void]$ps.AddParameter('Item', $item)
            foreach ($key in $Parameters.Keys) {
                [void]$ps.AddParameter($key, $Parameters[$key])
            }
            $jobs += [pscustomobject]@{ PowerShell = $ps; Handle = $ps.BeginInvoke() }
        }
        foreach ($job in $jobs) {
            try {
                $output = $job.PowerShell.EndInvoke($job.Handle)
                foreach ($value in $output) { $results.Add($value) }
                foreach ($errorRecord in $job.PowerShell.Streams.Error) {
                    $errors.Add($errorRecord.ToString())
                }
            } catch {
                $errors.Add($_.Exception.Message)
            } finally {
                $job.PowerShell.Dispose()
            }
        }
    } finally {
        $pool.Close()
        $pool.Dispose()
    }
    return [pscustomobject]@{ Results = $results.ToArray(); Errors = $errors.ToArray() }
}

function Get-CookieHeader {
    param([System.Net.Http.HttpClientHandler]$Handler, [string]$BaseUrl)
    $uri = [uri]$BaseUrl
    $cookies = $Handler.CookieContainer.GetCookies($uri)
    $parts = @()
    foreach ($cookie in $cookies) { $parts += ('{0}={1}' -f $cookie.Name, $cookie.Value) }
    return ($parts -join '; ')
}

# --- Окружение ---------------------------------------------------------------
function Get-EnvironmentInfo {
    $os = Get-CimInstance Win32_OperatingSystem
    $computer = Get-CimInstance Win32_ComputerSystem
    $volume = $null
    if ($script:VolumeRoot) {
        try { $volume = Get-CimInstance Win32_LogicalDisk -Filter ("DeviceID='{0}'" -f ([IO.Path]::GetPathRoot($script:VolumeRoot).TrimEnd('\'))) -ErrorAction Stop } catch { }
    }
    $defender = [ordered]@{ available = $false; realTime = $null; antivirus = $null; note = '' }
    try {
        $status = Get-MpComputerStatus -ErrorAction Stop
        $defender.available = $true
        $defender.realTime = [bool]$status.RealTimeProtectionEnabled
        $defender.antivirus = [bool]$status.AntivirusEnabled
    } catch {
        $defender.note = $_.Exception.Message
    }
    return [ordered]@{
        osCaption = $os.Caption
        osVersion = $os.Version
        osBuild = $os.BuildNumber
        powershell = $PSVersionTable.PSVersion.ToString()
        clr = [System.Environment]::Version.ToString()
        computer = $computer.Name
        cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name
        logicalProcessors = $computer.NumberOfLogicalProcessors
        ramGb = [math]::Round($computer.TotalPhysicalMemory / 1GB, 2)
        filesystem = $(if ($volume) { $volume.FileSystem } else { '' })
        drive = $(if ($volume) { $volume.DeviceID } else { '' })
        defenderRealTime = $defender.realTime
        defenderAntivirus = $defender.antivirus
        defenderNote = $defender.note
        defenderAvailable = $defender.available
    }
}

function Get-PostgresVersion {
    $psql = Get-Command psql.exe -ErrorAction SilentlyContinue
    if (-not $psql) {
        $candidate = Get-ChildItem 'C:\Program Files\PostgreSQL\*\bin\psql.exe' -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
        if ($candidate) { $psql = $candidate }
    }
    if (-not $psql) { return 'unknown' }
    try {
        $env:PGPASSWORD = 'nexus'
        $out = & $psql.FullName -U nexus -h 127.0.0.1 -d nexus -t -A -c 'show server_version' 2>$null
        if ($LASTEXITCODE -eq 0 -and $out) { return ([string]$out).Trim() }
    } catch { }
    return 'unknown'
}

function Assert-Volume {
    $volumes = @((Invoke-Api -Method GET -Path '/api/v1/storage/volumes').data)
    $online = @($volumes | Where-Object { $_.enabled -and $_.state -eq 'online' -and $_.mode -eq 'auto' })
    if ($online.Count -gt 0) {
        $script:VolumeRoot = $online[0].rootPath
        $script:VolumeId = $online[0].id
        Write-Log ("том: {0} ({1})" -f $online[0].name, $online[0].rootPath)
        return
    }
    if (-not $VolumeRoot) {
        $VolumeRoot = Join-Path $env:TEMP ('nexus-load-volume-' + $script:RunId.ToString('N').Substring(0, 8))
    }
    New-Item -ItemType Directory -Force -Path $VolumeRoot | Out-Null
    $created = Invoke-Api -Method POST -Path '/api/v1/storage/volumes' -Body @{
        name = 'Load Test ' + $script:RunId.ToString('N').Substring(0, 8)
        path = (Resolve-Path -LiteralPath $VolumeRoot).Path
        mode = 'auto'
        priority = 1000
        reservedBytes = 0
    }
    $script:VolumeId = $created.data.id
    $script:CreatedVolume = $true
    $script:VolumeRoot = $created.data.rootPath
    Write-Log ("создан временный том: {0}" -f $script:VolumeRoot)
}

function Remove-UploadedEntries {
    param([object[]]$Entries)
    foreach ($entry in $Entries) {
        try {
            [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}" -f $entry.id) -AllowError)
            [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}/purge" -f $entry.id) -AllowError)
        } catch {
            Write-Log ("  очистка {0}: {1}" -f $entry.id, $_.Exception.Message)
        }
    }
}

function Save-Reports {
    param($Report, [string]$Directory, [string]$Name)
    if (-not (Test-Path -LiteralPath $Directory)) {
        New-Item -ItemType Directory -Force -Path $Directory | Out-Null
    }
    $jsonPath = Join-Path $Directory ($Name + '.json')
    $mdPath = Join-Path $Directory ($Name + '.md')
    $logPath = Join-Path $Directory ($Name + '.log')
    $Report | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $jsonPath -Encoding UTF8
    $script:Log -join "`r`n" | Set-Content -LiteralPath $logPath -Encoding UTF8

    $lines = New-Object System.Collections.Generic.List[string]
    $add = { param([string]$text) $lines.Add($text) }

    & $add '# I5: нагрузка и память NEXUS'
    & $add ''
    & $add ('**Результат:** `{0}`  ' -f $Report.result)
    & $add ('**Дата:** {0}  ' -f $Report.generatedAt)
    & $add ('**Команда:** `{0}`' -f $Report.command)
    & $add ''
    & $add '## Окружение'
    & $add ''
    & $add '| Параметр | Значение |'
    & $add '|---|---|'
    & $add ('| ОС | {0} (build {1}, {2}) |' -f $Report.environment.osCaption, $Report.environment.osBuild, $Report.environment.osVersion)
    & $add ('| CPU / RAM | {0}, {1} лог. ядер / {2} ГиБ |' -f $Report.environment.cpu, $Report.environment.logicalProcessors, $Report.environment.ramGb)
    & $add ('| PowerShell / CLR | {0} / {1} |' -f $Report.environment.powershell, $Report.environment.clr)
    & $add ('| PostgreSQL | {0} |' -f $Report.environment.postgres)
    & $add ('| ФС тома | {0} ({1}) |' -f $Report.environment.filesystem, $Report.environment.drive)
    & $add ('| Defender real-time | {0} |' -f $Report.environment.defenderRealTime)
    & $add ''
    & $add '## Параметры'
    & $add ''
    & $add '| Параметр | Значение |'
    & $add '|---|---|'
    & $add ('| Сервер | `{0}` (PID {1}, версия {2}) |' -f $Report.server.url, $Report.server.pid, $Report.server.version)
    & $add ('| Файлов / размер | {0} × {1} МиБ |' -f $Report.parameters.fileCount, $Report.parameters.fileSizeMb)
    & $add ('| Параллельность | upload {0}, download {1} |' -f $Report.parameters.parallelUploads, $Report.parameters.parallelDownloads)
    & $add ('| Список | {0} запросов |' -f $Report.parameters.listRequests)
    & $add ('| Скачиваний | {0} раунда(ов) |' -f $Report.parameters.downloadRounds)
    & $add ('| Порог private bytes | +{0} МБ |' -f $Report.parameters.memoryGrowthLimitMb)
    & $add ''
    & $add '## Результаты по фазам'
    & $add ''
    & $add '| Фаза | Запросов | Ошибок | Байт | Секунды | Среднее, мс | p95, мс | МБ/с |'
    & $add '|---|---:|---:|---:|---:|---:|---:|---:|'
    foreach ($phase in $Report.phases) {
        & $add ('| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} |' -f $phase.name, $phase.requests, $phase.errors, $phase.bytes, $phase.seconds, $phase.avgMs, $phase.p95Ms, $phase.mbPerSecond)
    }
    & $add ''
    & $add '## Загрузка и целостность'
    & $add ''
    & $add '| Проверка | Значение |'
    & $add '|---|---|'
    & $add ('| Загружено файлов | {0} |' -f $Report.uploads.completed)
    & $add ('| Байт загружено | {0} |' -f $Report.uploads.bytes)
    & $add ('| Уникальных SHA-256 | {0} |' -f $Report.uploads.distinctSha)
    & $add ('| Ошибок загрузки | {0} |' -f $Report.uploads.errors)
    & $add ('| Скачано с совпавшим SHA | {0} / {1} |' -f $Report.downloads.shaMatched, $Report.downloads.total)
    & $add ('| Ошибок скачивания | {0} |' -f $Report.downloads.errors)
    & $add ('| Range 206 | {0} / {1} |' -f $Report.ranges.ok, $Report.ranges.total)
    & $add ('| Range: неверный Content-Range/байты | {0} |' -f $Report.ranges.failedDetail)
    & $add ''
    & $add '## Память процесса NEXUS'
    & $add ''
    if ($Report.memory.available) {
        & $add '| Метрика | МБ |'
        & $add '|---|---:|'
        & $add ('| Baseline private (после прогрева) | {0} |' -f $Report.memory.baselinePrivateMb)
        & $add ('| Peak private | {0} |' -f $Report.memory.peakPrivateMb)
        & $add ('| Final private (после 3 с) | {0} |' -f $Report.memory.finalPrivateMb)
        & $add ('| Прирост peak | {0} |' -f $Report.memory.peakDeltaMb)
        & $add ('| Прирост final | {0} |' -f $Report.memory.finalDeltaMb)
        & $add ('| Порог | +{0} |' -f $Report.memory.thresholdMb)
        & $add ('| Результат | {0} |' -f ($(if ($Report.memory.passed) { 'PASS' } else { 'FAIL' })))
        & $add ''
        & $add ('Замеров: {0}, интервал {1} мс. Считается PrivateMemorySize64.' -f $Report.memory.samples, $Report.parameters.sampleIntervalMs)
    } else {
        & $add ('Замер не выполнялся: {0}' -f $Report.memory.note)
    }
    & $add ''
    if ($Report.rangeTests.Count -gt 0) {
        & $add '## Range-проверки'
        & $add ''
        & $add '| Файл | Проверка | Статус | Байт | OK |'
        & $add '|---|---|---:|---:|---|'
        foreach ($test in $Report.rangeTests) {
            & $add ('| `{0}` | {1} | {2} | {3} | {4} |' -f $test.name, $test.check, $test.status, $test.bytes, $test.ok)
        }
        & $add ''
    }
    if ($Report.failures.Count -gt 0) {
        & $add '## Ошибки'
        & $add ''
        foreach ($failure in $Report.failures) { & $add ('* ' + $failure) }
        & $add ''
    }
    $lines -join "`r`n" | Set-Content -LiteralPath $mdPath -Encoding UTF8
    Write-Log ("отчёт: {0}" -f $mdPath)
}

# --- Основной сценарий -------------------------------------------------------
$report = [ordered]@{
    result = 'FAIL'
    generatedAt = (Get-Date).ToString('o')
    command = ('powershell -File tests/load.ps1 -FileCount {0} -FileSizeMb {1} -ReportDir "{2}" -ReportName "{3}"' -f $FileCount, $FileSizeMb, $ReportDir, $ReportName)
    environment = [ordered]@{}
    server = [ordered]@{}
    parameters = [ordered]@{
        fileCount = $FileCount
        fileSizeMb = $FileSizeMb
        parallelUploads = $ParallelUploads
        parallelDownloads = $ParallelDownloads
        downloadRounds = $DownloadRounds
        listRequests = $ListRequests
        memoryGrowthLimitMb = $MemoryGrowthLimitMb
        sampleIntervalMs = $SampleIntervalMs
    }
    volume = [ordered]@{ id = $null; rootPath = $null; created = $false }
    phases = @()
    uploads = [ordered]@{ completed = 0; bytes = 0; distinctSha = 0; errors = 0 }
    downloads = [ordered]@{ total = 0; shaMatched = 0; bytes = 0; errors = 0 }
    ranges = [ordered]@{ total = 0; ok = 0; failedDetail = 0 }
    rangeTests = @()
    memory = [ordered]@{ available = $false }
    failures = @()
}

$failures = New-Object System.Collections.Generic.List[string]
$phaseList = New-Object System.Collections.Generic.List[object]
$rangeTests = New-Object System.Collections.Generic.List[object]
$uploaded = New-Object System.Collections.Generic.List[object]
$entries = @()
$bytesByIndex = @{}
$shaByIndex = @{}
$samplePath = Join-Path $env:TEMP ('nexus-load-samples-' + $script:RunId.ToString('N') + '.jsonl')
$serverProcess = $null

try {
    if ([string]::IsNullOrWhiteSpace($env:NEXUS_ADMIN_PASSWORD)) {
        throw 'NEXUS_ADMIN_PASSWORD не задан. Пароль не передаётся аргументом, только через окружение.'
    }
    if ($FileCount -lt 1 -or $FileSizeMb -lt 1) { throw 'FileCount и FileSizeMb должны быть положительными.' }

    $script:Client = New-HttpClient
    $login = Invoke-Api -Method POST -Path '/api/v1/auth/login' -Body @{ username = $Username; password = $env:NEXUS_ADMIN_PASSWORD }
    $script:Csrf = $login.data.csrfToken
    $cookie = Get-CookieHeader -Handler $script:ClientHandler -BaseUrl $script:BaseUrl
    if (-not $cookie) { throw 'Не удалось получить cookie сессии после входа.' }
    Write-Log ("вход: {0}, cookie {1} байт" -f $login.data.user.username, $cookie.Length)

    $health = Invoke-Api -Method GET -Path '/api/v1/health'
    $serverProcess = Resolve-NexusProcess -Port ([uri]$script:BaseUrl).Port -ExplicitId $ServerProcessId
    if ($null -eq $serverProcess) { throw 'Процесс NEXUS не найден: замер PrivateMemorySize64 невозможен.' }
    $report.server = [ordered]@{
        url = $script:BaseUrl
        pid = $serverProcess.Id
        version = $health.data.version
        health = $health.data.status
        database = $health.data.database
    }
    Write-Log ("сервер PID {0}, версия {1}" -f $serverProcess.Id, $health.data.version)

    Assert-Volume
    $report.volume = [ordered]@{ id = $script:VolumeId; rootPath = $script:VolumeRoot; created = $script:CreatedVolume }
    $report.environment = Get-EnvironmentInfo
    $report.environment.postgres = Get-PostgresVersion

    # --- Прогрев: список + два маленьких файла -------------------------------
    Write-Log 'прогрев: список и два файла по 1 МиБ'
    for ($i = 0; $i -lt 10; $i++) {
        [void](Invoke-Api -Method GET -Path '/api/v1/cloud/entries?page=1&perPage=24&view=folder' -Measure)
    }
    $warmEntries = @()
    for ($i = 0; $i -lt 2; $i++) {
        $warmBytes = [NexusLoadContent]::Generate($script:RunId, 1000 + $i, 1 * $script:MiB)
        $session = (Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
            name = ('load-warm-{0}.bin' -f $i)
            parentId = $null
            size = $warmBytes.Length
            contentType = 'application/octet-stream'
        }).data
        [void](Invoke-Api -Method PATCH -Path ("/api/v1/cloud/uploads/{0}" -f $session.id) -Headers @{ 'Upload-Offset' = '0' } -Bytes $warmBytes)
        $done = (Invoke-Api -Method POST -Path ("/api/v1/cloud/uploads/{0}/complete" -f $session.id) -Headers @{ 'Idempotency-Key' = ([guid]::NewGuid().ToString()) } -Body @{ strategy = 'auto' }).data
        $warmEntries += $done
        $dl = Invoke-Api -Method GET -Path ("/api/v1/cloud/entries/{0}/download" -f $done.id) -AllowError
        if ($dl.__error) { throw ("прогрев: скачивание не удалось: HTTP {0}" -f $dl.status) }
    }
    Remove-UploadedEntries -Entries $warmEntries
    Start-Sleep -Seconds 2
    $baselinePrivate = Get-MemoryMb -ProcessId $serverProcess.Id
    if ($null -eq $baselinePrivate) { throw 'Процесс NEXUS исчез до начала нагрузки.' }
    $script:SamplerJob = Start-MemorySampler -ProcessId $serverProcess.Id -IntervalMs $SampleIntervalMs -Path $samplePath
    Write-Log ("baseline private: {0} МБ" -f $baselinePrivate)

    # --- 1. Список -----------------------------------------------------------
    Write-Log ("фаза 1: список, {0} запросов" -f $ListRequests)
    $listSw = [System.Diagnostics.Stopwatch]::StartNew()
    $listLatencies = New-Object System.Collections.Generic.List[double]
    $listErrors = 0
    $lastPage = 0
    for ($i = 0; $i -lt $ListRequests; $i++) {
        $page = 1 + ($i % 5)
        $result = Invoke-Api -Method GET -Path ("/api/v1/cloud/entries?page={0}&perPage=24&sort=name&direction=asc&view=folder" -f $page) -Measure -AllowError
        if ($result.__error) {
            $listErrors++
            $failures.Add(("список: HTTP {0} на запросе {1}" -f $result.status, $i))
        } else {
            $listLatencies.Add([double]$result.ms)
            $lastPage = @($result.body.data.items).Count
        }
    }
    $listSw.Stop()
    $listSorted = $listLatencies | Sort-Object
    $listAvg = if ($listLatencies.Count) { ($listLatencies | Measure-Object -Average).Average } else { 0 }
    $listP95 = if ($listSorted.Count) { $listSorted[[int][Math]::Min($listSorted.Count - 1, [Math]::Floor($listSorted.Count * 0.95))] } else { 0 }
    $phaseList.Add([ordered]@{
        name = 'список'
        requests = $ListRequests
        errors = $listErrors
        bytes = 0
        seconds = [math]::Round($listSw.Elapsed.TotalSeconds, 2)
        avgMs = [math]::Round($listAvg, 1)
        p95Ms = [math]::Round($listP95, 1)
        mbPerSecond = 0
        lastPageItems = $lastPage
    })
    Write-Log ("  список: avg {0:N1} мс, p95 {1:N1} мс, ошибок {2}" -f $listAvg, $listP95, $listErrors)

    # --- 2. Подготовка содержимого и параллельная загрузка -------------------
    Write-Log ("фаза 2: загрузка {0} × {1} МиБ, параллельно {2}" -f $FileCount, $FileSizeMb, $ParallelUploads)
    $sizeBytes = [int64]$FileSizeMb * $script:MiB
    for ($i = 0; $i -lt $FileCount; $i++) {
        $bytesByIndex[$i] = [NexusLoadContent]::Generate($script:RunId, $i, $sizeBytes)
        $shaByIndex[$i] = Get-Sha256Hex $bytesByIndex[$i]
    }
    $distinctSha = @($shaByIndex.Values | Sort-Object -Unique).Count

    $sessions = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $FileCount; $i++) {
        $name = ('load-{0}-{1:D3}.bin' -f $script:RunId.ToString('N').Substring(0, 8), $i)
        $session = (Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
            name = $name
            parentId = $null
            size = $sizeBytes
            contentType = 'application/octet-stream'
        }).data
        $sessions.Add([pscustomobject]@{ Index = $i; SessionId = $session.id; Name = $name; Bytes = $bytesByIndex[$i]; Size = $sizeBytes })
    }

    $uploadScript = {
        param($Item, $BaseUrl, $Cookie, $Csrf, $ChunkBytes)
        $ErrorActionPreference = 'Stop'
        $handler = New-Object System.Net.Http.HttpClientHandler
        $handler.UseCookies = $false
        $client = New-Object System.Net.Http.HttpClient($handler)
        $client.Timeout = [TimeSpan]::FromMinutes(10)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $offset = 0
            $chunks = 0
            while ($offset -lt $Item.Size) {
                $length = [int][Math]::Min([int64]$ChunkBytes, $Item.Size - $offset)
                $slice = New-Object byte[] $length
                [Array]::Copy($Item.Bytes, $offset, $slice, 0, $length)
                $request = New-Object System.Net.Http.HttpRequestMessage
                $request.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'PATCH'
                $request.RequestUri = [uri]("$BaseUrl/api/v1/cloud/uploads/$($Item.SessionId)")
                [void]$request.Headers.TryAddWithoutValidation('Cookie', $Cookie)
                [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $Csrf)
                [void]$request.Headers.TryAddWithoutValidation('Upload-Offset', [string]$offset)
                $request.Content = New-Object System.Net.Http.ByteArrayContent -ArgumentList (,$slice)
                $request.Content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue('application/octet-stream')
                $response = $client.SendAsync($request).GetAwaiter().GetResult()
                $status = [int]$response.StatusCode
                $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                $response.Dispose()
                $request.Dispose()
                if ($status -lt 200 -or $status -ge 300) {
                    return [pscustomobject]@{ Index = $Item.Index; Ok = $false; Error = "HTTP $status $text"; Ms = $sw.Elapsed.TotalMilliseconds; Chunks = $chunks }
                }
                $payload = $text | ConvertFrom-Json
                $offset = [int64]$payload.data.offset
                $chunks++
            }
            $sw.Stop()
            return [pscustomobject]@{ Index = $Item.Index; Ok = $true; Error = ''; Ms = $sw.Elapsed.TotalMilliseconds; Chunks = $chunks; Offset = $offset }
        } catch {
            return [pscustomobject]@{ Index = $Item.Index; Ok = $false; Error = ($_.Exception.Message + ' @ ' + $_.InvocationInfo.PositionMessage); Ms = $sw.Elapsed.TotalMilliseconds; Chunks = 0 }
        } finally {
            $client.Dispose()
        }
    }
    $uploadSw = [System.Diagnostics.Stopwatch]::StartNew()
    $uploadRun = Invoke-InParallel -Items $sessions.ToArray() -Script $uploadScript -Throttle $ParallelUploads -Parameters @{
        BaseUrl = $script:BaseUrl
        Cookie = $cookie
        Csrf = $script:Csrf
        ChunkBytes = [int64]64 * $script:MiB
    }
    $uploadSw.Stop()
    $uploadErrors = @($uploadRun.Errors).Count
    $uploadedOk = 0
    foreach ($errorText in $uploadRun.Errors) { $failures.Add('upload runspace: ' + $errorText) }
    foreach ($result in $uploadRun.Results) {
        if ($result.Ok) { $uploadedOk++ } else {
            $uploadErrors++
            $failures.Add(("загрузка #{0}: {1}" -f $result.Index, $result.Error))
        }
    }

    # Завершение сессий — последовательно, чтобы получить entry id.
    for ($i = 0; $i -lt $sessions.Count; $i++) {
        $session = $sessions[$i]
        $result = $uploadRun.Results | Where-Object { $_.Index -eq $session.Index } | Select-Object -First 1
        if ($null -eq $result -or -not $result.Ok) { continue }
        try {
            $done = (Invoke-Api -Method POST -Path ("/api/v1/cloud/uploads/{0}/complete" -f $session.SessionId) -Headers @{ 'Idempotency-Key' = ([guid]::NewGuid().ToString()) } -Body @{ strategy = 'auto' }).data
            $uploaded.Add([pscustomobject]@{ Index = $session.Index; Id = $done.id; Name = $session.Name; Size = $sizeBytes; Sha = $shaByIndex[$session.Index] })
        } catch {
            $uploadErrors++
            $failures.Add(("complete #{0}: {1}" -f $session.Index, $_.Exception.Message))
        }
    }
    $report.uploads.completed = $uploaded.Count
    $report.uploads.bytes = [int64]$uploaded.Count * $sizeBytes
    $report.uploads.distinctSha = $distinctSha
    $report.uploads.errors = $uploadErrors
    $phaseList.Add([ordered]@{
        name = 'загрузка'
        requests = $sessions.Count
        errors = $uploadErrors
        bytes = $report.uploads.bytes
        seconds = [math]::Round($uploadSw.Elapsed.TotalSeconds, 2)
        avgMs = $(if ($uploadRun.Results.Count) { [math]::Round((($uploadRun.Results | Measure-Object -Property Ms -Average).Average), 1) } else { 0 })
        p95Ms = 0
        mbPerSecond = $(if ($uploadSw.Elapsed.TotalSeconds -gt 0) { [math]::Round(($report.uploads.bytes / $script:MiB) / $uploadSw.Elapsed.TotalSeconds, 2) } else { 0 })
    })
    Write-Log ("  загрузка: {0}/{1} файлов, {2:N1} МБ/с, ошибок {3}" -f $uploaded.Count, $FileCount, (($report.uploads.bytes / $script:MiB) / [math]::Max(0.001, $uploadSw.Elapsed.TotalSeconds)), $uploadErrors)

    # Список после загрузки: все имена видны.
    $listAfter = Invoke-Api -Method GET -Path ('/api/v1/cloud/entries?page=1&perPage=100&view=folder&sort=name&direction=asc')
    $listedNames = @($listAfter.data.items | ForEach-Object { $_.name })
    $missingNames = @($uploaded | Where-Object { $listedNames -notcontains $_.Name })
    if ($missingNames.Count -gt 0) { $failures.Add(("в списке нет {0} загруженных файлов" -f $missingNames.Count)) }

    # --- 3. Параллельные скачивания с проверкой SHA --------------------------
    Write-Log ("фаза 3: скачивание {0} раундов, параллельно {1}" -f $DownloadRounds, $ParallelDownloads)
    $downloadScript = {
        param($Item, $BaseUrl, $Cookie)
        $ErrorActionPreference = 'Stop'
        $handler = New-Object System.Net.Http.HttpClientHandler
        $handler.UseCookies = $false
        $client = New-Object System.Net.Http.HttpClient($handler)
        $client.Timeout = [TimeSpan]::FromMinutes(10)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $request = New-Object System.Net.Http.HttpRequestMessage
            $request.Method = [System.Net.Http.HttpMethod]::Get
            $request.RequestUri = [uri]("$BaseUrl/api/v1/cloud/entries/$($Item.Id)/download")
            [void]$request.Headers.TryAddWithoutValidation('Cookie', $Cookie)
            $response = $client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
            $status = [int]$response.StatusCode
            if ($status -ne 200) {
                $response.Dispose(); $request.Dispose()
                return [pscustomobject]@{ Index = $Item.Index; Ok = $false; Error = "HTTP $status"; Bytes = 0; Ms = $sw.Elapsed.TotalMilliseconds; Sha = '' }
            }
            $expectedLength = [int64]$response.Content.Headers.ContentLength
            $stream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $sha = [System.Security.Cryptography.SHA256]::Create()
            $buffer = New-Object byte[] 81920
            $total = [int64]0
            $zero = New-Object byte[] 81920
            while (($n = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $total += $n
                [void]$sha.TransformBlock($buffer, 0, $n, $zero, 0)
            }
            [void]$sha.TransformFinalBlock($zero, 0, 0)
            $hash = ([System.BitConverter]::ToString($sha.Hash) -replace '-', '').ToLowerInvariant()
            $sha.Dispose(); $stream.Dispose(); $response.Dispose(); $request.Dispose()
            $sw.Stop()
            $ok = ($total -eq $Item.Size) -and ($expectedLength -eq $Item.Size) -and ($hash -eq $Item.Sha)
            return [pscustomobject]@{ Index = $Item.Index; Ok = $ok; Error = $(if ($ok) { '' } else { "size=$total/$($Item.Size) sha=$hash" }); Bytes = $total; Ms = $sw.Elapsed.TotalMilliseconds; Sha = $hash }
        } catch {
            return [pscustomobject]@{ Index = $Item.Index; Ok = $false; Error = $_.Exception.Message; Bytes = 0; Ms = $sw.Elapsed.TotalMilliseconds; Sha = '' }
        } finally {
            $client.Dispose()
        }
    }
    $downloadSw = [System.Diagnostics.Stopwatch]::StartNew()
    $downloadTotal = 0
    $downloadShaMatched = 0
    $downloadErrors = 0
    $downloadBytes = [int64]0
    $downloadLatencies = New-Object System.Collections.Generic.List[double]
    for ($round = 0; $round -lt $DownloadRounds; $round++) {
        $downloadRun = Invoke-InParallel -Items $uploaded.ToArray() -Script $downloadScript -Throttle $ParallelDownloads -Parameters @{
            BaseUrl = $script:BaseUrl
            Cookie = $cookie
        }
        foreach ($errorText in $downloadRun.Errors) { $downloadErrors++; $failures.Add('download runspace: ' + $errorText) }
        foreach ($result in $downloadRun.Results) {
            $downloadTotal++
            $downloadLatencies.Add([double]$result.Ms)
            if ($result.Ok) { $downloadShaMatched++; $downloadBytes += [int64]$result.Bytes } else {
                $downloadErrors++
                $failures.Add(("скачивание #{0} раунд {1}: {2}" -f $result.Index, ($round + 1), $result.Error))
            }
        }
    }
    $downloadSw.Stop()
    $report.downloads = [ordered]@{ total = $downloadTotal; shaMatched = $downloadShaMatched; bytes = $downloadBytes; errors = $downloadErrors }
    $dAvg = if ($downloadLatencies.Count) { ($downloadLatencies | Measure-Object -Average).Average } else { 0 }
    $dSorted = $downloadLatencies | Sort-Object
    $dP95 = if ($dSorted.Count) { $dSorted[[int][Math]::Min($dSorted.Count - 1, [Math]::Floor($dSorted.Count * 0.95))] } else { 0 }
    $phaseList.Add([ordered]@{
        name = 'скачивание'
        requests = $downloadTotal
        errors = $downloadErrors
        bytes = $downloadBytes
        seconds = [math]::Round($downloadSw.Elapsed.TotalSeconds, 2)
        avgMs = [math]::Round($dAvg, 1)
        p95Ms = [math]::Round($dP95, 1)
        mbPerSecond = $(if ($downloadSw.Elapsed.TotalSeconds -gt 0) { [math]::Round(($downloadBytes / $script:MiB) / $downloadSw.Elapsed.TotalSeconds, 2) } else { 0 })
    })
    Write-Log ("  скачивание: {0}/{1} с верным SHA, {2:N1} МБ/с, ошибок {3}" -f $downloadShaMatched, $downloadTotal, (($downloadBytes / $script:MiB) / [math]::Max(0.001, $downloadSw.Elapsed.TotalSeconds)), $downloadErrors)

    # --- 4. Range ------------------------------------------------------------
    Write-Log 'фаза 4: Range (начало, середина, хвост)'
    $rangeSw = [System.Diagnostics.Stopwatch]::StartNew()
    $rangeTotal = 0
    $rangeOk = 0
    $rangeDetailFailures = 0
    $rangeLatencies = New-Object System.Collections.Generic.List[double]
    $rangeTargets = @($uploaded | Select-Object -First ([Math]::Min(8, $uploaded.Count)))
    foreach ($entry in $rangeTargets) {
        $bytes = $bytesByIndex[$entry.Index]
        $length = [int64]$bytes.Length
        $span = [int64][Math]::Min(1048576, [Math]::Max(1, [Math]::Floor($length / 8)))
        $ranges = @(
            @{ check = 'начало'; start = [int64]0 },
            @{ check = 'середина'; start = [int64][Math]::Floor(($length - $span) / 2) },
            @{ check = 'хвост'; start = [int64]($length - $span) }
        )
        foreach ($range in $ranges) {
            $rangeTotal++
            $start = [int64]$range.start
            $end = [int64]($start + $span - 1)
            $request = New-Object System.Net.Http.HttpRequestMessage
            $request.Method = [System.Net.Http.HttpMethod]::Get
            $request.RequestUri = [uri]("{0}/api/v1/cloud/entries/{1}/download" -f $script:BaseUrl, $entry.Id)
            [void]$request.Headers.TryAddWithoutValidation('Range', ("bytes={0}-{1}" -f $start, $end))
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            try {
                $response = $script:Client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
                $status = [int]$response.StatusCode
                $contentRange = [string]$response.Content.Headers.GetValues('Content-Range')[0]
                $body = $response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
                $response.Dispose()
                $sw.Stop()
                $expected = New-Object byte[] $span
                [Array]::Copy($bytes, $start, $expected, 0, $span)
                $bytesOk = $false
                if ($body.Length -eq $expected.Length) {
                    $expectedHash = [System.Convert]::ToBase64String([System.Security.Cryptography.SHA256]::Create().ComputeHash($expected))
                    $bodyHash = [System.Convert]::ToBase64String([System.Security.Cryptography.SHA256]::Create().ComputeHash($body))
                    $bytesOk = ($expectedHash -eq $bodyHash)
                }
                $headersOk = ($contentRange -eq ("bytes {0}-{1}/{2}" -f $start, $end, $length))
                $testOk = ($status -eq 206) -and ($body.Length -eq $span) -and $headersOk -and $bytesOk
                if ($testOk) { $rangeOk++ } else { $rangeDetailFailures++ }
                $rangeLatencies.Add([double]$sw.Elapsed.TotalMilliseconds)
                $rangeTests.Add([pscustomobject]@{
                    name = $entry.Name; check = $range.check; status = $status; bytes = $body.Length; ok = $testOk
                    contentRange = $contentRange; bytesOk = $bytesOk
                })
                if (-not $testOk) {
                    $failures.Add(("Range {0} {1}: status={2}, content-range={3}, bytesOk={4}" -f $entry.Name, $range.check, $status, $contentRange, $bytesOk))
                }
            } catch {
                $sw.Stop()
                $rangeDetailFailures++
                $failures.Add(("Range {0} {1}: {2}" -f $entry.Name, $range.check, $_.Exception.Message))
            } finally {
                $request.Dispose()
            }
        }
    }
    $rangeSw.Stop()
    $report.ranges = [ordered]@{ total = $rangeTotal; ok = $rangeOk; failedDetail = $rangeDetailFailures }
    $report.rangeTests = $rangeTests.ToArray()
    $rAvg = if ($rangeLatencies.Count) { ($rangeLatencies | Measure-Object -Average).Average } else { 0 }
    $rSorted = $rangeLatencies | Sort-Object
    $rP95 = if ($rSorted.Count) { $rSorted[[int][Math]::Min($rSorted.Count - 1, [Math]::Floor($rSorted.Count * 0.95))] } else { 0 }
    $phaseList.Add([ordered]@{
        name = 'Range'
        requests = $rangeTotal
        errors = $rangeDetailFailures
        bytes = [int64]$rangeTotal * 1048576
        seconds = [math]::Round($rangeSw.Elapsed.TotalSeconds, 2)
        avgMs = [math]::Round($rAvg, 1)
        p95Ms = [math]::Round($rP95, 1)
        mbPerSecond = 0
    })
    Write-Log ("  Range: {0}/{1} ok, {2} с неверными деталями" -f $rangeOk, $rangeTotal, $rangeDetailFailures)

    # --- Очистка загруженных записей -----------------------------------------
    Remove-UploadedEntries -Entries $uploaded.ToArray()
    Write-Log ("очистка: удалено {0} записей" -f $uploaded.Count)

    # --- Память --------------------------------------------------------------
    Start-Sleep -Seconds 3
    $finalPrivate = Get-MemoryMb -ProcessId $serverProcess.Id
    $samples = Stop-MemorySampler -Job $script:SamplerJob -Path $samplePath
    $script:SamplerJob = $null
    $peakPrivate = $baselinePrivate
    foreach ($sample in $samples) {
        if ([double]$sample.privateMb -gt [double]$peakPrivate) { $peakPrivate = [double]$sample.privateMb }
    }
    if ($null -ne $finalPrivate -and [double]$finalPrivate -gt [double]$peakPrivate) { $peakPrivate = [double]$finalPrivate }
    $peakDelta = [math]::Round($peakPrivate - $baselinePrivate, 1)
    $finalDelta = if ($null -ne $finalPrivate) { [math]::Round($finalPrivate - $baselinePrivate, 1) } else { $null }
    $memoryPassed = ($peakDelta -le $MemoryGrowthLimitMb) -and ($null -ne $finalDelta) -and ($finalDelta -le $MemoryGrowthLimitMb)
    $report.memory = [ordered]@{
        available = $true
        baselinePrivateMb = $baselinePrivate
        peakPrivateMb = [math]::Round($peakPrivate, 1)
        finalPrivateMb = $finalPrivate
        peakDeltaMb = $peakDelta
        finalDeltaMb = $finalDelta
        thresholdMb = $MemoryGrowthLimitMb
        samples = $samples.Count
        passed = $memoryPassed
    }
    Write-Log ("память: baseline {0} МБ, peak {1} МБ (+{2}), final {3} МБ (+{4}), порог +{5}" -f `
        $baselinePrivate, [math]::Round($peakPrivate, 1), $peakDelta, $finalPrivate, $finalDelta, $MemoryGrowthLimitMb)
    if (-not $memoryPassed) {
        $failures.Add(("утечка/рост private bytes: peak +{0} МБ, final +{1} МБ, порог +{2} МБ" -f $peakDelta, $finalDelta, $MemoryGrowthLimitMb))
    }

    $report.phases = $phaseList.ToArray()
    if ($failures.Count -gt 0) {
        $report.failures = $failures.ToArray()
        $report.result = 'FAIL'
        $script:ExitCode = 1
    } else {
        $report.result = 'PASS'
        $script:ExitCode = 0
    }
} catch {
    $report.result = 'FAIL'
    $failures.Add($_.Exception.Message)
    $report.failures = $failures.ToArray()
    Write-Log ('ОШИБКА: ' + $_.Exception.Message)
    Write-Host $_.ScriptStackTrace
    $script:ExitCode = 1
} finally {
    try {
        if ($script:SamplerJob) {
            $samples = Stop-MemorySampler -Job $script:SamplerJob -Path $samplePath
            if ($report.memory.available -eq $false -and $samples.Count -gt 0) {
                $report.memory = [ordered]@{ available = $true; samples = $samples.Count; note = 'Прогон прерван; доступны только samples.'; series = $samples }
            }
        }
        if (-not $script:ReportsSaved) {
            Save-Reports -Report $report -Directory $ReportDir -Name $ReportName
            $script:ReportsSaved = $true
        }
        if ($script:CreatedVolume -and $CleanupVolume -and $script:VolumeId) {
            try {
                [void](Invoke-Api -Method DELETE -Path ("/api/v1/storage/volumes/{0}" -f $script:VolumeId) -AllowError)
                if ($script:VolumeRoot -and (Test-Path -LiteralPath $script:VolumeRoot)) {
                    Remove-Item -LiteralPath $script:VolumeRoot -Recurse -Force -ErrorAction SilentlyContinue
                }
                Write-Log 'временный том удалён'
            } catch {
                Write-Log ('не удалось удалить временный том: ' + $_.Exception.Message)
            }
        }
    } catch {
        Write-Host ('не удалось завершить отчёт: ' + $_.Exception.Message)
    }
    if ($script:Client) { $script:Client.Dispose() }
}

exit $script:ExitCode
