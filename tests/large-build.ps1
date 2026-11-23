#Requires -Version 5.1
<#
.SYNOPSIS
    Нагрузочный сценарий G2: загрузка ~25 ГБ в Minecraft-сборку, проверка,
    релиз, манифест и выборочная сверка SHA.

.DESCRIPTION
    Скрипт создаёт (или выбирает) сборку и том хранения, потоково генерирует
    файлы уникального содержимого, считает SHA-256 через .NET IncrementalHash,
    заливает чанками, сверяет SHA клиента/сервера/манифеста, публикует релиз,
    проверяет детерминированность манифеста и Range-скачивание.

    Файлы НЕ пишутся на диск клиента: содержимое генерируется в память
    порциями (по умолчанию 8 МБ) и сразу уходит в upload-сессию. Поэтому для
    теста нужно место только на серверном томе. Staging и objects лежат в
    одном корне, поэтому фиксация идёт через rename (1x места). Если бы тома
    были разными, скрипт потребовал бы 2x и сообщил бы об этом.

    Замер памяти: каждые 10 секунд читаются WorkingSet64 и
    PrivateMemorySize64 процесса NEXUS. Порог +200 МБ считается по private
    bytes (Windows активно подрезает working set).

.PARAMETER SizeGb
    Объём полезной нагрузки в ГиБ (по умолчанию 25).

.PARAMETER FileCount
    Число файлов (по умолчанию 500).

.PARAMETER VolumeRoot
    Корень зарегистрированного тома хранения. Если каталог ещё не
    зарегистрирован, скрипт создаст том через API. Если не задан — берётся
    онлайн-том с наибольшим свободным местом.

.PARAMETER ReportDir
    Каталог отчётов (по умолчанию docs\reports).

.PARAMETER ReportName
    Базовое имя отчёта без расширения. По умолчанию large-build-YYYYMMDD.
    Для smoke-прогона удобно указать large-build-YYYYMMDD-smoke1gb.

.PARAMETER BaseUrl
    Базовый URL API (по умолчанию http://127.0.0.1:8080).

.PARAMETER Username
    Администратор NEXUS (по умолчанию admin).

.PARAMETER Password
    Пароль администратора. Если не задан, берётся NEXUS_ADMIN_PASSWORD.

.PARAMETER ChunkMb
    Размер чанка загрузки в МиБ, но не больше limits.chunk_max_mb сервера.

.PARAMETER PrivateMemoryLimitMb
    Порог прироста private bytes, МБ (по умолчанию 200).

.PARAMETER Cleanup
    После замеров архивировать релиз и удалить сборку (освобождает место).

.EXAMPLE
    powershell -File tests/large-build.ps1 -SizeGb 1 -FileCount 40 -VolumeRoot "C:\Temp\nexus-g2" -ReportDir docs\reports

.EXAMPLE
    $env:NEXUS_ADMIN_PASSWORD = '***'
    powershell -File tests/large-build.ps1 -SizeGb 25 -FileCount 500 -VolumeRoot "C:\Temp\nexus-g2" -ReportDir docs\reports
#>
param(
    [double]$SizeGb = 25,
    [int]$FileCount = 500,
    [string]$VolumeRoot = '',
    [string]$ReportDir = 'docs\reports',
    [string]$ReportName = '',
    [string]$BaseUrl = 'http://127.0.0.1:8080',
    [string]$Username = 'admin',
    [string]$Password = $env:NEXUS_ADMIN_PASSWORD,
    [int]$ChunkMb = 8,
    [int]$PrivateMemoryLimitMb = 200,
    [int]$SampleSeconds = 10,
    [switch]$Cleanup
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
    Add-Type -AssemblyName System.Net.Http -ErrorAction Stop
} catch {
    throw 'Не удалось загрузить System.Net.Http (нужен .NET Framework 4.5+).'
}
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
$script:GiB = [int64]1073741824
$script:MiB = [int64]1048576
$script:KiB = [int64]1024
$script:BaseUrl = $BaseUrl.TrimEnd('/')
$script:LogLines = New-Object System.Collections.Generic.List[string]
$script:StartedAt = Get-Date
$script:ChunkRequests = 0
$script:Samples = New-Object System.Collections.Generic.List[object]
$script:LastSampleAt = [DateTime]::MinValue

function Write-Log {
    param([string]$Message)
    $line = '{0} {1}' -f (Get-Date).ToString('HH:mm:ss'), $Message
    $script:LogLines.Add($line)
    Write-Host $line
}

function Format-Gb([int64]$Bytes) {
    return [math]::Round($Bytes / $script:GiB, 2)
}

function New-HttpClient {
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(20)
    return $client
}

function New-JsonContent {
    param($Body)
    $json = $Body | ConvertTo-Json -Depth 20 -Compress
    $content = New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
    return $content
}

function Get-JsonSafe {
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return $null }
    try { return ($Text | ConvertFrom-Json) } catch { return $null }
}

function Get-ApiErrorText {
    param([int]$Status, [string]$Text)
    $parsed = Get-JsonSafe $Text
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
        [int]$MaxAttempts = 6,
        [switch]$AllowError
    )
    $uri = $script:BaseUrl + $Path
    $attempt = 0
    while ($true) {
        $attempt++
        $httpMethod = New-Object System.Net.Http.HttpMethod -ArgumentList $Method
        $request = New-Object System.Net.Http.HttpRequestMessage -ArgumentList $httpMethod, $uri
        if ($script:CsrfToken) {
            [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $script:CsrfToken)
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

        $response = $null
        try {
            $response = $script:Client.SendAsync($request).GetAwaiter().GetResult()
        } catch {
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
        $response.Dispose()
        $request.Dispose()

        if ($status -ge 200 -and $status -lt 300) {
            return (Get-JsonSafe $text)
        }
        if (($status -ge 500 -or $status -eq 408 -or $status -eq 429) -and $attempt -lt $MaxAttempts) {
            $delay = [math]::Min(5000, 250 * [math]::Pow(2, $attempt - 1))
            Write-Log ("  retry {0}/{1} {2} {3} -> HTTP {4}: {5}" -f $attempt, $MaxAttempts, $Method, $Path, $status, (Get-ApiErrorText $status $text))
            Start-Sleep -Milliseconds $delay
            continue
        }
        if ($AllowError) {
            return [pscustomobject]@{ __error = $true; status = $status; body = (Get-JsonSafe $text); text = $text }
        }
        throw ("HTTP {0} {1} {2}: {3}" -f $status, $Method, $Path, (Get-ApiErrorText $status $text))
    }
}

# --- Определение тома для staging/objects (P/Invoke GetVolumePathName) --------
if (-not ('NexusVolumeUtil' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class NexusVolumeUtil
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool GetVolumePathName(string lpszFileName, StringBuilder lpszVolumePathName, int cchBufferLength);

    public static string ForPath(string path)
    {
        StringBuilder buffer = new StringBuilder(1024);
        if (!GetVolumePathName(path, buffer, buffer.Capacity))
        {
            return null;
        }
        return buffer.ToString();
    }
}
'@
}

# --- Быстрый генератор уникального содержимого (потоково, без файла) ---------
if (-not ('NexusChunkGenerator' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Text;

public sealed class NexusChunkGenerator
{
    private readonly Guid runId;
    private readonly int fileIndex;
    private readonly long fileSize;
    private ulong state;

    public NexusChunkGenerator(Guid runId, int fileIndex, long fileSize)
    {
        this.runId = runId;
        this.fileIndex = fileIndex;
        this.fileSize = fileSize;
        this.state = Seed(runId, fileIndex);
    }

    public void Reset()
    {
        this.state = Seed(runId, fileIndex);
    }

    private static ulong Seed(Guid id, int index)
    {
        ulong h = 1469598103934665603UL;
        byte[] bytes = id.ToByteArray();
        for (int i = 0; i < bytes.Length; i++)
        {
            h ^= bytes[i];
            h *= 1099511628211UL;
        }
        h ^= (ulong)(uint)index;
        h *= 1099511628211UL;
        return h == 0 ? 0x9E3779B97F4A7C15UL : h;
    }

    private ulong Next()
    {
        state += 0x9E3779B97F4A7C15UL;
        ulong z = state;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
        return z ^ (z >> 31);
    }

    public void Fill(byte[] buffer, int offset, int count, long fileOffset)
    {
        int pos = 0;
        if (fileOffset == 0 && count >= 64)
        {
            byte[] header = new byte[64];
            Encoding.ASCII.GetBytes("NEXUS-G2").CopyTo(header, 0);
            runId.ToByteArray().CopyTo(header, 8);
            BitConverter.GetBytes(fileIndex).CopyTo(header, 24);
            BitConverter.GetBytes(fileSize).CopyTo(header, 28);
            Encoding.ASCII.GetBytes("UNIQ").CopyTo(header, 60);
            Array.Copy(header, 0, buffer, offset, 64);
            pos = 64;
        }
        while (pos < count)
        {
            ulong value = Next();
            int n = Math.Min(8, count - pos);
            for (int i = 0; i < n; i++)
            {
                buffer[offset + pos + i] = (byte)(value >> (8 * i));
            }
            pos += n;
        }
    }
}
'@
}

function Get-VolumeNameForPath {
    param([string]$Path)
    $current = $Path
    for ($i = 0; $i -lt 32 -and $current; $i++) {
        if (Test-Path -LiteralPath $current) {
            return [NexusVolumeUtil]::ForPath($current)
        }
        $parent = Split-Path -LiteralPath $current -Parent
        if (-not $parent -or $parent -eq $current) { break }
        $current = $parent
    }
    return $null
}

function Get-DriveInfo {
    param([string]$Path)
    $root = [System.IO.Path]::GetPathRoot($Path)
    if (-not $root -or $root.StartsWith('\\')) {
        throw "Путь тома должен быть локальным диском (получено: $Path)."
    }
    $device = $root.TrimEnd('\', '/')
    if (-not $device.EndsWith(':')) { $device = $device + ':' }
    $disk = Get-CimInstance Win32_LogicalDisk -Filter ("DeviceID='{0}'" -f $device) -ErrorAction SilentlyContinue
    if (-not $disk) {
        throw "Не удалось получить сведения о диске $device через Win32_LogicalDisk."
    }
    return [pscustomobject]@{
        DeviceID = $disk.DeviceID
        FileSystem = $disk.FileSystem
        DriveType = $disk.DriveType
        Size = [int64]$disk.Size
        FreeSpace = [int64]$disk.FreeSpace
        VolumeName = $disk.VolumeName
    }
}

function Get-PhysicalDiskInfo {
    param([string]$DeviceID)
    try {
        $partition = Get-Partition -DriveLetter $DeviceID.TrimEnd(':') -ErrorAction Stop | Select-Object -First 1
        $disk = Get-Disk -Number $partition.DiskNumber -ErrorAction Stop
        $physical = Get-PhysicalDisk -DeviceNumber $disk.Number -ErrorAction Stop
        return [pscustomobject]@{
            FriendlyName = $physical.FriendlyName
            MediaType = [string]$physical.MediaType
            BusType = [string]$physical.BusType
        }
    } catch {
        return [pscustomobject]@{ FriendlyName = 'unknown'; MediaType = 'unknown'; BusType = 'unknown' }
    }
}

function Measure-Tree {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ Files = 0; Bytes = [int64]0 }
    }
    $files = @(Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue)
    $sum = [int64]0
    foreach ($file in $files) { $sum += [int64]$file.Length }
    return [pscustomobject]@{ Files = $files.Count; Bytes = $sum }
}

function ConvertTo-HexString([byte[]]$Bytes) {
    return ([System.BitConverter]::ToString($Bytes) -replace '-', '').ToLowerInvariant()
}

function Resolve-NexusProcess {
    param([int]$Port)
    try {
        $connection = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop | Select-Object -First 1
        if ($connection) {
            return (Get-Process -Id $connection.OwningProcess -ErrorAction Stop)
        }
    } catch { }
    $process = Get-Process -Name 'nexus' -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1
    if ($process) { return $process }
    return $null
}

function Get-MemorySample {
    if (-not $script:NexusProcessId) { return $null }
    $process = Get-Process -Id $script:NexusProcessId -ErrorAction SilentlyContinue
    if (-not $process) { return $null }
    $sample = [pscustomobject]@{
        At = (Get-Date).ToString('o')
        WorkingSetMb = [math]::Round($process.WorkingSet64 / $script:MiB, 1)
        PrivateMb = [math]::Round($process.PrivateMemorySize64 / $script:MiB, 1)
    }
    $script:Samples.Add($sample)
    $script:LastSampleAt = Get-Date
    return $sample
}

function Invoke-MemorySampleIfDue {
    if (((Get-Date) - $script:LastSampleAt).TotalSeconds -ge $SampleSeconds) {
        [void](Get-MemorySample)
    }
}

function Find-BuildFileByPath {
    param($BuildId, [string]$Path)
    try {
        $response = Invoke-Api -Method GET -Path ("/api/v1/minecraft/builds/{0}/files" -f $BuildId) -MaxAttempts 3
        foreach ($item in @($response.data.items)) {
            if (([string]$item.path).ToLowerInvariant() -eq $Path.ToLowerInvariant() -and $item.kind -eq 'file') {
                return $item
            }
        }
    } catch {
        # Список файлов недоступен — считаем, что файла ещё нет.
    }
    return $null
}

function Get-MemorySummary {
    if ($script:Samples.Count -eq 0) {
        return [ordered]@{
            available = $false
            samples = 0
            note = 'Процесс NEXUS не найден: замер памяти не выполнялся.'
        }
    }
    $first = $script:Samples[0]
    $firstWs = [double]$first.WorkingSetMb
    $firstPrivate = [double]$first.PrivateMb
    $peakWs = [double](($script:Samples | Measure-Object -Property WorkingSetMb -Maximum).Maximum)
    $peakPrivate = [double](($script:Samples | Measure-Object -Property PrivateMb -Maximum).Maximum)
    $delta = [math]::Round($peakPrivate - $firstPrivate, 1)
    $series = $script:Samples.ToArray()
    return [ordered]@{
        available = $true
        baselineWorkingSetMb = $firstWs
        baselinePrivateMb = $firstPrivate
        peakWorkingSetMb = [math]::Round($peakWs, 1)
        peakPrivateMb = [math]::Round($peakPrivate, 1)
        privateDeltaMb = $delta
        thresholdMb = $PrivateMemoryLimitMb
        passed = ($delta -le $PrivateMemoryLimitMb)
        samples = $script:Samples.Count
        series = $series
    }
}

function Get-DefenderInfo {
    param([string]$Root, [string]$Staging, [string]$Objects)
    $info = [ordered]@{
        available = $false
        realTimeProtectionEnabled = $null
        antivirusEnabled = $null
        amServiceEnabled = $null
        exclusionPaths = @()
        preferenceAvailable = $false
        stagingExcluded = 'unknown'
        objectsExcluded = 'unknown'
        note = ''
    }
    try {
        $status = Get-MpComputerStatus -ErrorAction Stop
        $info.available = $true
        $info.realTimeProtectionEnabled = [bool]$status.RealTimeProtectionEnabled
        $info.antivirusEnabled = [bool]$status.AntivirusEnabled
        $info.amServiceEnabled = [bool]$status.AMServiceEnabled
    } catch {
        $info.note = "Get-MpComputerStatus недоступен: $($_.Exception.Message)"
    }
    try {
        $pref = Get-MpPreference -ErrorAction Stop
        $info.preferenceAvailable = $true
        $info.stagingExcluded = $false
        $info.objectsExcluded = $false
        $paths = @()
        if ($pref.ExclusionPath) { $paths = @($pref.ExclusionPath) }
        $info.exclusionPaths = $paths
        $rootNorm = $Root.TrimEnd('\', '/')
        foreach ($path in $paths) {
            $ex = ([string]$path).TrimEnd('\', '/')
            if ($ex -ieq $rootNorm -or $Staging.StartsWith($ex + '\', [StringComparison]::OrdinalIgnoreCase)) {
                $info.stagingExcluded = $true
            }
            if ($ex -ieq $rootNorm -or $Objects.StartsWith($ex + '\', [StringComparison]::OrdinalIgnoreCase)) {
                $info.objectsExcluded = $true
            }
        }
    } catch {
        if (-not $info.note) { $info.note = "Get-MpPreference недоступен: $($_.Exception.Message)" }
        else { $info.note = $info.note + "; Get-MpPreference недоступен: $($_.Exception.Message)" }
    }
    return $info
}

function Save-Reports {
    param($Report, [string]$Directory, [string]$Name)
    if (-not (Test-Path -LiteralPath $Directory)) {
        New-Item -ItemType Directory -Force -Path $Directory | Out-Null
    }
    $jsonPath = Join-Path $Directory ($Name + '.json')
    $mdPath = Join-Path $Directory ($Name + '.md')
    $logPath = Join-Path $Directory ($Name + '.log')

    $Report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $jsonPath -Encoding UTF8
    $script:LogLines -join "`r`n" | Set-Content -LiteralPath $logPath -Encoding UTF8

    $lines = New-Object System.Collections.Generic.List[string]
    $add = { param([string]$text) $lines.Add($text) }

    & $add ('# G2: нагрузочный прогон {0} ГиБ' -f $Report.parameters.SizeGb)
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
    & $add ('| PowerShell / .NET | {0} / {1} |' -f $Report.environment.powershell, $Report.environment.dotnet)
    & $add ('| Файловая система | {0} |' -f $Report.environment.filesystem)
    & $add ('| Диск | {0}, {1}, {2} ({3}) |' -f $Report.environment.driveLetter, $Report.environment.physicalDisk, $Report.environment.mediaType, $Report.environment.busType)
    & $add ('| Свободно до старта | {0} ГиБ |' -f $Report.environment.freeBeforeGb)
    & $add ('| Свободно после загрузки (до очистки) | {0} ГиБ |' -f $Report.environment.freeAfterUploadGb)
    & $add ('| Свободно после очистки | {0} ГиБ |' -f $Report.environment.freeAfterGb)
    & $add ('| Defender real-time | {0} |' -f $Report.environment.defenderRealTime)
    & $add ('| Исключения для staging/objects | {0} / {1} |' -f $Report.environment.stagingExcluded, $Report.environment.objectsExcluded)
    if ($Report.environment.defenderNote) {
        & $add ('| Defender note | {0} |' -f $Report.environment.defenderNote)
    }
    & $add ''

    & $add '## Том и размещение'
    & $add ''
    & $add '| Параметр | Значение |'
    & $add '|---|---|'
    & $add ('| Том | {0} (`{1}`) |' -f $Report.storage.volumeName, $Report.storage.rootPath)
    & $add ('| Staging | `{0}` |' -f $Report.storage.stagingPath)
    & $add ('| Objects | `{0}` |' -f $Report.storage.objectsPath)
    & $add ('| Один том | {0} |' -f $Report.storage.sameVolume)
    & $add ('| Стратегия фиксации | {0} |' -f $Report.storage.strategy)
    & $add ('| Требовалось места | {0} ГиБ + резерв {1} ГиБ = {2} ГиБ |' -f $Report.storage.requiredFreeGb, $Report.storage.headroomGb, $Report.storage.requiredWithHeadroomGb)
    & $add ''

    & $add '## Нагрузка'
    & $add ''
    & $add '| Параметр | Значение |'
    & $add '|---|---|'
    & $add ('| Запрошено | {0} ГиБ, {1} файлов |' -f $Report.parameters.SizeGb, $Report.parameters.FileCount)
    & $add ('| Загружено | {0} ГиБ ({1} байт), {2} файлов |' -f $Report.payload.uploadedGb, $Report.payload.uploadedBytes, $Report.payload.fileCount)
    & $add ('| Уникальных SHA-256 | {0} |' -f $Report.payload.distinctShaCount)
    & $add ('| Дубликатов SHA-256 | {0} |' -f $Report.payload.duplicateShaCount)
    & $add ('| Чанков отправлено | {0} |' -f $Report.payload.chunkRequests)
    & $add ('| Объектов добавлено на диск | {0} ({1} ГиБ) |' -f $Report.payload.objectFilesAdded, $Report.payload.objectsBytesAddedGb)
    & $add ('| Staging до / после / добавлено | {0} / {1} / {2} |' -f $Report.payload.stagingFilesBefore, $Report.payload.stagingFilesAfter, $Report.payload.stagingFilesAdded)
    & $add ''
    if ($Report.payload.diskUsageNote) {
        & $add ('> ' + $Report.payload.diskUsageNote)
        & $add ''
    }

    & $add '## Время и пропускная способность'
    & $add ''
    & $add '| Этап | Секунды |'
    & $add '|---|---:|'
    & $add ('| Предварительная проверка | {0} |' -f $Report.timings.preflightSeconds)
    & $add ('| Генерация + SHA + загрузка | {0} |' -f $Report.timings.uploadSeconds)
    & $add ('| Проверка сборки | {0} |' -f $Report.timings.validateSeconds)
    & $add ('| Релиз и манифест | {0} |' -f $Report.timings.releaseSeconds)
    & $add ('| Сверка SHA и Range | {0} |' -f $Report.timings.verifySeconds)
    & $add ('| **Всего** | **{0}** |' -f $Report.timings.totalSeconds)
    & $add ''
    & $add ('Throughput загрузки: **{0} МБ/с**, end-to-end: **{1} МБ/с**.' -f $Report.throughput.uploadMbPerSecond, $Report.throughput.endToEndMbPerSecond)
    & $add ''

    & $add '## Память процесса NEXUS'
    & $add ''
    if ($Report.memory.available) {
        & $add '| Метрика | МБ |'
        & $add '|---|---:|'
        & $add ('| Baseline working set | {0} |' -f $Report.memory.baselineWorkingSetMb)
        & $add ('| Baseline private | {0} |' -f $Report.memory.baselinePrivateMb)
        & $add ('| Peak working set | {0} |' -f $Report.memory.peakWorkingSetMb)
        & $add ('| Peak private | {0} |' -f $Report.memory.peakPrivateMb)
        & $add ('| Прирост private | {0} |' -f $Report.memory.privateDeltaMb)
        & $add ('| Порог | {0} |' -f $Report.memory.thresholdMb)
        & $add ('| Результат | {0} |' -f ($(if ($Report.memory.passed) { 'PASS' } else { 'FAIL' })))
        & $add ''
        & $add ('Замеров: {0}, интервал {1} с. Порог считается по private bytes.' -f $Report.memory.samples, $Report.parameters.SampleSeconds)
    } else {
        & $add ('Замер не выполнялся: {0}' -f $Report.memory.note)
    }
    & $add ''

    & $add '## Сверка SHA-256 и манифест'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    & $add ('| Клиент = сервер (все файлы) | {0} |' -f $Report.sha.clientVsServerMatched)
    & $add ('| Клиент = манифест (все файлы) | {0} |' -f $Report.sha.clientVsManifestMatched)
    & $add ('| Уникальных SHA в манифесте | {0} |' -f $Report.sha.distinctManifestShaCount)
    & $add ('| Манифест детерминирован (2 запроса) | {0} |' -f $Report.manifest.fetchedTwiceEqual)
    & $add ('| Манифест не изменился после публикации | {0} |' -f $Report.manifest.stableAfterPublish)
    & $add ('| Порядок файлов (ordinal) | {0} |' -f $Report.manifest.filesSortedOrdinal)
    & $add ('| Файлов в манифесте | {0} |' -f $Report.manifest.fileCount)
    & $add ('| Размер в манифесте | {0} ГиБ |' -f $Report.manifest.totalGb)
    & $add ''
    if ($Report.sha.mismatches.Count -gt 0) {
        & $add 'Несовпадения:'
        & $add ''
        foreach ($item in $Report.sha.mismatches) { & $add ('* `{0}`' -f $item) }
        & $add ''
    }

    & $add '## Выборочное скачивание и Range'
    & $add ''
    & $add '| Файл | Проверка | Статус | Байт | OK |'
    & $add '|---|---|---:|---:|---|'
    foreach ($test in $Report.range.tests) {
        & $add ('| `{0}` | {1} | {2} | {3} | {4} |' -f $test.path, $test.check, $test.status, $test.bytes, $test.ok)
    }
    & $add ''

    & $add '## Проверки'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    foreach ($key in $Report.assertions.Keys) {
        & $add ('| {0} | {1} |' -f $key, $Report.assertions[$key])
    }
    & $add ''

    if ($Report.blockers.Count -gt 0) {
        & $add '## Блокеры'
        & $add ''
        foreach ($blocker in $Report.blockers) { & $add ('* {0}' -f $blocker) }
        & $add ''
    }

    if ($Report.cleanup.requested) {
        & $add '## Очистка'
        & $add ''
        & $add ('Запрошена: да. Выполнена: {0}. Освобождено: {1} ГиБ.' -f $Report.cleanup.performed, $Report.cleanup.freedGb)
        & $add ''
    }

    & $add '## Артефакты прогона'
    & $add ''
    & $add ('* buildId: `{0}`' -f $Report.artifacts.buildId)
    & $add ('* slug: `{0}`' -f $Report.artifacts.buildSlug)
    & $add ('* releaseId: `{0}`' -f $Report.artifacts.releaseId)
    & $add ('* releaseVersion: `{0}`' -f $Report.artifacts.releaseVersion)
    & $add ('* JSON: `{0}`' -f $jsonPath)
    & $add ('* лог: `{0}`' -f $logPath)
    & $add ''
    & $add 'Полная временная серия памяти и машинные поля — в JSON рядом с этим отчётом.'

    $lines -join "`r`n" | Set-Content -LiteralPath $mdPath -Encoding UTF8
    Write-Host ''
    Write-Host ("Отчёт: {0}" -f $mdPath)
    Write-Host ("JSON:  {0}" -f $jsonPath)
    Write-Host ("Лог:   {0}" -f $logPath)
}

# ---------------------------------------------------------------------------
# Отчёт-заготовка
# ---------------------------------------------------------------------------
$dateStamp = (Get-Date).ToString('yyyyMMdd')
if ([string]::IsNullOrWhiteSpace($ReportName)) { $ReportName = "large-build-$dateStamp" }

$commandParts = @('tests/large-build.ps1')
foreach ($entry in $PSBoundParameters.GetEnumerator()) {
    $value = $entry.Value
    if ($entry.Key -eq 'Password') { $value = '***' }
    $commandParts += ('-{0} {1}' -f $entry.Key, $value)
}
$commandLine = $commandParts -join ' '

$os = Get-CimInstance Win32_OperatingSystem
$report = [ordered]@{
    phase = 'G2'
    generatedAt = (Get-Date).ToString('o')
    result = 'PENDING'
    command = $commandLine
    environment = [ordered]@{
        hostname = $env:COMPUTERNAME
        osCaption = $os.Caption
        osVersion = $os.Version
        osBuild = $os.BuildNumber
        powershell = $PSVersionTable.PSVersion.ToString()
        dotnet = [System.Environment]::Version.ToString()
        filesystem = ''
        driveLetter = ''
        driveType = ''
        physicalDisk = ''
        mediaType = ''
        busType = ''
        totalGb = 0
        freeBeforeGb = 0
        freeAfterUploadGb = 0
        freeAfterGb = 0
        defenderAvailable = $false
        defenderRealTime = 'unknown'
        exclusionPaths = @()
        preferenceAvailable = $false
        stagingExcluded = 'unknown'
        objectsExcluded = 'unknown'
        defenderNote = ''
    }
    storage = [ordered]@{
        volumeId = ''
        volumeName = ''
        rootPath = ''
        state = ''
        stagingPath = ''
        objectsPath = ''
        sameVolume = $false
        sameVolumeMethod = ''
        strategy = ''
        requiredFreeBytes = 0
        requiredFreeGb = 0
        requiredWithHeadroomGb = 0
        headroomGb = 0
        freeBytes = 0
        freeGb = 0
        payloadMultiplier = 1
    }
    parameters = [ordered]@{
        SizeGb = $SizeGb
        FileCount = $FileCount
        ChunkMb = $ChunkMb
        PrivateMemoryLimitMb = $PrivateMemoryLimitMb
        SampleSeconds = $SampleSeconds
        Cleanup = [bool]$Cleanup
        BaseUrl = $script:BaseUrl
    }
    payload = [ordered]@{
        requestedBytes = 0
        uploadedBytes = 0
        uploadedGb = 0
        fileCount = 0
        minFileBytes = 0
        maxFileBytes = 0
        avgFileBytes = 0
        distinctShaCount = 0
        duplicateShaCount = 0
        chunkRequests = 0
        objectFilesBefore = 0
        objectBytesBefore = 0
        objectFilesAfter = 0
        objectBytesAfter = 0
        objectFilesAdded = 0
        objectsBytesAdded = 0
        objectsBytesAddedGb = 0
        stagingFilesBefore = 0
        stagingFilesAfter = 0
        stagingFilesAdded = 0
        diskFreeDeltaBytes = 0
        diskUsageNote = ''
    }
    timings = [ordered]@{
        preflightSeconds = 0
        uploadSeconds = 0
        validateSeconds = 0
        releaseSeconds = 0
        verifySeconds = 0
        cleanupSeconds = 0
        totalSeconds = 0
    }
    throughput = [ordered]@{
        uploadMbPerSecond = 0
        endToEndMbPerSecond = 0
    }
    memory = [ordered]@{
        available = $false
        samples = 0
        note = 'Замер ещё не выполнялся.'
    }
    sha = [ordered]@{
        clientVsServerMatched = $false
        clientVsManifestMatched = $false
        manifestCount = 0
        distinctManifestShaCount = 0
        mismatches = @()
        sampleDownloads = @()
    }
    manifest = [ordered]@{
        fetchedTwiceEqual = $false
        stableAfterPublish = $false
        filesSortedOrdinal = $false
        schemaVersion = ''
        fileCount = 0
        totalBytes = 0
        totalGb = 0
        launcherMatchesAdmin = $false
    }
    range = [ordered]@{
        passed = $false
        tests = @()
    }
    artifacts = [ordered]@{
        buildId = ''
        buildSlug = ''
        releaseId = ''
        releaseVersion = ''
        launcherTokenId = ''
    }
    assertions = [ordered]@{}
    cleanup = [ordered]@{
        requested = [bool]$Cleanup
        performed = $false
        freedGb = 0
    }
    blockers = @()
}

$script:Client = New-HttpClient
$script:CsrfToken = $null
$exitCode = 0
$script:ReportsSaved = $false
$payloadRecords = New-Object System.Collections.Generic.List[object]

try {
    $preflightStart = Get-Date

    # --- 1. Аутентификация ---------------------------------------------------
    if ([string]::IsNullOrWhiteSpace($Password)) {
        throw 'Не задан пароль администратора: укажите -Password или переменную NEXUS_ADMIN_PASSWORD.'
    }
    Write-Log 'Вход администратора...'
    $login = Invoke-Api -Method POST -Path '/api/v1/auth/login' -Body @{ username = $Username; password = $Password }
    if (-not $login.success) { throw 'Не удалось войти: сервер вернул success=false.' }
    $script:CsrfToken = $login.data.csrfToken
    Write-Log ("Пользователь {0}, сессия до {1}" -f $login.data.user.username, $login.data.session.expiresAt)

    $settingsResponse = Invoke-Api -Method GET -Path '/api/v1/system/settings'
    $effective = $settingsResponse.data.effective
    $serverChunkMb = [int]$effective.'limits.chunk_max_mb'
    $maxUploadMb = [int]$effective.'limits.max_upload_mb'
    $quotaGib = [int]$effective.'limits.quota_minecraft_gb'

    # --- 2. Том --------------------------------------------------------------
    $volumes = @((Invoke-Api -Method GET -Path '/api/v1/storage/volumes').data)
    if ($volumes.Count -eq 0) {
        throw 'Тома хранения не зарегистрированы. Создайте том в /system/storage или укажите -VolumeRoot.'
    }

    $selectedVolume = $null
    if (-not [string]::IsNullOrWhiteSpace($VolumeRoot)) {
        $rootFull = [System.IO.Path]::GetFullPath($VolumeRoot).TrimEnd('\', '/')
        $selectedVolume = $volumes | Where-Object {
            ([System.IO.Path]::GetFullPath($_.rootPath).TrimEnd('\', '/')) -ieq $rootFull
        } | Select-Object -First 1
        if (-not $selectedVolume) {
            Write-Log ("Каталог {0} не зарегистрирован как том — создаю." -f $rootFull)
            if (-not (Test-Path -LiteralPath $rootFull)) {
                New-Item -ItemType Directory -Force -Path $rootFull | Out-Null
            }
            $created = Invoke-Api -Method POST -Path '/api/v1/storage/volumes' -Body @{
                name = ('G2 large build {0}' -f (Get-Date).ToString('yyyyMMdd-HHmmss'))
                path = $rootFull
                mode = 'auto'
                priority = 100
            }
            $selectedVolume = $created.data
        }
    } else {
        $selectedVolume = $volumes | Where-Object { $_.state -eq 'online' -and $_.enabled } |
            Sort-Object -Property availableBytes -Descending | Select-Object -First 1
        if (-not $selectedVolume) {
            throw 'Нет ни одного онлайн-тома. Включите том в /system/storage или укажите -VolumeRoot.'
        }
    }

    if ($selectedVolume.state -ne 'online') {
        throw ("Том {0} в состоянии {1}. Проверьте его в /system/storage перед прогоном." -f $selectedVolume.name, $selectedVolume.state)
    }

    $rootPath = [System.IO.Path]::GetFullPath($selectedVolume.rootPath).TrimEnd('\', '/')
    $stagingPath = Join-Path $rootPath '.nexus\staging'
    $objectsPath = Join-Path $rootPath '.nexus\objects'

    $drive = Get-DriveInfo $rootPath
    $physical = Get-PhysicalDiskInfo $drive.DeviceID

    $stagingVolume = Get-VolumeNameForPath $stagingPath
    $objectsVolume = Get-VolumeNameForPath $objectsPath
    $sameVolume = $false
    $sameVolumeMethod = 'GetVolumePathName'
    if ($stagingVolume -and $objectsVolume) {
        $sameVolume = ($stagingVolume.TrimEnd('\') -ieq $objectsVolume.TrimEnd('\'))
    } else {
        # На обоих путях один и тот же существующий корень — значит и том один.
        $sameVolume = $true
        $sameVolumeMethod = 'fallback: общий корень тома'
    }
    $strategy = if ($sameVolume) { 'rename' } else { 'copy' }
    $payloadMultiplier = if ($sameVolume) { 1 } else { 2 }

    # --- 3. Расчёт объёма и проверка места ----------------------------------
    $payloadBytes = [int64][math]::Round($SizeGb * $script:GiB)
    if ($payloadBytes -le 0) { throw 'SizeGb должен быть больше нуля.' }
    if ($FileCount -le 0) { throw 'FileCount должен быть больше нуля.' }
    $baseSize = [int64][math]::Floor($payloadBytes / $FileCount)
    if ($baseSize -lt 1024) { throw 'Слишком маленький файл: увеличьте SizeGb или уменьшите FileCount.' }
    $sizes = [int64[]]::new($FileCount)
    $assigned = [int64]0
    for ($i = 0; $i -lt $FileCount; $i++) {
        $sizes[$i] = $baseSize
        $assigned += $baseSize
    }
    $sizes[$FileCount - 1] += ($payloadBytes - $assigned)

    $largestFile = ($sizes | Measure-Object -Maximum).Maximum
    if ($largestFile -gt ([int64]$maxUploadMb * $script:MiB)) {
        throw ("Размер файла {0} МБ превышает limits.max_upload_mb={1}. Уменьшите -SizeGb или увеличьте лимит." -f `
            [math]::Round($largestFile / $script:MiB, 1), $maxUploadMb)
    }
    if ($payloadBytes -gt ([int64]$quotaGib * $script:GiB)) {
        throw ("Нагрузка {0} ГиБ превышает квоту Minecraft {1} ГиБ." -f (Format-Gb $payloadBytes), $quotaGib)
    }

    $overhead = [int64](512 * $script:MiB) + [int64]($payloadBytes * 0.02)
    $requiredBytes = ([int64]$payloadMultiplier * $payloadBytes) + $overhead
    $headroomBytes = [int64](5 * $script:GiB)
    $requiredWithHeadroom = $requiredBytes + $headroomBytes
    $freeBytes = $drive.FreeSpace

    $report.environment.filesystem = $drive.FileSystem
    $report.environment.driveLetter = $drive.DeviceID
    $report.environment.driveType = $drive.DriveType
    $report.environment.physicalDisk = $physical.FriendlyName
    $report.environment.mediaType = $physical.MediaType
    $report.environment.busType = $physical.BusType
    $report.environment.totalGb = Format-Gb $drive.Size
    $report.environment.freeBeforeGb = Format-Gb $freeBytes
    $report.storage.volumeId = $selectedVolume.id
    $report.storage.volumeName = $selectedVolume.name
    $report.storage.rootPath = $rootPath
    $report.storage.state = $selectedVolume.state
    $report.storage.stagingPath = $stagingPath
    $report.storage.objectsPath = $objectsPath
    $report.storage.sameVolume = $sameVolume
    $report.storage.sameVolumeMethod = $sameVolumeMethod
    $report.storage.strategy = $strategy
    $report.storage.requiredFreeBytes = $requiredBytes
    $report.storage.requiredFreeGb = Format-Gb $requiredBytes
    $report.storage.requiredWithHeadroomGb = Format-Gb $requiredWithHeadroom
    $report.storage.headroomGb = Format-Gb $headroomBytes
    $report.storage.freeBytes = $freeBytes
    $report.storage.freeGb = Format-Gb $freeBytes
    $report.storage.payloadMultiplier = $payloadMultiplier

    $defender = Get-DefenderInfo -Root $rootPath -Staging $stagingPath -Objects $objectsPath
    $report.environment.defenderAvailable = $defender.available
    $report.environment.defenderRealTime = if ($defender.available) { [string]$defender.realTimeProtectionEnabled } else { 'unavailable' }
    $report.environment.exclusionPaths = @($defender.exclusionPaths)
    $report.environment.preferenceAvailable = $defender.preferenceAvailable
    $report.environment.stagingExcluded = $defender.stagingExcluded
    $report.environment.objectsExcluded = $defender.objectsExcluded
    $report.environment.defenderNote = $defender.note

    Write-Log ("Том: {0} ({1})" -f $selectedVolume.name, $rootPath)
    Write-Log ("Место: свободно {0} ГиБ, нужно {1} ГиБ ({2}), запас {3} ГиБ" -f `
        (Format-Gb $freeBytes), (Format-Gb $requiredBytes), $strategy, (Format-Gb $headroomBytes))

    if ($freeBytes -lt $requiredWithHeadroom) {
        $deficit = $requiredWithHeadroom - $freeBytes
        $maxPayload = [int64](($freeBytes - $headroomBytes - [int64](512 * $script:MiB)) / (1.02 * $payloadMultiplier))
        $maxSizeGb = [math]::Floor($maxPayload / $script:GiB * 10) / 10
        $commandHint = 'powershell -File tests/large-build.ps1 -SizeGb {0} -FileCount {1} -VolumeRoot "{2}" -ReportDir {3} -Cleanup' -f `
            $SizeGb, $FileCount, $rootPath, $ReportDir
        $message = ("BLOCKED: на {0} свободно {1} ГиБ, а для {2} ГиБ нужно {3} ГиБ " +
            "(данные {4} ГиБ + служебный запас 2% + резерв 5 ГиБ). Не хватает {5} ГиБ. " +
            "Освободите место на {0} (том {6}) или уменьшите объём: -SizeGb не больше {7}. " +
            "Команда после освобождения места: {8}. " +
            "Ожидаемые поля отчёта после успешного прогона: environment.*, storage.strategy, " +
            "payload.uploadedBytes, sha.clientVsManifestMatched, memory.privateDeltaMb, " +
            "throughput.uploadMbPerSecond, manifest.fetchedTwiceEqual.") -f `
            $drive.DeviceID, (Format-Gb $freeBytes), $SizeGb, (Format-Gb $requiredWithHeadroom),
            (Format-Gb $payloadBytes), (Format-Gb $deficit), $rootPath, $maxSizeGb, $commandHint
        $report.result = 'BLOCKED'
        $report.environment.freeAfterUploadGb = Format-Gb $freeBytes
        $report.environment.freeAfterGb = Format-Gb $freeBytes
        $report.blockers = @($message)
        $report.assertions = [ordered]@{ 'Достаточно свободного места' = $false }
        $report.timings.preflightSeconds = [math]::Round(((Get-Date) - $preflightStart).TotalSeconds, 2)
        $report.timings.totalSeconds = $report.timings.preflightSeconds
        Save-Reports -Report $report -Directory $ReportDir -Name $ReportName
        $script:ReportsSaved = $true
        Write-Log $message
        exit 2
    }

    $objectsBefore = Measure-Tree $objectsPath
    $stagingBefore = Measure-Tree $stagingPath
    $report.payload.requestedBytes = $payloadBytes
    $report.payload.stagingFilesBefore = $stagingBefore.Files

    # --- 4. Процесс NEXUS и baseline памяти ---------------------------------
    $port = ([System.Uri]$script:BaseUrl).Port
    $nexus = Resolve-NexusProcess -Port $port
    if ($nexus) {
        $script:NexusProcessId = $nexus.Id
        $script:LastSampleAt = [DateTime]::MinValue
        [void](Get-MemorySample)
        $report.memory = Get-MemorySummary
        Write-Log ("Процесс NEXUS: PID {0}, private {1} МБ" -f $nexus.Id, $report.memory.baselinePrivateMb)
    } else {
        Write-Log 'Процесс NEXUS не найден: замер памяти будет пропущен.'
    }

    # --- 5. Сборка -----------------------------------------------------------
    $buildName = 'G2 large build {0}' -f (Get-Date).ToString('yyyyMMdd-HHmmss')
    Write-Log ("Создаю сборку {0}..." -f $buildName)
    $build = (Invoke-Api -Method POST -Path '/api/v1/minecraft/builds' -Body @{
        name = $buildName
        description = ('Автоматический прогон G2: {0} ГиБ, {1} файлов' -f $SizeGb, $FileCount)
        version = '1.0.0'
        category = 'other'
        visibility = 'private'
        minecraftVersion = '1.20.1'
        loaderType = 'vanilla'
        loaderVersion = ''
        javaMajor = 21
        javaMin = 21
        javaNotes = 'G2 load test'
    }).data
    $report.artifacts.buildId = $build.id
    $report.artifacts.buildSlug = $build.slug
    Write-Log ("Сборка: {0} ({1})" -f $build.slug, $build.id)

    $chunkBytes = [int64]([math]::Min($ChunkMb, $serverChunkMb) * $script:MiB)
    $chunkBytes = [int64]([math]::Max(1, $chunkBytes))
    $buffer = [byte[]]::new($chunkBytes)
    $runId = [guid]::NewGuid()

    $preflightSeconds = [math]::Round(((Get-Date) - $preflightStart).TotalSeconds, 2)
    $report.timings.preflightSeconds = $preflightSeconds
    Write-Log ("Старт загрузки: {0} ГиБ, {1} файлов, чанк {2} МиБ, runId {3}" -f `
        (Format-Gb $payloadBytes), $FileCount, ($chunkBytes / $script:MiB), $runId)

    $uploadStart = Get-Date
    $uploadedBytes = [int64]0
    $clientShas = New-Object 'System.Collections.Generic.HashSet[string]'
    $totalChunks = 0

    for ($index = 0; $index -lt $FileCount; $index++) {
        $size = $sizes[$index]
        $relativePath = 'mods/g2-{0:D4}.jar' -f $index
        $sessionId = $null
        $fileRecord = $null
        $fileRestarts = 0

        while ($null -eq $fileRecord) {
            try {
                $session = (Invoke-Api -Method POST -Path '/api/v1/minecraft/uploads' -Body @{
                    buildId = $build.id
                    path = $relativePath
                    size = $size
                    category = 'mods'
                    contentType = 'application/octet-stream'
                }).data
                $sessionId = $session.id
                if ([int64]$session.declaredSize -ne $size) {
                    throw ("Сессия объявила размер {0}, ожидался {1}." -f $session.declaredSize, $size)
                }

                $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)
                $generator = New-Object NexusChunkGenerator -ArgumentList $runId, $index, $size
                $offset = [int64]0
                while ($offset -lt $size) {
                    $count = [int][math]::Min([int64]$buffer.Length, $size - $offset)
                    $generator.Fill($buffer, 0, $count, $offset)
                    $hasher.AppendData($buffer, 0, $count)
                    $chunk = [byte[]]::new($count)
                    [System.Array]::Copy($buffer, 0, $chunk, 0, $count)

                    $chunkResponse = Invoke-Api -Method PATCH -Path ("/api/v1/minecraft/uploads/{0}" -f $sessionId) `
                        -Bytes $chunk -Headers @{ 'Upload-Offset' = [string]$offset } -MaxAttempts 6
                    $newOffset = [int64]$chunkResponse.data.offset
                    if ($newOffset -ne ($offset + $count)) {
                        throw ("Сервер сообщил offset {0}, ожидался {1}." -f $newOffset, ($offset + $count))
                    }
                    $offset = $newOffset
                    $script:ChunkRequests++
                    $totalChunks++
                    Invoke-MemorySampleIfDue
                }

                $localSha = ConvertTo-HexString ($hasher.GetHashAndReset())
                $idempotencyKey = [guid]::NewGuid().ToString()
                $fileRecord = $null
                $completeErrors = New-Object System.Collections.Generic.List[string]
                for ($completeAttempt = 1; $completeAttempt -le 3 -and $null -eq $fileRecord; $completeAttempt++) {
                    try {
                        $completeResponse = Invoke-Api -Method POST -Path ("/api/v1/minecraft/uploads/{0}/complete" -f $sessionId) `
                            -Body @{} -Headers @{ 'Idempotency-Key' = $idempotencyKey } -MaxAttempts 2
                        $fileRecord = $completeResponse.data
                    } catch {
                        $completeErrors.Add($_.Exception.Message)
                        # 500 мог прийти после фактической фиксации: проверяем файл по пути.
                        $existing = Find-BuildFileByPath -BuildId $build.id -Path $relativePath
                        if ($existing -and [string]$existing.sha256 -eq $localSha) {
                            $fileRecord = $existing
                            Write-Log ("  файл {0}: complete вернул ошибку, но файл уже зафиксирован" -f $relativePath)
                        } elseif ($completeAttempt -lt 3) {
                            Start-Sleep -Milliseconds (400 * $completeAttempt)
                        }
                    }
                }
                if ($null -eq $fileRecord) {
                    throw ("complete не удался после 3 попыток: {0}" -f ($completeErrors -join ' | '))
                }
                if ([string]$fileRecord.sha256 -ne $localSha) {
                    throw ("SHA не совпал для {0}: клиент {1}, сервер {2}." -f $relativePath, $localSha, $fileRecord.sha256)
                }
                if ([int64]$fileRecord.sizeBytes -ne $size) {
                    throw ("Размер не совпал для {0}: клиент {1}, сервер {2}." -f $relativePath, $size, $fileRecord.sizeBytes)
                }

                $payloadRecords.Add([pscustomobject]@{
                    Path = $relativePath
                    Size = $size
                    Sha = $localSha
                    ServerSha = [string]$fileRecord.sha256
                    FileId = [string]$fileRecord.id
                    Chunks = [int][math]::Ceiling($size / [double]$chunkBytes)
                })
                [void]$clientShas.Add($localSha)
                $uploadedBytes += $size
                $sessionId = $null
            } catch {
                $message = $_.Exception.Message
                if ($sessionId) {
                    try { [void](Invoke-Api -Method DELETE -Path ("/api/v1/minecraft/uploads/{0}" -f $sessionId) -AllowError) } catch { }
                }
                $fileRestarts++
                if ($fileRestarts -gt 3) {
                    throw ("Файл {0} не загрузился после {1} попыток: {2}" -f $relativePath, $fileRestarts, $message)
                }
                Write-Log ("  файл {0}: повтор {1}/3 после ошибки: {2}" -f $relativePath, $fileRestarts, $message)
                Start-Sleep -Milliseconds (500 * $fileRestarts)
            }
        }

        Invoke-MemorySampleIfDue
        $done = $index + 1
        if ($done -eq $FileCount -or ($done % [math]::Max(1, [int]($FileCount / 20))) -eq 0) {
            $elapsed = ((Get-Date) - $uploadStart).TotalSeconds
            $rate = if ($elapsed -gt 0) { ($uploadedBytes / $script:MiB) / $elapsed } else { 0 }
            $privateNow = if ($script:Samples.Count -gt 0) { $script:Samples[$script:Samples.Count - 1].PrivateMb } else { 0 }
            Write-Log ("  {0}/{1} файлов, {2} ГиБ, {3:N1} МБ/с, private {4} МБ" -f `
                $done, $FileCount, (Format-Gb $uploadedBytes), $rate, $privateNow)
        }
    }

    $report.timings.uploadSeconds = [math]::Round(((Get-Date) - $uploadStart).TotalSeconds, 2)
    Write-Log ("Загрузка завершена за {0} с: {1} ГиБ, {2} чанков" -f `
        $report.timings.uploadSeconds, (Format-Gb $uploadedBytes), $totalChunks)

    # --- 6. Проверка сборки --------------------------------------------------
    $validateStart = Get-Date
    Write-Log 'Проверка сборки...'
    $validation = (Invoke-Api -Method POST -Path ("/api/v1/minecraft/builds/{0}/validate" -f $build.id)).data
    $report.timings.validateSeconds = [math]::Round(((Get-Date) - $validateStart).TotalSeconds, 2)
    if (-not $validation.valid) {
        $errors = @($validation.errors | ForEach-Object { $_.code }) -join ', '
        throw ("Проверка сборки не пройдена: {0}" -f $errors)
    }
    if ([int64]$validation.fileCount -ne $FileCount) {
        throw ("Проверка видит {0} файлов, ожидалось {1}." -f $validation.fileCount, $FileCount)
    }
    if ([int64]$validation.totalBytes -ne $uploadedBytes) {
        throw ("Проверка видит {0} байт, ожидалось {1}." -f $validation.totalBytes, $uploadedBytes)
    }

    # --- 7. Релиз и манифест -------------------------------------------------
    $releaseStart = Get-Date
    $releaseVersion = '1.0.{0}' -f [int][double]::Parse((Get-Date).ToString('HHmmss'))
    # Версия должна быть уникальной в рамках сборки; новая сборка создаётся каждый прогон.
    $release = (Invoke-Api -Method POST -Path ("/api/v1/minecraft/builds/{0}/releases" -f $build.id) -Body @{
        version = $releaseVersion
        changelog = ('G2: {0} ГиБ, {1} файлов' -f $SizeGb, $FileCount)
    }).data
    $report.artifacts.releaseId = $release.id
    $report.artifacts.releaseVersion = $release.version
    Write-Log ("Релиз {0} создан ({1})" -f $release.version, $release.id)

    $manifestPath = "/api/v1/minecraft/builds/{0}/releases/{1}/manifest" -f $build.id, $release.id
    $manifestFirstResponse = Invoke-Api -Method GET -Path $manifestPath
    $manifestSecondResponse = Invoke-Api -Method GET -Path $manifestPath
    $manifestFirst = $manifestFirstResponse.data
    $manifestSecond = $manifestSecondResponse.data
    $manifestFirstRaw = $manifestFirst | ConvertTo-Json -Depth 20 -Compress
    $manifestSecondRaw = $manifestSecond | ConvertTo-Json -Depth 20 -Compress
    $report.manifest.fetchedTwiceEqual = ($manifestFirstRaw -ceq $manifestSecondRaw)
    $report.manifest.schemaVersion = [string]$manifestFirst.schema_version
    $report.manifest.fileCount = @($manifestFirst.files).Count
    $report.manifest.totalBytes = [int64](($manifestFirst.files | Measure-Object -Property size -Sum).Sum)
    $report.manifest.totalGb = Format-Gb $report.manifest.totalBytes

    # Порядок файлов — байтовая (ordinal) сортировка по path.
    $sorted = $true
    $files = @($manifestFirst.files)
    for ($i = 1; $i -lt $files.Count; $i++) {
        if ([string]::CompareOrdinal([string]$files[$i - 1].path, [string]$files[$i].path) -ge 0) {
            $sorted = $false
            break
        }
    }
    $report.manifest.filesSortedOrdinal = $sorted

    # Все SHA и размеры из манифеста против клиентских.
    $manifestByPath = @{}
    foreach ($file in $files) {
        $manifestByPath[([string]$file.path).ToLowerInvariant()] = $file
    }
    $manifestMismatches = New-Object System.Collections.Generic.List[string]
    $manifestShas = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($record in $payloadRecords) {
        $key = $record.Path.ToLowerInvariant()
        if (-not $manifestByPath.ContainsKey($key)) {
            $manifestMismatches.Add(('нет в манифесте: {0}' -f $record.Path))
            continue
        }
        $entry = $manifestByPath[$key]
        if ([string]$entry.sha256 -ne $record.Sha) {
            $manifestMismatches.Add(('SHA: {0} клиент={1} манифест={2}' -f $record.Path, $record.Sha, $entry.sha256))
        }
        if ([int64]$entry.size -ne $record.Size) {
            $manifestMismatches.Add(('размер: {0} клиент={1} манифест={2}' -f $record.Path, $record.Size, $entry.size))
        }
        [void]$manifestShas.Add([string]$entry.sha256)
    }
    $report.sha.clientVsServerMatched = $true
    $report.sha.clientVsManifestMatched = ($manifestMismatches.Count -eq 0)
    $report.sha.manifestCount = @($files).Count
    $report.sha.distinctManifestShaCount = $manifestShas.Count
    $report.sha.mismatches = $manifestMismatches.ToArray()

    # Публикация и проверка, что сохранённый манифест не изменился.
    Write-Log 'Публикация релиза...'
    $published = (Invoke-Api -Method POST -Path ("/api/v1/minecraft/builds/{0}/releases/{1}/publish" -f $build.id, $release.id) -Body @{
        acceptWarnings = $true
    }).data
    $manifestAfterPublish = (Invoke-Api -Method GET -Path $manifestPath).data
    $manifestAfterRaw = $manifestAfterPublish | ConvertTo-Json -Depth 20 -Compress
    $report.manifest.stableAfterPublish = ($manifestFirstRaw -ceq $manifestAfterRaw)
    $report.timings.releaseSeconds = [math]::Round(((Get-Date) - $releaseStart).TotalSeconds, 2)
    Write-Log ("Релиз опубликован: {0}" -f $published.status)

    # --- 8. Лаунчер: манифест и выборочное скачивание -----------------------
    $verifyStart = Get-Date
    Write-Log 'Выдаю токен лаунчера и сверяю манифест...'
    $tokenResponse = Invoke-Api -Method POST -Path '/api/v1/launcher/auth/token' -Body @{
        name = 'g2-large-build'
        username = $Username
        password = $Password
        scopes = @('launcher.catalog', 'launcher.artifacts')
        expiresInHours = 1
    }
    $launcherToken = $tokenResponse.data.token
    $authHeader = @{ Authorization = ('Bearer {0}' -f $launcherToken) }

    $launcherManifest = (Invoke-Api -Method GET -Path ("/api/v1/launcher/builds/{0}/releases/{1}/manifest" -f $build.slug, $release.version) -Headers $authHeader).data
    $launcherFiles = @($launcherManifest.files)
    $launcherMatches = ($launcherFiles.Count -eq $files.Count)
    if ($launcherMatches) {
        $launcherByPath = @{}
        foreach ($file in $launcherFiles) { $launcherByPath[([string]$file.path).ToLowerInvariant()] = $file }
        foreach ($record in $payloadRecords) {
            $key = $record.Path.ToLowerInvariant()
            if (-not $launcherByPath.ContainsKey($key)) { $launcherMatches = $false; break }
            if ([string]$launcherByPath[$key].sha256 -ne $record.Sha) { $launcherMatches = $false; break }
            if ([int64]$launcherByPath[$key].size -ne $record.Size) { $launcherMatches = $false; break }
        }
    }
    $report.manifest.launcherMatchesAdmin = $launcherMatches

    # Выбираем 3 файла: первый, средний и последний.
    $sampleIndexes = @(0, [int][math]::Floor(($FileCount - 1) / 2), ($FileCount - 1)) | Select-Object -Unique
    $rangeTests = New-Object System.Collections.Generic.List[object]
    $sampleDownloads = New-Object System.Collections.Generic.List[object]

    foreach ($sampleIndex in $sampleIndexes) {
        $record = $payloadRecords[$sampleIndex]
        $entry = $manifestByPath[$record.Path.ToLowerInvariant()]
        $artifactUrl = $script:BaseUrl + [string]$entry.download_url

        # Полное скачивание потоком и SHA по факту байтов.
        $request = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Get, $artifactUrl)
        [void]$request.Headers.TryAddWithoutValidation('Authorization', ('Bearer {0}' -f $launcherToken))
        $response = $script:Client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        $status = [int]$response.StatusCode
        $stream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        $downloadHasher = [System.Security.Cryptography.IncrementalHash]::CreateHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)
        $downloadBuffer = [byte[]]::new(1 * $script:MiB)
        $downloaded = [int64]0
        while (($read = $stream.Read($downloadBuffer, 0, $downloadBuffer.Length)) -gt 0) {
            $downloadHasher.AppendData($downloadBuffer, 0, $read)
            $downloaded += $read
        }
        $downloadSha = ConvertTo-HexString ($downloadHasher.GetHashAndReset())
        $stream.Dispose()
        $response.Dispose()
        $request.Dispose()
        $fullOk = ($status -eq 200 -and $downloaded -eq $record.Size -and $downloadSha -eq $record.Sha)
        $rangeTests.Add([pscustomobject]@{
            path = $record.Path
            check = 'full download + SHA'
            status = $status
            bytes = $downloaded
            ok = $fullOk
        })
        $sampleDownloads.Add([pscustomobject]@{
            path = $record.Path
            sha = $downloadSha
            expected = $record.Sha
            bytes = $downloaded
            ok = $fullOk
        })

        # Range: начало, середина, хвост.
        $mid = [int64][math]::Floor($record.Size / 2) - (512 * $script:KiB)
        if ($mid -lt 0) { $mid = 0 }
        $ranges = @(
            @{ Label = 'range 0..1MiB-1'; Value = 'bytes=0-1048575'; Expect = [int64](1 * $script:MiB) },
            @{ Label = 'range middle 1MiB'; Value = ('bytes={0}-{1}' -f $mid, ($mid + (1 * $script:MiB) - 1)); Expect = [int64](1 * $script:MiB) },
            @{ Label = 'range suffix 4096'; Value = 'bytes=-4096'; Expect = [int64]4096 }
        )
        foreach ($range in $ranges) {
            $rangeRequest = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Get, $artifactUrl)
            [void]$rangeRequest.Headers.TryAddWithoutValidation('Authorization', ('Bearer {0}' -f $launcherToken))
            [void]$rangeRequest.Headers.TryAddWithoutValidation('Range', $range.Value)
            $rangeResponse = $script:Client.SendAsync($rangeRequest, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
            $rangeStatus = [int]$rangeResponse.StatusCode
            $rangeStream = $rangeResponse.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $rangeBuffer = [byte[]]::new(65536)
            $rangeBytes = [int64]0
            while (($read = $rangeStream.Read($rangeBuffer, 0, $rangeBuffer.Length)) -gt 0) { $rangeBytes += $read }
            $rangeStream.Dispose()
            $rangeResponse.Dispose()
            $rangeRequest.Dispose()
            $rangeOk = ($rangeStatus -eq 206 -and $rangeBytes -eq $range.Expect)
            $rangeTests.Add([pscustomobject]@{
                path = $record.Path
                check = $range.Label
                status = $rangeStatus
                bytes = $rangeBytes
                ok = $rangeOk
            })
        }
        Invoke-MemorySampleIfDue
    }

    $report.sha.sampleDownloads = $sampleDownloads.ToArray()
    $report.range.tests = $rangeTests.ToArray()
    $report.range.passed = -not ($rangeTests | Where-Object { -not $_.ok })
    $report.timings.verifySeconds = [math]::Round(((Get-Date) - $verifyStart).TotalSeconds, 2)

    # --- 9. Фактический объём на диске --------------------------------------
    $objectsAfter = Measure-Tree $objectsPath
    $stagingAfter = Measure-Tree $stagingPath
    $freeAfterUpload = (Get-DriveInfo $rootPath).FreeSpace
    $report.payload.objectFilesBefore = $objectsBefore.Files
    $report.payload.objectBytesBefore = $objectsBefore.Bytes
    $report.payload.objectFilesAfter = $objectsAfter.Files
    $report.payload.objectBytesAfter = $objectsAfter.Bytes
    $report.payload.objectFilesAdded = $objectsAfter.Files - $objectsBefore.Files
    $report.payload.objectsBytesAdded = $objectsAfter.Bytes - $objectsBefore.Bytes
    $report.payload.objectsBytesAddedGb = Format-Gb $report.payload.objectsBytesAdded
    $report.payload.stagingFilesAfter = $stagingAfter.Files
    $report.payload.stagingFilesAdded = $stagingAfter.Files - $stagingBefore.Files
    $report.payload.diskFreeDeltaBytes = $freeBytes - $freeAfterUpload
    $report.environment.freeAfterUploadGb = Format-Gb $freeAfterUpload
    $report.environment.freeAfterGb = Format-Gb $freeAfterUpload
    if ($uploadedBytes -gt 0 -and [math]::Abs([double]$report.payload.diskFreeDeltaBytes - [double]$uploadedBytes) -gt ([double]$uploadedBytes * 0.05)) {
        $report.payload.diskUsageNote = ('Дельта свободного места ({0} ГиБ) отличается от полезной нагрузки ({1} ГиБ) более чем на 5%. ' +
            'Во время прогона сторонние процессы могли освобождать или занимать диск. ' +
            'Фактический объём подтверждён суммой длин файлов в objects: {2} байт и {3} объектов.') -f `
            (Format-Gb ([int64]$report.payload.diskFreeDeltaBytes)), (Format-Gb $uploadedBytes), $report.payload.objectsBytesAdded, $report.payload.objectFilesAdded
    }
    $report.payload.uploadedBytes = $uploadedBytes
    $report.payload.uploadedGb = Format-Gb $uploadedBytes
    $report.payload.fileCount = $payloadRecords.Count
    $report.payload.minFileBytes = ($sizes | Measure-Object -Minimum).Minimum
    $report.payload.maxFileBytes = ($sizes | Measure-Object -Maximum).Maximum
    $report.payload.avgFileBytes = [int64][math]::Round(($sizes | Measure-Object -Average).Average)
    $report.payload.distinctShaCount = $clientShas.Count
    $report.payload.duplicateShaCount = $payloadRecords.Count - $clientShas.Count
    $report.payload.chunkRequests = $totalChunks

    # --- 10. Итоговые проверки ----------------------------------------------
    $diskMatches = ([math]::Abs($report.payload.objectsBytesAdded - $uploadedBytes) -le (16 * $script:MiB))
    $uniqueOk = ($clientShas.Count -eq $FileCount -and $manifestShas.Count -eq $FileCount)
    $stagingOk = ($report.payload.stagingFilesAdded -eq 0)

    $report.memory = Get-MemorySummary
    $report.assertions = [ordered]@{
        'Все файлы загружены' = ($payloadRecords.Count -eq $FileCount)
        'Контент уникален (SHA)' = $uniqueOk
        'Клиент = сервер = манифест' = ($report.sha.clientVsServerMatched -and $report.sha.clientVsManifestMatched)
        'Фактический объём на диске совпадает' = $diskMatches
        'Staging не содержит остатков' = $stagingOk
        'Манифест детерминирован' = ($report.manifest.fetchedTwiceEqual -and $report.manifest.stableAfterPublish -and $report.manifest.filesSortedOrdinal)
        'Лаунчер видит тот же манифест' = $report.manifest.launcherMatchesAdmin
        'Range и полное скачивание' = $report.range.passed
        'Память в пределах порога' = ($report.memory.available -and $report.memory.passed)
        'Сборка проверена и опубликована' = ($validation.valid -and $published.status -eq 'published')
    }

    $failedAssertions = @($report.assertions.Keys | Where-Object { -not $report.assertions[$_] })
    if ($failedAssertions.Count -gt 0) {
        $report.result = 'FAIL'
        $report.blockers = @($failedAssertions | ForEach-Object { 'Проверка не пройдена: ' + $_ })
        $exitCode = 1
    } else {
        $report.result = 'PASS'
        $exitCode = 0
    }

    # --- 11. Очистка ---------------------------------------------------------
    if ($Cleanup) {
        $cleanupStart = Get-Date
        Write-Log 'Очистка: архивирую релиз и удаляю сборку...'
        try {
            [void](Invoke-Api -Method POST -Path ("/api/v1/minecraft/builds/{0}/releases/{1}/archive" -f $build.id, $release.id))
            [void](Invoke-Api -Method DELETE -Path ("/api/v1/minecraft/builds/{0}" -f $build.id))
            Start-Sleep -Seconds 2
            $report.cleanup.performed = $true
            $objectsAfterCleanup = Measure-Tree $objectsPath
            $report.cleanup.freedGb = Format-Gb ($objectsAfter.Bytes - $objectsAfterCleanup.Bytes)
        } catch {
            Write-Log ("  очистка не удалась: {0}" -f $_.Exception.Message)
        }
        $report.timings.cleanupSeconds = [math]::Round(((Get-Date) - $cleanupStart).TotalSeconds, 2)
    }
} catch {
    $report.result = 'FAIL'
    $report.blockers = @($report.blockers) + @($_.Exception.Message)
    Write-Log ("ОШИБКА: {0}" -f $_.Exception.Message)
    Write-Host $_.ScriptStackTrace
    $exitCode = 1
} finally {
    try {
        $report.memory = Get-MemorySummary
        $report.timings.totalSeconds = [math]::Round(((Get-Date) - $script:StartedAt).TotalSeconds, 2)
        if ($report.timings.uploadSeconds -gt 0) {
            $report.throughput.uploadMbPerSecond = [math]::Round(($report.payload.uploadedBytes / $script:MiB) / $report.timings.uploadSeconds, 2)
            $report.throughput.endToEndMbPerSecond = [math]::Round(($report.payload.uploadedBytes / $script:MiB) / $report.timings.totalSeconds, 2)
        }
        if ($report.storage.rootPath) {
            try {
                $report.environment.freeAfterGb = Format-Gb (Get-DriveInfo $report.storage.rootPath).FreeSpace
            } catch {
                $report.environment.freeAfterGb = $report.environment.freeAfterUploadGb
            }
        } elseif (-not $report.environment.freeAfterGb) {
            $report.environment.freeAfterGb = $report.environment.freeBeforeGb
        }
        if (-not $script:ReportsSaved) {
            Save-Reports -Report $report -Directory $ReportDir -Name $ReportName
            $script:ReportsSaved = $true
        }
    } catch {
        Write-Host ("Не удалось сохранить отчёт: {0}" -f $_.Exception.Message)
        Write-Host $_.ScriptStackTrace
    }
    if ($script:Client) { $script:Client.Dispose() }
}

exit $exitCode
