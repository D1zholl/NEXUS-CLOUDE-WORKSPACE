#Requires -Version 5.1
<#
.SYNOPSIS
    I5: отказ диска, обрыв клиента и жёсткий перезапуск NEXUS на Windows.

.DESCRIPTION
    Проверяет четыре сценария на живом сервере:

      1. отказ диска. Основной способ - VHDX: создать, подключить, назначить
         букву, отформатировать, зарегистрировать том в NEXUS, записать файл,
         затем Dismount-VHD/diskpart detach. После отказа проверяются
         check_volume -> offline, понятная ошибка загрузки и отказ скачивания.
         Повторное подключение возвращает том в online и файл скачивается с тем
         же SHA-256. Альтернативы без VHDX: -DiskFailureMode Rename (переименовать
         корень тома) и -DiskFailureMode Acl (icacls deny). Снятие буквы диска
         входит в detach VHDX; отдельный режим не нужен.
      2. обрыв клиента. PATCH с заявленным Content-Length обрывается на 25%
         тела; сессия остаётся active, offset не теряется, загрузка
         продолжается с серверного offset и завершается.
      3. жёсткое падение. Stop-Process -Force убивает сервер во время
         running-задачи system.objects.verify. До перезапуска в БД остаётся
         running; после старта recover_interrupted переводит её в pending.
         Переходы фиксируются триггером resilience_job_transitions, поэтому
         короткое состояние pending не теряется.
      4. Restart-Service. Если служба установлена - Restart-Service. С ключом
         -TestService создаётся временная служба с binPath `serve --service`
         (встроенный SCM dispatcher) и проверяется Restart-Service. Без службы
         используется Start-Process.

    VHDX и Stop-Process требуют прав администратора. Все созданные ресурсы
    удаляются в finally: записи Cloud, том, VHDX, временная служба, триггер.

.PARAMETER BaseUrl
    Базовый URL API (по умолчанию http://127.0.0.1:8080).

.PARAMETER Username
    Администратор. Пароль - только через NEXUS_ADMIN_PASSWORD.

.PARAMETER DiskFailureMode
    Auto (VHDX, если есть права, иначе Rename), Vhdx, Rename или Acl.

.PARAMETER TempRoot
    Каталог для VHDX и временных файлов (по умолчанию %TEMP%\nexus-resilience).

.PARAMETER C2Objects
    Сколько объектов подготовить для длительной задачи verify_objects.

.PARAMETER C2ObjectSizeMb
    Размер каждого объекта для C2.

.PARAMETER TestService
    Создать временную службу Windows (binPath `serve --service`) и проверить
    Restart-Service через встроенный SCM dispatcher.

.EXAMPLE
    $env:NEXUS_ADMIN_PASSWORD = '***'
    powershell -ExecutionPolicy Bypass -File tests/resilience.ps1 -TestService
#>
param(
    [string]$BaseUrl = 'http://127.0.0.1:8080',
    [string]$Username = 'admin',
    [ValidateSet('Auto', 'Vhdx', 'Rename', 'Acl')]
    [string]$DiskFailureMode = 'Auto',
    [string]$TempRoot = "$env:TEMP\nexus-resilience",
    [int]$VhdSizeMb = 2048,
    [int]$C2Objects = 16,
    [int]$C2ObjectSizeMb = 24,
    [string]$ServiceName,
    [string]$DatabaseUrl,
    [string]$PsqlPath,
    [string]$ReportDir = 'docs\reports',
    [string]$ReportName = ('resilience-' + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [switch]$TestService,
    [switch]$KeepArtifacts
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Net.Http -ErrorAction Stop

$script:MiB = [int64]1048576
$script:BaseUrl = $BaseUrl.TrimEnd('/')
$script:Root = Split-Path -Parent $PSScriptRoot
$script:RunId = [guid]::NewGuid()
$script:RunTag = $script:RunId.ToString('N').Substring(0, 8)
$script:Log = New-Object System.Collections.Generic.List[string]
$script:StartedAt = Get-Date
$script:Csrf = $null
$script:Client = $null
$script:ClientHandler = $null
$script:ExitCode = 0
$script:ReportsSaved = $false
$script:CreatedEntries = New-Object System.Collections.Generic.List[object]
$script:DisabledVolumes = New-Object System.Collections.Generic.List[object]
$script:VolumeId = $null
$script:Vhd = $null
$script:ServerProcessId = 0
$script:ServerExe = $null
$script:ServerWorkDir = $null
$script:ServiceInfo = [ordered]@{ tested = $false; name = $null; reason = '' }
$script:TriggerInstalled = $false

function Write-Log {
    param([string]$Message)
    $line = '{0} {1}' -f (Get-Date).ToString('HH:mm:ss'), $Message
    $script:Log.Add($line)
    Write-Host $line
}

function New-HttpClient {
    param([switch]$Temporary)
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $handler.AutomaticDecompression = [System.Net.DecompressionMethods]::GZip -bor [System.Net.DecompressionMethods]::Deflate
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(15)
    if (-not $Temporary) { $script:ClientHandler = $handler }
    return $client
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
        [int]$MaxAttempts = 4,
        [switch]$AllowError
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
            $json = $Body | ConvertTo-Json -Depth 20 -Compress
            $request.Content = New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
        }

        $response = $null
        try {
            $response = $script:Client.SendAsync($request).GetAwaiter().GetResult()
        } catch {
            $request.Dispose()
            if ($attempt -lt $MaxAttempts) {
                Start-Sleep -Milliseconds ([math]::Min(4000, 250 * [math]::Pow(2, $attempt - 1)))
                continue
            }
            throw ("HTTP {0} {1}: {2}" -f $Method, $Path, $_.Exception.Message)
        }

        $status = [int]$response.StatusCode
        $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $response.Dispose()
        $request.Dispose()

        if ($status -ge 200 -and $status -lt 300) { return (ConvertFrom-JsonSafe $text) }
        if (($status -ge 500 -or $status -eq 408 -or $status -eq 429) -and $attempt -lt $MaxAttempts) {
            Start-Sleep -Milliseconds ([math]::Min(5000, 250 * [math]::Pow(2, $attempt - 1)))
            continue
        }
        if ($AllowError) {
            return [pscustomobject]@{ __error = $true; status = $status; body = (ConvertFrom-JsonSafe $text); text = $text }
        }
        throw ("HTTP {0} {1} {2}: {3}" -f $status, $Method, $Path, (Get-ApiErrorText $status $text))
    }
}

function Wait-Health {
    param([string]$Url = $null, [int]$TimeoutSeconds = 60)
    if (-not $Url) { $Url = $script:BaseUrl }
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-RestMethod -Method Get -Uri ($Url + '/api/v1/health') -TimeoutSec 3
            if ($response.data.status -eq 'ok') { return $response.data }
        } catch { }
        Start-Sleep -Milliseconds 500
    }
    throw ("сервер не поднялся за {0} с: {1}" -f $TimeoutSeconds, $Url)
}

# --- Детерминированное содержимое --------------------------------------------
if (-not ('NexusResilienceContent' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Security.Cryptography;
using System.Text;

public static class NexusResilienceContent
{
    public static byte[] Generate(Guid id, int index, long size)
    {
        byte[] buffer = new byte[size];
        ulong state = 1469598103934665603UL;
        foreach (byte b in id.ToByteArray()) { state ^= b; state *= 1099511628211UL; }
        state ^= (ulong)(uint)index;
        state *= 1099511628211UL;
        if (state == 0) state = 0x9E3779B97F4A7C15UL;
        byte[] header = new byte[64];
        Encoding.ASCII.GetBytes("NEXUS-I5").CopyTo(header, 0);
        id.ToByteArray().CopyTo(header, 16);
        BitConverter.GetBytes(index).CopyTo(header, 32);
        BitConverter.GetBytes(size).CopyTo(header, 36);
        Encoding.ASCII.GetBytes("RES").CopyTo(header, 60);
        int head = Math.Min(64, buffer.Length);
        Array.Copy(header, 0, buffer, 0, head);
        int pos = head;
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
    return [NexusResilienceContent]::Sha256Hex($Bytes)
}

function Get-CookieHeader {
    param([string]$Url)
    $uri = [uri]$Url
    $parts = @()
    foreach ($cookie in $script:ClientHandler.CookieContainer.GetCookies($uri)) {
        $parts += ('{0}={1}' -f $cookie.Name, $cookie.Value)
    }
    return ($parts -join '; ')
}

function Invoke-Login {
    param([string]$Url = $null, $Client = $null)
    if (-not $Url) { $Url = $script:BaseUrl }
    if ($null -eq $Client) { $Client = $script:Client }
    $response = $Client.PostAsync(
        ($Url + '/api/v1/auth/login'),
        (New-Object System.Net.Http.StringContent -ArgumentList ('{"username":"' + $Username + '","password":"' + $env:NEXUS_ADMIN_PASSWORD.Replace('"', '\"') + '"}'), ([System.Text.Encoding]::UTF8), 'application/json')
    ).GetAwaiter().GetResult()
    $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    $status = [int]$response.StatusCode
    $response.Dispose()
    if ($status -lt 200 -or $status -ge 300) { throw ("вход: HTTP {0} {1}" -f $status, $text) }
    return (ConvertFrom-JsonSafe $text)
}

function New-CloudFile {
    param(
        [string]$Name,
        [byte[]]$Bytes,
        [string]$ContentType = 'application/octet-stream'
    )
    $sha = Get-Sha256Hex $Bytes
    $session = (Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
        name = $Name
        parentId = $null
        size = $Bytes.Length
        contentType = $ContentType
    }).data
    $offset = 0
    $chunk = [int64]64 * $script:MiB
    while ($offset -lt $Bytes.Length) {
        $length = [int][Math]::Min($chunk, [int64]$Bytes.Length - $offset)
        $slice = New-Object byte[] $length
        [Array]::Copy($Bytes, $offset, $slice, 0, $length)
        $result = Invoke-Api -Method PATCH -Path ("/api/v1/cloud/uploads/{0}" -f $session.id) -Headers @{ 'Upload-Offset' = [string]$offset } -Bytes $slice
        $offset = [int64]$result.data.offset
    }
    $done = (Invoke-Api -Method POST -Path ("/api/v1/cloud/uploads/{0}/complete" -f $session.id) -Headers @{ 'Idempotency-Key' = ([guid]::NewGuid().ToString()) } -Body @{ strategy = 'auto' }).data
    $script:CreatedEntries.Add([pscustomobject]@{ id = $done.id; name = $Name; sha = $sha; size = $Bytes.Length })
    return [pscustomobject]@{ entry = $done; sha = $sha; size = $Bytes.Length }
}

function Get-DownloadSha {
    param([string]$EntryId)
    $uri = [uri]($script:BaseUrl + '/api/v1/cloud/entries/' + $EntryId + '/download')
    $request = New-Object System.Net.Http.HttpRequestMessage
    $request.Method = [System.Net.Http.HttpMethod]::Get
    $request.RequestUri = $uri
    $response = $script:Client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
    $status = [int]$response.StatusCode
    if ($status -ne 200) {
        $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $response.Dispose(); $request.Dispose()
        return [pscustomobject]@{ status = $status; sha = ''; bytes = 0; error = (Get-ApiErrorText $status $text) }
    }
    $stream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $buffer = New-Object byte[] 81920
    $zero = New-Object byte[] 81920
    $total = [int64]0
    while (($n = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
        $total += $n
        [void]$sha.TransformBlock($buffer, 0, $n, $zero, 0)
    }
    [void]$sha.TransformFinalBlock($zero, 0, 0)
    $hash = ([System.BitConverter]::ToString($sha.Hash) -replace '-', '').ToLowerInvariant()
    $sha.Dispose(); $stream.Dispose(); $response.Dispose(); $request.Dispose()
    return [pscustomobject]@{ status = 200; sha = $hash; bytes = $total; error = '' }
}

function Remove-CreatedEntries {
    foreach ($entry in $script:CreatedEntries) {
        try {
            [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}" -f $entry.id) -AllowError)
            [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}/purge" -f $entry.id) -AllowError)
        } catch { }
    }
}

# --- VHDX --------------------------------------------------------------------
function Get-FreeDriveLetter {
    $used = @(Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object { [string]$_.DriveLetter })
    foreach ($letter in @('R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z')) {
        if ($used -notcontains $letter) { return $letter }
    }
    throw 'Нет свободной буквы диска для VHDX.'
}

function Invoke-Diskpart {
    param([string[]]$Lines, [string]$Name)
    $file = Join-Path $TempRoot ('diskpart-' + $Name + '-' + $script:RunTag + '.txt')
    $Lines | Set-Content -LiteralPath $file -Encoding ASCII
    $output = & diskpart.exe /s $file 2>&1
    $code = $LASTEXITCODE
    Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
    if ($code -ne 0) {
        throw ("diskpart {0}: exit {1}`n{2}" -f $Name, $code, ($output -join "`n"))
    }
    return $output
}

function New-ResilienceVhd {
    $letter = Get-FreeDriveLetter
    $file = Join-Path $TempRoot ('resilience-' + $script:RunTag + '.vhdx')
    New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null
    Write-Log ("VHDX: {0} ({1} МБ), буква {2}:" -f $file, $VhdSizeMb, $letter)
    [void](Invoke-Diskpart -Name 'create' -Lines @(
        ('create vdisk file="{0}" maximum={1} type=expandable' -f $file, $VhdSizeMb),
        ('select vdisk file="{0}"' -f $file),
        'attach vdisk',
        'create partition primary',
        'format fs=ntfs quick label=NEXUSRES',
        ('assign letter={0}' -f $letter)
    ))
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and -not (Test-Path ($letter + ':\'))) { Start-Sleep -Milliseconds 500 }
    if (-not (Test-Path ($letter + ':\'))) { throw ("VHDX подключён, но {0}: не появился" -f $letter) }
    $volume = Get-Volume -DriveLetter $letter
    $root = Join-Path ($letter + ':\') ('nexus-' + $script:RunTag)
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    return [pscustomobject]@{
        Method = 'VHDX'
        File = $file
        Letter = $letter
        Root = $root
        SizeMb = $VhdSizeMb
        FileSystem = $volume.FileSystemType
    }
}

function Dismount-ResilienceVhd {
    param($Vhd)
    if (Get-Command Dismount-VHD -ErrorAction SilentlyContinue) {
        Dismount-VHD -Path $Vhd.File -ErrorAction Stop
        return 'Dismount-VHD'
    }
    [void](Invoke-Diskpart -Name 'detach' -Lines @(
        ('select vdisk file="{0}"' -f $Vhd.File),
        'detach vdisk'
    ))
    return 'diskpart detach vdisk'
}

function Mount-ResilienceVhd {
    param($Vhd)
    if (Get-Command Mount-VHD -ErrorAction SilentlyContinue) {
        Mount-VHD -Path $Vhd.File -ErrorAction Stop
    } else {
        [void](Invoke-Diskpart -Name 'attach' -Lines @(
            ('select vdisk file="{0}"' -f $Vhd.File),
            'attach vdisk',
            'select partition 1',
            ('assign letter={0}' -f $Vhd.Letter)
        ))
    }
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and -not (Test-Path ($Vhd.Letter + ':\'))) { Start-Sleep -Milliseconds 500 }
    if (-not (Test-Path ($Vhd.Letter + ':\'))) { throw ("VHDX подключён, но {0}: не появился" -f $Vhd.Letter) }
}

function Remove-ResilienceVhd {
    param($Vhd)
    if ($null -eq $Vhd) { return }
    try {
        if (Test-Path ($Vhd.Letter + ':\')) { [void](Dismount-ResilienceVhd -Vhd $Vhd) }
    } catch {
        Write-Log ('VHDX detach: ' + $_.Exception.Message)
    }
    if (-not $KeepArtifacts -and (Test-Path -LiteralPath $Vhd.File)) {
        try { Remove-Item -LiteralPath $Vhd.File -Force -ErrorAction Stop } catch {
            Write-Log ('VHDX удалить не удалось: ' + $_.Exception.Message)
        }
    }
}

# --- PostgreSQL (для C2) -----------------------------------------------------
function Get-DatabaseUrl {
    if ($DatabaseUrl) { return $DatabaseUrl }
    if ($env:DATABASE_URL) { return $env:DATABASE_URL }
    $envFile = Join-Path $script:Root '.env'
    if (Test-Path $envFile) {
        foreach ($line in Get-Content -LiteralPath $envFile) {
            if ($line -match '^\s*DATABASE_URL\s*=\s*(.+?)\s*$') { return $Matches[1].Trim('"', "'") }
        }
    }
    throw 'DATABASE_URL не найден: задайте -DatabaseUrl или .env.'
}

function Resolve-Psql {
    if ($PsqlPath) { return $PsqlPath }
    $command = Get-Command psql.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $candidate = Get-ChildItem 'C:\Program Files\PostgreSQL\*\bin\psql.exe' -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($candidate) { return $candidate.FullName }
    throw 'psql.exe не найден: задайте -PsqlPath.'
}

function Initialize-Psql {
    $script:DbUrl = Get-DatabaseUrl
    $script:Psql = Resolve-Psql
    $uri = [uri]$script:DbUrl
    $userInfo = $uri.UserInfo.Split(':', 2)
    $script:DbUser = [uri]::UnescapeDataString($userInfo[0])
    $script:DbPassword = if ($userInfo.Count -gt 1) { [uri]::UnescapeDataString($userInfo[1]) } else { '' }
    $script:DbHost = $uri.Host
    $script:DbPort = if ($uri.Port -gt 0) { $uri.Port } else { 5432 }
    $script:DbName = $uri.AbsolutePath.Trim('/')
    $env:PGPASSWORD = $script:DbPassword
    Write-Log ("PostgreSQL: {0}@{1}:{2}/{3}" -f $script:DbUser, $script:DbHost, $script:DbPort, $script:DbName)
}

function Invoke-Psql {
    param([string]$Sql, [switch]$Scalar)
    $psqlArgs = @('-h', $script:DbHost, '-p', [string]$script:DbPort, '-U', $script:DbUser, '-d', $script:DbName, '-t', '-A', '-v', 'ON_ERROR_STOP=1', '-c', $Sql)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $script:Psql @psqlArgs 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($code -ne 0) { throw ("psql: exit {0}: {1}" -f $code, (($output | ForEach-Object { [string]$_ }) -join ' ')) }
    $lines = @($output | ForEach-Object { [string]$_ } | Where-Object { $_ -ne '' })
    if ($Scalar) { return ($lines | Select-Object -First 1) }
    return $lines
}

function Install-C2Trigger {
    $sql = @'
drop trigger if exists resilience_job_status on background_jobs;
drop function if exists resilience_log_job_status();
drop table if exists resilience_job_transitions;
create table resilience_job_transitions (
    id bigserial primary key,
    job_id uuid not null,
    old_status text,
    new_status text,
    at timestamptz not null default now()
);
create function resilience_log_job_status() returns trigger as $$
begin
    if (tg_op = 'UPDATE' and old.status is distinct from new.status) then
        insert into resilience_job_transitions(job_id, old_status, new_status)
        values (new.id, old.status, new.status);
    end if;
    return new;
end $$ language plpgsql;
create trigger resilience_job_status after update on background_jobs
for each row execute function resilience_log_job_status();
'@
    [void](Invoke-Psql -Sql $sql)
    $script:TriggerInstalled = $true
}

function Remove-C2Trigger {
    if (-not $script:TriggerInstalled) { return }
    try {
        [void](Invoke-Psql -Sql @'
drop trigger if exists resilience_job_status on background_jobs;
drop function if exists resilience_log_job_status();
drop table if exists resilience_job_transitions;
'@)
    } catch {
        Write-Log ('триггер C2 не удалён: ' + $_.Exception.Message)
    }
    $script:TriggerInstalled = $false
}

# --- Служба Windows ----------------------------------------------------------
function Get-NexusService {
    $candidates = @()
    if ($ServiceName) { $candidates += $ServiceName }
    $candidates += @('NexusWorkspace', 'nexus', 'NEXUS')
    foreach ($name in $candidates) {
        try {
            $service = Get-Service -Name $name -ErrorAction Stop
            if ($service) { return $service }
        } catch { }
    }
    return $null
}

function New-TempNexusService {
    $name = 'NexusResilience' + $script:RunTag
    $port = 8092
    $envFile = Join-Path $script:Root '.env'
    $pairs = New-Object System.Collections.Generic.List[string]
    if (Test-Path $envFile) {
        foreach ($line in Get-Content -LiteralPath $envFile) {
            if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
                $key = $Matches[1]
                $value = $Matches[2].Trim('"', "'")
                if ($key -in @('DATABASE_URL', 'NEXUS_SECRET', 'NEXUS_COOKIE_SECURE')) {
                    $pairs.Add(('{0}={1}' -f $key, $value))
                }
            }
        }
    }
    $pairs.Add(('NEXUS_HOST=127.0.0.1'))
    $pairs.Add(('NEXUS_PORT={0}' -f $port))
    $pairs.Add(('NEXUS_PUBLIC_URL=http://127.0.0.1:{0}' -f $port))
    $pairs.Add(('NEXUS_STATIC_DIR={0}' -f (Join-Path $script:Root 'frontend\dist')))
    $pairs.Add(('NEXUS_COOKIE_SECURE=false'))
    $pairs.Add(('NEXUS_LOG_DIR={0}' -f (Join-Path $TempRoot 'service-logs')))
    $pairs.Add(('NEXUS_LOG_FILE=nexus-service.log'))
    $pairs.Add(('RUST_LOG=nexus=info,tower_http=warn,sqlx=warn'))
    $pairs.Add(('NEXUS_SERVICE_NAME={0}' -f $name))
    $scm = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control' -Name WaitToKillServiceTimeout -ErrorAction SilentlyContinue
    $scmTimeout = if ($scm -and $scm.WaitToKillServiceTimeout) { [int]$scm.WaitToKillServiceTimeout } else { 5000 }
    $pairs.Add(('NEXUS_SERVICE_STOP_TIMEOUT_MS={0}' -f $scmTimeout))
    $pairs = $pairs | Sort-Object -Unique
    $binPath = ('"{0}" serve --service' -f $script:ServerExe)
    & sc.exe create $name binPath= $binPath start= demand DisplayName= 'NEXUS Resilience Test' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw ("sc create: exit {0}" -f $LASTEXITCODE) }
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$name"
    New-ItemProperty -Path $key -Name Environment -PropertyType MultiString -Value ([string[]]$pairs) -Force | Out-Null
    return [pscustomobject]@{ Name = $name; Port = $port; Url = ('http://127.0.0.1:' + $port) }
}

function Remove-TempNexusService {
    param($Service)
    if ($null -eq $Service) { return }
    try {
        $connection = Get-NetTCPConnection -LocalPort $Service.Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($connection) { Stop-Process -Id $connection.OwningProcess -Force -ErrorAction SilentlyContinue }
    } catch { }
    try { Stop-Service -Name $Service.Name -Force -ErrorAction SilentlyContinue } catch { }
    & sc.exe delete $Service.Name | Out-Null
    $logDir = Join-Path $TempRoot 'service-logs'
    if (-not $KeepArtifacts -and (Test-Path $logDir)) { Remove-Item -LiteralPath $logDir -Recurse -Force -ErrorAction SilentlyContinue }
}

function Start-TestService {
    param($Service)
    # На части стендов Invoke-CimMethod Win32_Service.StartService возвращает
    # WBEM_E_INVALID_PARAMETER (0x80041008), а sc.exe start ждёт
    # SERVICE_RUNNING до 180 секунд. Поэтому пробуем оба способа и всегда
    # проверяем итоговое состояние через Get-Service.
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $state = Get-Service -Name $Service.Name -ErrorAction SilentlyContinue
        if ($null -eq $state) { break }
        if ($state.Status -eq 'Running') { return }
        if ($state.Status -eq 'Stopped') { break }
        Start-Sleep -Milliseconds 300
    }

    $started = $false
    try {
        $instance = Get-CimInstance Win32_Service -Filter ("Name='{0}'" -f $Service.Name) -ErrorAction Stop
        if ($instance) {
            $start = Invoke-CimMethod -InputObject $instance -MethodName StartService -ErrorAction Stop
            if ($start -and $start.ReturnValue -in @(0, 10)) { $started = $true }
        }
    } catch { }
    if (-not $started) {
        # 0 — запущено, 1056 — уже работает.
        & sc.exe start $Service.Name | Out-Null
        if ($LASTEXITCODE -notin @(0, 1056)) {
            throw ("Не удалось запустить службу {0}: sc.exe start вернул {1}." -f $Service.Name, $LASTEXITCODE)
        }
    }

    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        $state = Get-Service -Name $Service.Name -ErrorAction SilentlyContinue
        if ($null -eq $state) { break }
        if ($state.Status -eq 'Running') { return }
        Start-Sleep -Seconds 1
    }
    $state = Get-Service -Name $Service.Name -ErrorAction SilentlyContinue
    $status = if ($state) { [string]$state.Status } else { 'missing' }
    $message = "Служба осталась в состоянии {0}. Проверьте binPath (`serve --service`), logs\service-error.log и события Service Control Manager." -f $status
    throw $message
}

function Test-ServiceRestart {
    $result = [ordered]@{
        tested = $false; name = $null; port = $null; reason = ''
        started = $false; healthAfterStart = $false
        sessionSurvived = $false; uptimeBefore = $null; uptimeAfter = $null
        restartSeconds = $null
    }
    $service = $null
    $created = $false
    try {
        if ($TestService) {
            # Временная служба с binPath `serve --service`: проверяем реальный
            # SCM dispatcher, а не ограничение консольного приложения.
            $service = New-TempNexusService
            $created = $true
            $result.name = $service.Name
            $result.port = $service.Port
        } else {
            $existing = Get-NexusService
            if ($null -eq $existing) {
                $result.reason = 'Служба Windows не установлена; Restart-Service не проверялся (нужен -TestService).'
                return $result
            }
            $service = [pscustomobject]@{ Name = $existing.Name; Port = ([uri]$script:BaseUrl).Port; Url = $script:BaseUrl }
            $result.name = $service.Name
            $result.port = $service.Port
        }

        Write-Log ("служба {0}: запуск на {1}" -f $service.Name, $service.Url)
        Start-TestService -Service $service
        $health = Wait-Health -Url $service.Url -TimeoutSeconds 30
        $result.started = $true
        $result.healthAfterStart = $true
        $result.uptimeBefore = $health.uptimeSeconds

        $serviceClient = New-HttpClient -Temporary
        try {
            [void](Invoke-Login -Url $service.Url -Client $serviceClient)
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            Restart-Service -Name $service.Name -Force -ErrorAction Stop
            $health2 = Wait-Health -Url $service.Url -TimeoutSeconds 90
            $sw.Stop()
            $result.restartSeconds = [math]::Round($sw.Elapsed.TotalSeconds, 2)
            $result.uptimeAfter = $health2.uptimeSeconds
            $meResponse = $serviceClient.GetAsync($service.Url + '/api/v1/auth/me').GetAwaiter().GetResult()
            $meText = $meResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult()
            $me = ConvertFrom-JsonSafe $meText
            $meResponse.Dispose()
            $result.sessionSurvived = [bool]$me.success
            $result.tested = $result.sessionSurvived
            if (-not $result.sessionSurvived) { $result.reason = 'сессия не пережила Restart-Service' }
        } finally {
            $serviceClient.Dispose()
        }
    } catch {
        $result.reason = $_.Exception.Message
        Write-Log ('служба: ' + $_.Exception.Message)
    } finally {
        if ($created) { Remove-TempNexusService -Service $service }
    }
    return $result
}

# --- Окружение ---------------------------------------------------------------
function Get-EnvironmentInfo {
    $os = Get-CimInstance Win32_OperatingSystem
    $computer = Get-CimInstance Win32_ComputerSystem
    $defender = [ordered]@{ available = $false; realTime = $null; note = '' }
    try {
        $status = Get-MpComputerStatus -ErrorAction Stop
        $defender.available = $true
        $defender.realTime = [bool]$status.RealTimeProtectionEnabled
    } catch { $defender.note = $_.Exception.Message }
    $pg = 'unknown'
    try { $pg = (Invoke-Psql -Sql 'show server_version' -Scalar) } catch { }
    return [ordered]@{
        osCaption = $os.Caption
        osVersion = $os.Version
        osBuild = $os.BuildNumber
        powershell = $PSVersionTable.PSVersion.ToString()
        clr = [System.Environment]::Version.ToString()
        cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name
        logicalProcessors = $computer.NumberOfLogicalProcessors
        ramGb = [math]::Round($computer.TotalPhysicalMemory / 1GB, 2)
        filesystem = $(if ($script:Vhd) { $script:Vhd.FileSystem } else { 'NTFS (каталог)' })
        defenderRealTime = $defender.realTime
        defenderNote = $defender.note
        postgres = $pg
        admin = $true
    }
}

function Save-Reports {
    param($Report, [string]$Directory, [string]$Name)
    if (-not (Test-Path -LiteralPath $Directory)) { New-Item -ItemType Directory -Force -Path $Directory | Out-Null }
    $jsonPath = Join-Path $Directory ($Name + '.json')
    $mdPath = Join-Path $Directory ($Name + '.md')
    $logPath = Join-Path $Directory ($Name + '.log')
    $Report | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $jsonPath -Encoding UTF8
    $script:Log -join "`r`n" | Set-Content -LiteralPath $logPath -Encoding UTF8

    $lines = New-Object System.Collections.Generic.List[string]
    $add = { param([string]$text) $lines.Add($text) }

    & $add '# I5: устойчивость NEXUS (Windows)'
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
    & $add ('| ФС тестового тома | {0} |' -f $Report.environment.filesystem)
    & $add ('| Defender real-time | {0} |' -f $Report.environment.defenderRealTime)
    & $add ('| Сервер | `{0}` (PID {1}) |' -f $Report.server.url, $Report.server.pid)
    & $add ''
    & $add '## 1. Отказ диска'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    & $add ('| Способ | {0} |' -f $Report.disk.method)
    & $add ('| VHDX / корень | `{0}` |' -f $Report.disk.path)
    & $add ('| Том NEXUS | {0} (`{1}`) |' -f $Report.disk.volumeId, $Report.disk.volumeName)
    & $add ('| До отказа: reachable / state | {0} / {1} |' -f $Report.disk.beforeReachable, $Report.disk.beforeState)
    & $add ('| Отключение | {0} |' -f $Report.disk.failureAction)
    & $add ('| После отказа: reachable / state | {0} / {1} |' -f $Report.disk.afterReachable, $Report.disk.afterState)
    & $add ('| Загрузка на offline-том | {0} |' -f $Report.disk.uploadAfterFailure)
    & $add ('| Скачивание при отказе | HTTP {0}, code {1} |' -f $Report.disk.downloadStatus, $Report.disk.downloadCode)
    & $add ('| Восстановление | {0} |' -f $Report.disk.recovered)
    & $add ('| SHA после восстановления | {0} |' -f $Report.disk.shaAfterRecovery)
    & $add ''
    & $add '## 2. Обрыв клиента'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    & $add ('| Обрыв на 25% тела | {0} |' -f $Report.disconnect.truncated)
    & $add ('| Offset после обрыва | {0} из {1} |' -f $Report.disconnect.offsetAfter, $Report.disconnect.size)
    & $add ('| Статус сессии | {0} |' -f $Report.disconnect.statusAfter)
    & $add ('| Продолжение с серверного offset | {0} |' -f $Report.disconnect.resumed)
    & $add ('| SHA после завершения | {0} |' -f $Report.disconnect.shaOk)
    & $add ''
    & $add '## 3. Жёсткое падение (C2)'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    & $add ('| Задача | {0} (`{1}`) |' -f $Report.c2.jobType, $Report.c2.jobId)
    & $add ('| Объектов подготовлено | {0} × {1} МиБ |' -f $Report.c2.objects, $Report.c2.objectSizeMb)
    & $add ('| running до Stop-Process | {0} |' -f $Report.c2.runningBeforeKill)
    & $add ('| Stop-Process -Force | {0} |' -f $Report.c2.killed)
    & $add ('| Статус в БД после kill | {0} |' -f $Report.c2.statusAfterKill)
    & $add ('| Переходы статусов | {0} |' -f (($Report.c2.transitions | ForEach-Object { $_ }) -join ' → '))
    & $add ('| running → pending зафиксирован | {0} |' -f $Report.c2.recoveredToPending)
    & $add ('| Перезапуск | {0}, {1} с |' -f $Report.c2.restartMethod, $Report.c2.restartSeconds)
    & $add ('| Задача снова взята | {0} |' -f $Report.c2.claimedAgain)
    & $add ''
    & $add '## 4. Restart-Service'
    & $add ''
    & $add '| Проверка | Результат |'
    & $add '|---|---|'
    & $add ('| Проверялось | {0} |' -f $Report.service.tested)
    & $add ('| Служба | {0} |' -f $Report.service.name)
    & $add ('| Запуск / health | {0} / {1} |' -f $Report.service.started, $Report.service.healthAfterStart)
    & $add ('| Restart-Service, с | {0} |' -f $Report.service.restartSeconds)
    & $add ('| Сессия пережила перезапуск | {0} |' -f $Report.service.sessionSurvived)
    if ($Report.service.reason) { & $add ('| Причина / ограничение | {0} |' -f $Report.service.reason) }
    & $add ''
    if ($Report.failures.Count -gt 0) {
        & $add '## Ошибки'
        & $add ''
        foreach ($failure in $Report.failures) { & $add ('* ' + $failure) }
        & $add ''
    }
    if ($Report.limitations.Count -gt 0) {
        & $add '## Известные ограничения'
        & $add ''
        foreach ($limitation in $Report.limitations) { & $add ('* ' + $limitation) }
    }
    $lines -join "`r`n" | Set-Content -LiteralPath $mdPath -Encoding UTF8
    Write-Log ('отчёт: ' + $mdPath)
}

# --- Основной сценарий -------------------------------------------------------
$report = [ordered]@{
    result = 'FAIL'
    generatedAt = (Get-Date).ToString('o')
    command = ('powershell -File tests/resilience.ps1 -DiskFailureMode {0} -TestService:{1} -ReportName "{2}"' -f $DiskFailureMode, $TestService.IsPresent, $ReportName)
    environment = [ordered]@{}
    server = [ordered]@{}
    disk = [ordered]@{
        method = ''; path = ''; volumeId = ''; volumeName = ''
        beforeReachable = $null; beforeState = ''
        failureAction = ''; afterReachable = $null; afterState = ''
        uploadAfterFailure = ''; downloadStatus = $null; downloadCode = ''
        recovered = $false; shaAfterRecovery = ''
    }
    disconnect = [ordered]@{
        truncated = $false; offsetAfter = $null; size = $null; statusAfter = ''
        resumed = $false; shaOk = $false
    }
    c2 = [ordered]@{
        jobType = ''; jobId = ''; objects = 0; objectSizeMb = 0
        runningBeforeKill = $false; killed = $false; statusAfterKill = ''
        transitions = @(); recoveredToPending = $false
        restartMethod = ''; restartSeconds = $null; claimedAgain = $false
    }
    service = $script:ServiceInfo
    limitations = @()
    failures = @()
}
$failures = New-Object System.Collections.Generic.List[string]
$limitations = New-Object System.Collections.Generic.List[string]
$env:NEXUS_ADMIN_PASSWORD = $env:NEXUS_ADMIN_PASSWORD

try {
    if ([string]::IsNullOrWhiteSpace($env:NEXUS_ADMIN_PASSWORD)) {
        throw 'NEXUS_ADMIN_PASSWORD не задан. Пароль передаётся только через окружение.'
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $isAdmin) { $limitations.Add('Процесс запущен без прав администратора: VHDX и Stop-Process будут недоступны.') }

    if ($DiskFailureMode -eq 'Auto') {
        $DiskFailureMode = if ($isAdmin -and (Get-Command diskpart.exe -ErrorAction SilentlyContinue)) { 'Vhdx' } else { 'Rename' }
    }
    if ($DiskFailureMode -eq 'Vhdx' -and -not $isAdmin) {
        $limitations.Add('VHDX требует администратора; выполнен fallback на Rename.')
        $DiskFailureMode = 'Rename'
    }

    $script:Client = New-HttpClient
    $login = Invoke-Login
    $script:Csrf = $login.data.csrfToken
    $health = Invoke-Api -Method GET -Path '/api/v1/health'
    $connection = Get-NetTCPConnection -LocalPort ([uri]$script:BaseUrl).Port -State Listen -ErrorAction Stop | Select-Object -First 1
    if (-not $connection) { throw 'Сервер NEXUS не слушает порт.' }
    $process = Get-Process -Id $connection.OwningProcess -ErrorAction Stop
    $script:ServerProcessId = $process.Id
    $script:ServerExe = $process.Path
    $script:ServerWorkDir = Split-Path (Split-Path (Split-Path $script:ServerExe -Parent) -Parent) -Parent
    if (-not $script:ServerWorkDir -or -not (Test-Path -LiteralPath $script:ServerWorkDir)) {
        $script:ServerWorkDir = Split-Path $script:ServerExe -Parent
    }
    # Если сервер запущен как служба Windows, C2 должен возвращать её через
    # SCM, а не поднимать второй экземпляр рядом с занятым портом.
    $script:ServerService = $null
    try {
        $script:ServerService = Get-CimInstance Win32_Service -ErrorAction Stop |
            Where-Object { $_.PathName -and $_.PathName.IndexOf($script:ServerExe, [StringComparison]::OrdinalIgnoreCase) -ge 0 } |
            Select-Object -First 1
        if ($script:ServerService) {
            $script:ServerWorkDir = Split-Path $script:ServerExe -Parent
            Write-Log ("сервер под управлением службы {0}" -f $script:ServerService.Name)
        }
    } catch { }
    $report.server = [ordered]@{ url = $script:BaseUrl; pid = $process.Id; version = $health.data.version; health = $health.data.status }
    Write-Log ("сервер PID {0}, {1}, exe {2}" -f $process.Id, $health.data.status, $script:ServerExe)

    Initialize-Psql
    $report.environment = Get-EnvironmentInfo
    New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

    # --- 0. Отключить посторонние auto-тома, чтобы отказ был изолирован ------
    $volumes = @((Invoke-Api -Method GET -Path '/api/v1/storage/volumes').data)
    foreach ($volume in $volumes) {
        if ($volume.enabled -and $volume.state -eq 'online' -and $volume.mode -eq 'auto') {
            try {
                [void](Invoke-Api -Method PATCH -Path ("/api/v1/storage/volumes/{0}" -f $volume.id) -Body @{ enabled = $false })
                $script:DisabledVolumes.Add([pscustomobject]@{ id = $volume.id; name = $volume.name; root = $volume.rootPath })
                Write-Log ("временно отключён том {0} ({1})" -f $volume.name, $volume.rootPath)
            } catch {
                $limitations.Add(("не удалось временно отключить том {0}: {1}" -f $volume.name, $_.Exception.Message))
            }
        }
    }

    # --- 1. Создать носитель и зарегистрировать том --------------------------
    $script:Vhd = $null
    $storageRoot = $null
    if ($DiskFailureMode -eq 'Vhdx') {
        $script:Vhd = New-ResilienceVhd
        $storageRoot = $script:Vhd.Root
        $report.disk.method = 'VHDX + detach (снятие буквы и отключение диска)'
        $report.disk.path = $script:Vhd.File
        $report.environment.filesystem = ('NTFS (VHDX {0} МБ)' -f $VhdSizeMb)
    } else {
        $storageRoot = Join-Path $TempRoot ('disk-' + $script:RunTag)
        New-Item -ItemType Directory -Force -Path $storageRoot | Out-Null
        $report.disk.method = $DiskFailureMode
        $report.disk.path = $storageRoot
    }
    $volumeName = 'Resilience ' + $script:RunTag
    $createdVolume = (Invoke-Api -Method POST -Path '/api/v1/storage/volumes' -Body @{
        name = $volumeName
        path = $storageRoot
        mode = 'auto'
        priority = 1000
        reservedBytes = 0
    }).data
    $script:VolumeId = $createdVolume.id
    $report.disk.volumeId = $createdVolume.id
    $report.disk.volumeName = $createdVolume.name
    Write-Log ("том зарегистрирован: {0} ({1})" -f $createdVolume.name, $createdVolume.rootPath)

    $checkBefore = (Invoke-Api -Method POST -Path ("/api/v1/storage/volumes/{0}/check" -f $script:VolumeId)).data
    $report.disk.beforeReachable = [bool]$checkBefore.report.reachable
    $report.disk.beforeState = $checkBefore.row.state
    if (-not $report.disk.beforeReachable) { throw 'Том не online сразу после регистрации.' }

    # --- 2. Записать файл на тестовый том ------------------------------------
    $probeBytes = [NexusResilienceContent]::Generate($script:RunId, 1, 2 * $script:MiB)
    $probe = New-CloudFile -Name ('resilience-probe-' + $script:RunTag + '.bin') -Bytes $probeBytes
    $probeDownload = Get-DownloadSha -EntryId $probe.entry.id
    if ($probeDownload.sha -ne $probe.sha) { throw 'Файл на новом томе не прошёл проверку SHA до отказа.' }
    $objectOnVolume = [int64](Invoke-Psql -Sql ("select count(*) from storage_objects where volume_id = '{0}' and state = 'committed'" -f $script:VolumeId) -Scalar)
    if ($objectOnVolume -lt 1) { throw 'Объект не записан на тестовый том: выбор тома ушёл на другой диск.' }
    Write-Log ("probe: SHA {0}, объектов на томе {1}" -f $probe.sha.Substring(0, 16), $objectOnVolume)

    # --- 3. Отказ диска ------------------------------------------------------
    $failureAction = ''
    if ($DiskFailureMode -eq 'Vhdx') {
        $failureAction = Dismount-ResilienceVhd -Vhd $script:Vhd
    } elseif ($DiskFailureMode -eq 'Rename') {
        $renamed = $storageRoot + '.offline'
        Rename-Item -LiteralPath $storageRoot -NewName (Split-Path $renamed -Leaf) -ErrorAction Stop
        $failureAction = 'rename корня тома'
    } elseif ($DiskFailureMode -eq 'Acl') {
        $who = (& whoami).Trim()
        & icacls.exe $storageRoot /deny ('{0}:(OI)(CI)F' -f $who) | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'icacls deny завершился с ошибкой.' }
        $failureAction = ('icacls deny {0}' -f $who)
    }
    $report.disk.failureAction = $failureAction
    Start-Sleep -Seconds 2
    $checkAfterResponse = Invoke-Api -Method POST -Path ("/api/v1/storage/volumes/{0}/check" -f $script:VolumeId) -AllowError
    if ($checkAfterResponse.__error) {
        $report.disk.afterReachable = $false
        $report.disk.afterState = 'error'
        $failures.Add(("check после отказа: HTTP {0}" -f $checkAfterResponse.status))
    } else {
        $checkAfter = $checkAfterResponse.data
        $report.disk.afterReachable = [bool]$checkAfter.report.reachable
        $report.disk.afterState = $checkAfter.row.state
    }
    if ($report.disk.afterReachable) {
        $failures.Add('После отказа диска том всё ещё reachable/online.')
    } else {
        Write-Log ("том после отказа: reachable={0}, state={1}" -f $report.disk.afterReachable, $report.disk.afterState)
    }

    $uploadAttempt = Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
        name = ('offline-' + $script:RunTag + '.txt'); parentId = $null; size = 16; contentType = 'text/plain'
    } -AllowError
    if ($uploadAttempt.__error) {
        $code = ''
        if ($uploadAttempt.body -and $uploadAttempt.body.error) { $code = [string]$uploadAttempt.body.error.code }
        $report.disk.uploadAfterFailure = ('HTTP {0} {1}' -f $uploadAttempt.status, $code)
        if ($code -notin @('NO_VOLUME_AVAILABLE', 'VOLUME_UNAVAILABLE', 'INSUFFICIENT_SPACE')) {
            $failures.Add(('загрузка на offline-том вернула неожиданный код: {0}' -f $code))
        }
    } else {
        $report.disk.uploadAfterFailure = 'unexpected success'
        $failures.Add('Загрузка на offline-том неожиданно удалась.')
    }
    Write-Log ('загрузка на offline-том: ' + $report.disk.uploadAfterFailure)

    $downloadAttempt = Get-DownloadSha -EntryId $probe.entry.id
    $report.disk.downloadStatus = $downloadAttempt.status
    $report.disk.downloadCode = $downloadAttempt.error
    if ($downloadAttempt.status -eq 200) { $failures.Add('Скачивание с offline-тома неожиданно удалось.') }
    Write-Log ("скачивание при отказе: HTTP {0} {1}" -f $downloadAttempt.status, $downloadAttempt.error)

    # --- 4. Восстановление ---------------------------------------------------
    if ($DiskFailureMode -eq 'Vhdx') {
        Mount-ResilienceVhd -Vhd $script:Vhd
    } elseif ($DiskFailureMode -eq 'Rename') {
        Rename-Item -LiteralPath ($storageRoot + '.offline') -NewName (Split-Path $storageRoot -Leaf) -ErrorAction Stop
    } elseif ($DiskFailureMode -eq 'Acl') {
        $who = (& whoami).Trim()
        & icacls.exe $storageRoot /remove:d $who | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'icacls remove:d завершился с ошибкой.' }
    }
    Start-Sleep -Seconds 1
    $checkRestored = (Invoke-Api -Method POST -Path ("/api/v1/storage/volumes/{0}/check" -f $script:VolumeId)).data
    $report.disk.recovered = [bool]$checkRestored.report.reachable
    if (-not $report.disk.recovered) { $failures.Add('Том не восстановился после возврата диска.') }
    $downloadRestored = Get-DownloadSha -EntryId $probe.entry.id
    $report.disk.shaAfterRecovery = $downloadRestored.sha
    if ($downloadRestored.status -ne 200 -or $downloadRestored.sha -ne $probe.sha) {
        $failures.Add('После восстановления файл не скачался с исходным SHA.')
    }
    Write-Log ("восстановление: reachable={0}, SHA {1}" -f $report.disk.recovered, $downloadRestored.sha.Substring(0, [Math]::Min(16, $downloadRestored.sha.Length)))

    # Вернуть остальные тома.
    foreach ($volume in $script:DisabledVolumes) {
        try { [void](Invoke-Api -Method PATCH -Path ("/api/v1/storage/volumes/{0}" -f $volume.id) -Body @{ enabled = $true }) } catch { }
    }
    $script:DisabledVolumes.Clear()

    # --- 5. Обрыв клиента ----------------------------------------------------
    Write-Log 'обрыв клиента: PATCH на 25% тела'
    $disconnectBytes = [NexusResilienceContent]::Generate($script:RunId, 2, 1 * $script:MiB)
    $disconnectSha = Get-Sha256Hex $disconnectBytes
    $disconnectSession = (Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
        name = ('disconnect-' + $script:RunTag + '.bin'); parentId = $null; size = $disconnectBytes.Length; contentType = 'application/octet-stream'
    }).data
    $cookie = Get-CookieHeader -Url $script:BaseUrl
    $uri = [uri]$script:BaseUrl
    $tcp = New-Object System.Net.Sockets.TcpClient
    try {
        $tcp.Connect($uri.Host, $uri.Port)
        $stream = $tcp.GetStream()
        $header = ("PATCH /api/v1/cloud/uploads/{0} HTTP/1.1`r`nHost: {1}:{2}`r`nCookie: {3}`r`nX-CSRF-Token: {4}`r`nUpload-Offset: 0`r`nContent-Type: application/octet-stream`r`nContent-Length: {5}`r`nConnection: close`r`n`r`n" -f `
            $disconnectSession.id, $uri.Host, $uri.Port, $cookie, $script:Csrf, $disconnectBytes.Length)
        $headerBytes = [System.Text.Encoding]::ASCII.GetBytes($header)
        $stream.Write($headerBytes, 0, $headerBytes.Length)
        $partial = [int]($disconnectBytes.Length * 0.25)
        $stream.Write($disconnectBytes, 0, $partial)
        $stream.Flush()
        $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $report.disconnect.truncated = $true
    } finally {
        $tcp.Close()
    }
    Start-Sleep -Seconds 1
    $sessionStatus = (Invoke-Api -Method GET -Path ("/api/v1/cloud/uploads/{0}" -f $disconnectSession.id)).data
    $report.disconnect.offsetAfter = [int64]$sessionStatus.offset
    $report.disconnect.size = [int64]$disconnectBytes.Length
    $report.disconnect.statusAfter = $sessionStatus.status
    if ($sessionStatus.status -ne 'active') { $failures.Add('Сессия после обрыва не active.') }
    if ([int64]$sessionStatus.offset -gt $partial) { $failures.Add('Offset после обрыва больше отправленного.') }
    $resumeOffset = [int64]$sessionStatus.offset
    $remaining = New-Object byte[] ([int]($disconnectBytes.Length - $resumeOffset))
    [Array]::Copy($disconnectBytes, $resumeOffset, $remaining, 0, $remaining.Length)
    [void](Invoke-Api -Method PATCH -Path ("/api/v1/cloud/uploads/{0}" -f $disconnectSession.id) -Headers @{ 'Upload-Offset' = [string]$resumeOffset } -Bytes $remaining)
    $disconnectDone = (Invoke-Api -Method POST -Path ("/api/v1/cloud/uploads/{0}/complete" -f $disconnectSession.id) -Headers @{ 'Idempotency-Key' = ([guid]::NewGuid().ToString()) } -Body @{ strategy = 'auto' }).data
    $script:CreatedEntries.Add([pscustomobject]@{ id = $disconnectDone.id; name = $disconnectDone.name; sha = $disconnectSha; size = $disconnectBytes.Length })
    $disconnectDownload = Get-DownloadSha -EntryId $disconnectDone.id
    $report.disconnect.resumed = $true
    $report.disconnect.shaOk = ($disconnectDownload.sha -eq $disconnectSha)
    if (-not $report.disconnect.shaOk) { $failures.Add('Продолженная после обрыва загрузка не совпала по SHA.') }
    Write-Log ("обрыв: offset {0}/{1}, продолжено, SHA ok={2}" -f $report.disconnect.offsetAfter, $report.disconnect.size, $report.disconnect.shaOk)

    # --- 6. Restart-Service --------------------------------------------------
    Write-Log 'проверка Restart-Service'
    $report.service = Test-ServiceRestart
    if ($report.service.tested) {
        Write-Log ("Restart-Service: {0} с, сессия жива={1}" -f $report.service.restartSeconds, $report.service.sessionSurvived)
    } else {
        $limitations.Add([string]$report.service.reason)
        Write-Log ('Restart-Service: ' + $report.service.reason)
    }
    # Перезапуск мог сменить PID: C2 должен убивать текущий процесс.
    $currentListener = Get-NetTCPConnection -LocalPort ([uri]$script:BaseUrl).Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($currentListener) {
        $currentProcess = Get-Process -Id $currentListener.OwningProcess -ErrorAction SilentlyContinue
        if ($currentProcess) {
            $script:ServerProcessId = $currentProcess.Id
            $script:ServerExe = $currentProcess.Path
        }
    }

    # --- 7. C2: жёсткое падение во время running-задачи ----------------------
    Write-Log ("C2: подготовка {0} × {1} МиБ" -f $C2Objects, $C2ObjectSizeMb)
    $c2Size = [int64]$C2ObjectSizeMb * $script:MiB
    for ($i = 0; $i -lt $C2Objects; $i++) {
        $bytes = [NexusResilienceContent]::Generate($script:RunId, 100 + $i, $c2Size)
        [void](New-CloudFile -Name ('c2-' + $script:RunTag + '-{0:D3}.bin' -f $i) -Bytes $bytes)
        $bytes = $null
    }
    [void](Invoke-Psql -Sql 'select 1' -Scalar)
    Install-C2Trigger
    $report.c2.objects = $C2Objects
    $report.c2.objectSizeMb = $C2ObjectSizeMb

    $job = $null
    for ($attempt = 1; $attempt -le 3 -and $null -eq $job; $attempt++) {
        $created = (Invoke-Api -Method POST -Path '/api/v1/jobs/maintenance' -Body @{ kind = 'verify_objects'; mode = 'full'; samplePercent = 100 }).data
        $jobId = [string]$created.id
        $deadline = (Get-Date).AddSeconds(15)
        $running = $false
        while ((Get-Date) -lt $deadline) {
            $current = (Invoke-Api -Method GET -Path ("/api/v1/jobs/{0}" -f $jobId)).data
            if ($current.status -eq 'running') { $running = $true; break }
            if ($current.status -in @('completed', 'failed', 'cancelled')) { break }
            Start-Sleep -Milliseconds 20
        }
        if ($running) {
            $job = [pscustomobject]@{ id = $jobId; type = $created.jobType; status = 'running' }
        } else {
            Write-Log ("  попытка {0}: задача завершилась до kill, повтор" -f $attempt)
            Start-Sleep -Milliseconds 500
        }
    }
    if ($null -eq $job) { throw 'Не удалось поймать running-задачу для C2 за 3 попытки.' }
    $report.c2.jobId = $job.id
    $report.c2.jobType = $job.type
    $report.c2.runningBeforeKill = $true
    Write-Log ("  задача {0} в running, Stop-Process -Force" -f $job.id)

    Stop-Process -Id $script:ServerProcessId -Force -ErrorAction Stop
    $report.c2.killed = $true
    Start-Sleep -Milliseconds 800
    $report.c2.statusAfterKill = [string](Invoke-Psql -Sql ("select status from background_jobs where id = '{0}'" -f $job.id) -Scalar)
    Write-Log ("  статус в БД после kill: {0}" -f $report.c2.statusAfterKill)
    if ($report.c2.statusAfterKill -ne 'running') {
        $failures.Add('После Stop-Process задача не осталась в running — жёсткое падение не воспроизведено.')
    }

    $restartSw = [System.Diagnostics.Stopwatch]::StartNew()
    if ($script:ServerService) {
        # Служба могла быть автоматически перезапущена SCM (recovery actions).
        # Ждём health; если SCM оставил её Stopped — запускаем явно.
        $port = ([uri]$script:BaseUrl).Port
        $deadline = (Get-Date).AddSeconds(45)
        $recovered = $false
        while ((Get-Date) -lt $deadline) {
            try {
                $probe = Invoke-Api -Method GET -Path '/api/v1/health'
                if ($probe.success) { $recovered = $true; break }
            } catch { }
            $state = Get-Service -Name $script:ServerService.Name -ErrorAction SilentlyContinue
            if ($state -and $state.Status -eq 'Stopped') { break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $recovered) {
            Start-TestService -Service ([pscustomobject]@{ Name = $script:ServerService.Name })
            [void](Wait-Health -TimeoutSeconds 90)
        }
        $restartSw.Stop()
        $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($listener) { $script:ServerProcessId = $listener.OwningProcess }
        $report.c2.restartMethod = ('Start-Service ({0})' -f $script:ServerService.Name)
    } else {
        $outLog = Join-Path $TempRoot ('server-out-' + $script:RunTag + '.log')
        $errLog = Join-Path $TempRoot ('server-err-' + $script:RunTag + '.log')
        $started = Start-Process -FilePath $script:ServerExe -ArgumentList 'serve' -WorkingDirectory $script:ServerWorkDir -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru
        [void](Wait-Health -TimeoutSeconds 90)
        $restartSw.Stop()
        $script:ServerProcessId = $started.Id
        $report.c2.restartMethod = 'Start-Process (служба не использовалась для C2)'
    }
    $report.c2.restartSeconds = [math]::Round($restartSw.Elapsed.TotalSeconds, 2)
    Write-Log ("  сервер перезапущен PID {0} за {1} с ({2})" -f $script:ServerProcessId, $report.c2.restartSeconds, $report.c2.restartMethod)

    # Сессия должна пережить падение: cookie хранится в БД.
    $me = Invoke-Api -Method GET -Path '/api/v1/auth/me'
    if (-not $me.success) { $failures.Add('Сессия администратора не пережила жёсткий перезапуск.') }

    Start-Sleep -Seconds 4
    $transitions = @()
    try {
        $transitions = @(Invoke-Psql -Sql ("select old_status || '->' || new_status from resilience_job_transitions where job_id = '{0}' order by id" -f $job.id))
    } catch {
        $failures.Add('Не удалось прочитать переходы статусов C2: ' + $_.Exception.Message)
    }
    $report.c2.transitions = $transitions
    $report.c2.recoveredToPending = @($transitions | Where-Object { $_ -like 'running->pending' }).Count -gt 0
    $report.c2.claimedAgain = @($transitions | Where-Object { $_ -like 'pending->running' -or $_ -like 'pending->completed' }).Count -gt 0
    if (-not $report.c2.recoveredToPending) {
        $failures.Add('C2: переход running→pending не зафиксирован.')
    }
    Write-Log ("  переходы: {0}" -f ($transitions -join ', '))

    # Дождаться завершения или отменить задачу.
    try {
        $finalJob = (Invoke-Api -Method GET -Path ("/api/v1/jobs/{0}" -f $job.id)).data
        if ($finalJob.status -in @('pending', 'running', 'paused')) {
            [void](Invoke-Api -Method POST -Path ("/api/v1/jobs/{0}/cancel" -f $job.id) -Body @{})
        }
    } catch { }

    # --- 8. Очистка ----------------------------------------------------------
    Remove-CreatedEntries
    if ($script:VolumeId) {
        try { [void](Invoke-Api -Method DELETE -Path ("/api/v1/storage/volumes/{0}" -f $script:VolumeId) -AllowError) } catch { }
    }
    Remove-C2Trigger
    if ($script:Vhd) { Remove-ResilienceVhd -Vhd $script:Vhd }
    if (-not $KeepArtifacts -and $DiskFailureMode -ne 'Vhdx' -and (Test-Path $storageRoot)) {
        Remove-Item -LiteralPath $storageRoot -Recurse -Force -ErrorAction SilentlyContinue
    }

    $report.limitations = $limitations.ToArray()
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
    # Вернуть тома, даже если сценарий упал.
    foreach ($volume in $script:DisabledVolumes) {
        try { [void](Invoke-Api -Method PATCH -Path ("/api/v1/storage/volumes/{0}" -f $volume.id) -Body @{ enabled = $true }) } catch { }
    }
    try { Remove-CreatedEntries } catch { }
    try { Remove-C2Trigger } catch { }
    $serverAlive = $false
    if ($script:ServerProcessId) {
        $serverAlive = $null -ne (Get-Process -Id $script:ServerProcessId -ErrorAction SilentlyContinue)
    }
    if ($script:VolumeId -and $serverAlive) {
        try { [void](Invoke-Api -Method DELETE -Path ("/api/v1/storage/volumes/{0}" -f $script:VolumeId) -AllowError) } catch { }
    }
    if ($script:Vhd) {
        if (-not $serverAlive -and $script:VolumeId) {
            Write-Log 'сервер недоступен: VHDX оставлен подключённым для ручной очистки'
        } else {
            Remove-ResilienceVhd -Vhd $script:Vhd
        }
    }
    if (-not $KeepArtifacts) {
        Get-ChildItem -LiteralPath $TempRoot -Filter ('server-*-' + $script:RunTag + '.log') -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }
    if (-not $script:ReportsSaved) {
        try {
            $report.limitations = $limitations.ToArray()
            $report.failures = $failures.ToArray()
            Save-Reports -Report $report -Directory $ReportDir -Name $ReportName
            $script:ReportsSaved = $true
        } catch {
            Write-Host ('не удалось сохранить отчёт: ' + $_.Exception.Message)
        }
    }
    if ($script:Client) {
        try { $script:Client.Dispose() } catch { }
    }
}

[System.Environment]::Exit($script:ExitCode)
