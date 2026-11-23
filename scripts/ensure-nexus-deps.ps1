# NEXUS WORKSPACE - проверка и запуск зависимостей перед миграциями.
#
# Вызывается из start.bat и first-run.bat:
#   powershell -File scripts\ensure-nexus-deps.ps1 -EnvFile <path> -StartProxy
#
# Находит службу PostgreSQL по порту из DATABASE_URL и запускает её, если она
# остановлена. Caddy запускается как необязательный HTTPS-прокси: его ошибка
# не мешает запуску NEXUS.
#
# Коды возврата: 0 - PostgreSQL готов, 1 - PostgreSQL не удалось запустить,
# 2 - некорректный DATABASE_URL.

[CmdletBinding()]
param(
    [string]$EnvFile = '',
    [switch]$StartProxy,
    [switch]$CheckOnly,
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = 'Stop'

function Write-Line {
    param([string]$Text)
    Write-Output $Text
}

function Write-Ok {
    param([string]$Text)
    Write-Output ("[ok] " + $Text)
}

function Write-Warn {
    param([string]$Text)
    Write-Output ("[!!] " + $Text)
}

function Write-Fail {
    param([string]$Text)
    Write-Output ("[!!] " + $Text)
}

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Read-EnvFile {
    param([string]$Path)
    $config = @{}
    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
        return $config
    }
    Get-Content -LiteralPath $Path -Encoding UTF8 | Where-Object {
        $_ -match '^\s*[A-Za-z_][A-Za-z0-9_]*='
    } | ForEach-Object {
        $parts = $_ -split '=', 2
        $value = $parts[1].Trim()
        if ($value.Length -ge 2 -and
            (($value.StartsWith('"') -and $value.EndsWith('"')) -or
             ($value.StartsWith("'") -and $value.EndsWith("'")))) {
            $value = $value.Substring(1, $value.Length - 2)
        }
        $config[$parts[0].Trim()] = $value
    }
    return $config
}

function Get-DbTarget {
    param($Config)
    $url = $Config['DATABASE_URL']
    if ([string]::IsNullOrWhiteSpace($url)) {
        return $null
    }
    try {
        $uri = New-Object System.Uri($url)
    } catch {
        throw "DATABASE_URL не является корректным URL."
    }
    if (-not $uri.IsAbsoluteUri -or [string]::IsNullOrWhiteSpace($uri.Host)) {
        throw "В DATABASE_URL не указан хост PostgreSQL."
    }
    $port = 5432
    if ($uri.Port -gt 0) { $port = $uri.Port }
    $isLocal = @('127.0.0.1', 'localhost', '::1', '[::1]') -contains $uri.Host.ToLowerInvariant()
    if (-not $isLocal) {
        try {
            $addresses = @([System.Net.Dns]::GetHostAddresses($uri.Host))
            $isLocal = @($addresses | Where-Object { [System.Net.IPAddress]::IsLoopback($_) }).Count -gt 0
        } catch {
            $isLocal = $false
        }
    }
    return [pscustomobject]@{
        Host     = $uri.Host
        Port     = $port
        Database = $uri.AbsolutePath.Trim('/')
        IsLocal  = $isLocal
    }
}

function Get-ListeningProcessId {
    param([int]$Port)
    $connection = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($connection) { return [uint32]$connection.OwningProcess }
    return [uint32]0
}

function Get-ServiceOwnerName {
    param([uint32]$ProcessId)
    $services = Get-CimInstance Win32_Service -ErrorAction SilentlyContinue
    $current = $ProcessId
    for ($i = 0; $i -lt 8 -and $current -gt 0; $i++) {
        $service = $services | Where-Object { $_.ProcessId -eq $current } | Select-Object -First 1
        if ($service) { return $service.Name }
        $process = Get-CimInstance Win32_Process -Filter ('ProcessId=' + $current) -ErrorAction SilentlyContinue
        if (-not $process) { break }
        $current = [uint32]$process.ParentProcessId
    }
    return $null
}

function Get-PgDataDirectory {
    param([string]$PathName)
    if ([string]::IsNullOrWhiteSpace($PathName)) { return $null }
    if ($PathName -match '-D\s+"([^"]+)"') { return $Matches[1] }
    if ($PathName -match '-D\s+([^\s"]+)') { return $Matches[1] }
    return $null
}

function Get-PgServicePort {
    param($Service)
    $dataDir = Get-PgDataDirectory $Service.PathName
    if (-not $dataDir) { return $null }
    $port = 5432
    $readAny = $false
    foreach ($name in @('postgresql.conf', 'postgresql.auto.conf')) {
        $file = Join-Path $dataDir $name
        if (-not (Test-Path -LiteralPath $file)) { continue }
        try {
            $lines = @(Get-Content -LiteralPath $file -ErrorAction Stop)
        } catch {
            continue
        }
        $readAny = $true
        foreach ($line in $lines) {
            if ($line -match "^\s*port\s*=\s*'?(\d+)'?\s*(?:#.*)?$") {
                $port = [int]$Matches[1]
            }
        }
    }
    if (-not $readAny) { return $null }
    return $port
}

function Stop-WithError {
    param([string]$Text, [int]$Code = 1)
    Write-Fail $Text
    exit $Code
}

$envPath = $EnvFile
if ([string]::IsNullOrWhiteSpace($envPath)) {
    $envPath = Join-Path (Get-Location).Path '.env'
}

Write-Line "==> Проверяю зависимости NEXUS"
if (-not (Test-Path -LiteralPath $envPath)) {
    Write-Warn "Файл .env не найден: $envPath"
    Write-Line "PostgreSQL не проверялся."
    exit 0
}
Write-Line "Файл .env: $envPath"

$config = Read-EnvFile $envPath
try {
    $database = Get-DbTarget $config
} catch {
    Stop-WithError $_.Exception.Message 2
}

if (-not $database) {
    Write-Warn "DATABASE_URL не задан - PostgreSQL не проверялся."
    exit 0
}

Write-Line ("PostgreSQL: {0}:{1}/{2}" -f $database.Host, $database.Port, $database.Database)
if (-not $database.IsLocal) {
    Write-Ok "База данных внешняя, локальные службы не нужны."
    exit 0
}

$listener = Get-ListeningProcessId $database.Port
if ($listener -gt 0) {
    $owner = Get-ServiceOwnerName $listener
    if ($owner) {
        Write-Ok ("Порт {0} слушается, служба {1}." -f $database.Port, $owner)
    } else {
        Write-Ok ("Порт {0} слушается." -f $database.Port)
    }
} else {
    $pgServices = @(Get-CimInstance Win32_Service -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like 'postgresql*' })
    $candidates = @()
    foreach ($service in $pgServices) {
        $candidates += [pscustomobject]@{
            Name      = $service.Name
            Port      = Get-PgServicePort $service
            State     = $service.State
            StartMode = $service.StartMode
        }
    }

    $match = $candidates | Where-Object { $_.Port -eq $database.Port } | Select-Object -First 1
    if (-not $match -and $candidates.Count -eq 1) {
        $match = $candidates[0]
    }
    if (-not $match) {
        Write-Fail ("Не удалось определить службу PostgreSQL для порта {0}." -f $database.Port)
        foreach ($candidate in $candidates) {
            $candidatePort = '?'
            if ($candidate.Port) { $candidatePort = $candidate.Port }
            Write-Line ("  {0}: порт {1}, состояние {2}" -f $candidate.Name, $candidatePort, $candidate.State)
        }
        Write-Line "Запустите PostgreSQL вручную или укажите службу: Start-Service <имя>."
        exit 1
    }

    if ($CheckOnly) {
        if ($match.State -eq 'Running') {
            Write-Warn ("Служба {0} запущена, но порт {1} ещё не слушается." -f $match.Name, $database.Port)
        } else {
            Write-Warn ("Служба {0} не запущена, состояние {1}." -f $match.Name, $match.State)
        }
        Write-Line ("Запустите: Start-Service {0}" -f $match.Name)
        exit 1
    }

    if (-not (Test-Admin)) {
        Write-Warn "Нет прав администратора: запуск службы PostgreSQL может быть запрещён."
    }
    Write-Line ("==> Запускаю службу {0}..." -f $match.Name)
    try {
        $service = Get-Service -Name $match.Name -ErrorAction Stop
        if ($service.Status -ne 'Running') {
            Start-Service -Name $match.Name -ErrorAction Stop
            $service.WaitForStatus(
                [System.ServiceProcess.ServiceControllerStatus]::Running,
                [TimeSpan]::FromSeconds($TimeoutSeconds))
        }
    } catch {
        Write-Fail ("Не удалось запустить {0}: {1}" -f $match.Name, $_.Exception.Message)
        if (-not (Test-Admin)) {
            Write-Line "Запустите start.bat от имени администратора."
        }
        exit 1
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not (Get-ListeningProcessId $database.Port) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
    }
    if (-not (Get-ListeningProcessId $database.Port)) {
        Write-Fail ("Служба {0} запущена, но порт {1} не слушается." -f $match.Name, $database.Port)
        Write-Line "Проверьте port в postgresql.conf и файрвол."
        exit 1
    }
    Write-Ok ("PostgreSQL запущен, порт {0}." -f $database.Port)
}

if ($StartProxy) {
    $caddy = Get-Service -Name 'Caddy' -ErrorAction SilentlyContinue
    if (-not $caddy) {
        Write-Warn "Служба Caddy не зарегистрирована - HTTPS-прокси не запущен."
    } elseif ($caddy.Status -eq 'Running') {
        Write-Ok "Caddy уже работает."
    } elseif ($CheckOnly) {
        Write-Warn ("Caddy не запущен, состояние {0}." -f $caddy.Status)
    } else {
        try {
            Start-Service -Name 'Caddy' -ErrorAction Stop
            $caddy.WaitForStatus(
                [System.ServiceProcess.ServiceControllerStatus]::Running,
                [TimeSpan]::FromSeconds(15))
            Write-Ok "Caddy запущен."
        } catch {
            Write-Warn ("Caddy не запущен: {0}" -f $_.Exception.Message)
            Write-Line "NEXUS продолжит работу без HTTPS-прокси."
        }
    }
}

exit 0
