# NEXUS WORKSPACE - общие функции обслуживания (Windows).
#
# Подключается из backup.ps1, restore.ps1 и update-windows.ps1:
#   . (Join-Path $PSScriptRoot 'nexus-common.ps1')
#
# Пароль PostgreSQL никогда не передаётся аргументом командной строки:
# используется временный PGPASSFILE, который удаляется в finally.

function Write-NexusStep {
    param([string]$Text)
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}

# HttpClient живёт в System.Net.Http.dll; в Windows PowerShell 5.1 его нужно
# загрузить явно (в тестах это делается отдельно).
Add-Type -AssemblyName System.Net.Http

function Assert-NexusAdmin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Запустите скрипт от имени администратора."
    }
}

# Читает KEY=VALUE из .env. Кавычки и пробелы вокруг значения снимаются.
function Get-NexusEnvConfig {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Не найден $Path. Укажите правильный -InstallDir."
    }
    $config = @{}
    Get-Content -LiteralPath $Path | Where-Object { $_ -match '^\s*[A-Za-z_][A-Za-z0-9_]*=' } | ForEach-Object {
        $parts = $_ -split '=', 2
        $value = $parts[1].Trim()
        if ($value.Length -ge 2 -and (($value.StartsWith('"') -and $value.EndsWith('"')) -or
                                      ($value.StartsWith("'") -and $value.EndsWith("'")))) {
            $value = $value.Substring(1, $value.Length - 2)
        }
        $config[$parts[0].Trim()] = $value
    }
    return $config
}

# Разбирает postgres://user:pass@host:port/db?params без потери спецсимволов.
function ConvertFrom-NexusDatabaseUrl {
    param([Parameter(Mandatory = $true)][string]$DatabaseUrl)
    if ($DatabaseUrl -notmatch '^postgres(ql)?://') {
        throw "DATABASE_URL должен начинаться с postgres:// или postgresql://."
    }
    $rest = $DatabaseUrl.Substring($DatabaseUrl.IndexOf('://') + 3)
    $queryIndex = $rest.IndexOf('?')
    if ($queryIndex -ge 0) { $rest = $rest.Substring(0, $queryIndex) }
    $slash = $rest.LastIndexOf('/')
    if ($slash -lt 0) { throw "В DATABASE_URL не указано имя базы." }
    $authority = $rest.Substring(0, $slash)
    $database = [Uri]::UnescapeDataString($rest.Substring($slash + 1))
    if ([string]::IsNullOrWhiteSpace($database)) { throw "В DATABASE_URL не указано имя базы." }

    $user = ''
    $password = ''
    $at = $authority.LastIndexOf('@')
    if ($at -ge 0) {
        $userInfo = $authority.Substring(0, $at)
        $authority = $authority.Substring($at + 1)
        $colon = $userInfo.IndexOf(':')
        if ($colon -ge 0) {
            $user = [Uri]::UnescapeDataString($userInfo.Substring(0, $colon))
            $password = [Uri]::UnescapeDataString($userInfo.Substring($colon + 1))
        } else {
            $user = [Uri]::UnescapeDataString($userInfo)
        }
    }

    $hostName = $authority
    $port = '5432'
    if ($authority.StartsWith('[')) {
        $end = $authority.IndexOf(']')
        if ($end -lt 0) { throw "Некорректный IPv6-адрес в DATABASE_URL." }
        $hostName = $authority.Substring(1, $end - 1)
        $after = $authority.Substring($end + 1)
        if ($after.StartsWith(':')) { $port = $after.Substring(1) }
    } else {
        $lastColon = $authority.LastIndexOf(':')
        if ($lastColon -ge 0) {
            $hostName = $authority.Substring(0, $lastColon)
            $port = $authority.Substring($lastColon + 1)
        }
    }
    if ([string]::IsNullOrWhiteSpace($hostName)) { throw "В DATABASE_URL не указан хост." }
    return [pscustomobject]@{
        User     = $user
        Password = $password
        Host     = $hostName
        Port     = $port
        Database = $database
    }
}

# Возвращает каталог bin PostgreSQL: явный параметр, PATH, затем установки
# C:\Program Files\PostgreSQL\<версия>\bin (последняя версия первой).
function Resolve-NexusPgBin {
    param([string]$PgBin = "")
    if ($PgBin) {
        $candidate = Join-Path $PgBin 'pg_dump.exe'
        if (-not (Test-Path -LiteralPath $candidate)) {
            throw "В $PgBin нет pg_dump.exe."
        }
        return (Resolve-Path -LiteralPath $PgBin).Path
    }
    $onPath = Get-Command pg_dump.exe -ErrorAction SilentlyContinue
    if ($onPath) { return (Split-Path -Parent $onPath.Source) }
    $roots = @()
    if (${env:ProgramFiles}) { $roots += (Join-Path ${env:ProgramFiles} 'PostgreSQL') }
    if (${env:ProgramFiles(x86)}) { $roots += (Join-Path ${env:ProgramFiles(x86)} 'PostgreSQL') }
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $versions = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { Test-Path (Join-Path $_.FullName 'bin\pg_dump.exe') } |
            Sort-Object { [int]($_.Name -replace '\D', '') } -Descending
        if ($versions) { return (Join-Path $versions[0].FullName 'bin') }
    }
    throw "pg_dump.exe не найден. Укажите -PgBin 'C:\Program Files\PostgreSQL\17\bin'."
}

# Создаёт временный .pgpass (формат host:port:db:user:password с экранированием
# ':' и '\'), выставляет PGPASSFILE и убирает PGPASSWORD. Возвращает путь.
function New-NexusPgPassFile {
    param([Parameter(Mandatory = $true)]$Database)
    function Escape([string]$value) {
        return ($value -replace '\\', '\\') -replace ':', '\:'
    }
    # Поле базы — '*': один временный файл используется и для целевой БД, и для
    # служебного подключения к postgres (drop/create database).
    $line = '{0}:{1}:*:{2}:{3}' -f `
        (Escape $Database.Host), (Escape $Database.Port), `
        (Escape $Database.User), (Escape $Database.Password)
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ("nexus-pgpass-" + [guid]::NewGuid().ToString('N'))
    [System.IO.File]::WriteAllText($path, $line + [Environment]::NewLine, (New-Object System.Text.UTF8Encoding($false)))
    try {
        & icacls $path /inheritance:r /grant:r "$($env:USERNAME):(R)" /C /Q | Out-Null
    } catch {
        # Права уже ограничены профилем пользователя; не прерываем обслуживание.
    }
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
    $env:PGPASSFILE = $path
    if (-not $env:PGCONNECT_TIMEOUT) { $env:PGCONNECT_TIMEOUT = '10' }
    return $path
}

function Remove-NexusPgPassFile {
    param([string]$Path)
    if ($Path) { Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue }
    Remove-Item Env:PGPASSFILE -ErrorAction SilentlyContinue
}

function Get-NexusServiceObject {
    param([string]$Name)
    return (Get-Service -Name $Name -ErrorAction SilentlyContinue)
}

# Запускает nexus.exe с перенаправлением stdout/stderr в файлы. Это единственный
# надёжный способ получить код возврата в PowerShell 5.1: `& exe 2>&1` при
# ErrorActionPreference=Stop превращает stderr в NativeCommandError и обрывает
# обработку ошибок миграции до её разбора.
function Invoke-NexusProcess {
    param(
        [Parameter(Mandatory = $true)][string]$ExePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory = ''
    )
    $stdout = [System.IO.Path]::GetTempFileName()
    $stderr = [System.IO.Path]::GetTempFileName()
    try {
        $params = @{
            FilePath               = $ExePath
            ArgumentList           = $Arguments
            NoNewWindow            = $true
            Wait                   = $true
            PassThru               = $true
            RedirectStandardOutput = $stdout
            RedirectStandardError  = $stderr
        }
        if ($WorkingDirectory) { $params.WorkingDirectory = $WorkingDirectory }
        $process = Start-Process @params
        return [pscustomobject]@{
            Code   = $process.ExitCode
            StdOut = (Get-Content -LiteralPath $stdout -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
            StdErr = (Get-Content -LiteralPath $stderr -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
        }
    } finally {
        Remove-Item -LiteralPath $stdout, $stderr -Force -ErrorAction SilentlyContinue
    }
}

# Останавливает службу и ждёт Stopped. Возвращает $true, если служба была
# запущена и остановлена этим вызовом.
function Stop-NexusService {
    param([string]$Name, [int]$TimeoutSeconds = 40)
    $service = Get-NexusServiceObject -Name $Name
    if (-not $service -or $service.Status -eq 'Stopped') { return $false }
    Stop-Service -Name $Name -Force -ErrorAction Stop
    $service.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds($TimeoutSeconds))
    return $true
}

function Start-NexusService {
    param([string]$Name, [int]$TimeoutSeconds = 30)
    $service = Get-NexusServiceObject -Name $Name
    if (-not $service) { throw "Служба $Name не зарегистрирована." }
    if ($service.Status -ne 'Running') {
        Start-Service -Name $Name -ErrorAction Stop
        $service.WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Running,
            [TimeSpan]::FromSeconds($TimeoutSeconds))
    }
    return (Get-NexusServiceObject -Name $Name).Status
}

# Ждёт /api/v1/health на локальном порту. Возвращает HTTP-код или 0.
function Wait-NexusHttp {
    param([int]$Port = 8080, [int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri ("http://127.0.0.1:{0}/api/v1/health" -f $Port) `
                -UseBasicParsing -TimeoutSec 3 -ErrorAction Stop
            return [int]$response.StatusCode
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    return 0
}

# Проверяет вход через HTTP API и возвращает фактические коды login/me.
# Пароль передаётся параметром в памяти процесса, не через командную строку.
function Test-NexusLogin {
    param(
        [string]$BaseUrl = 'http://127.0.0.1:8080',
        [string]$Username,
        [string]$Password
    )
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    # Корневой сертификат Caddy установлен в LocalMachine\Root (certutil),
    # поэтому стандартная проверка TLS проходит без отключения валидации.
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(30)
    try {
        $json = @{ username = $Username; password = $Password } | ConvertTo-Json -Compress
        $content = New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
        $login = $client.PostAsync(($BaseUrl + '/api/v1/auth/login'), $content).GetAwaiter().GetResult()
        $loginBody = $login.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $loginCode = [int]$login.StatusCode
        $csrf = $null
        if ($loginCode -eq 200) {
            $parsed = $loginBody | ConvertFrom-Json
            if ($parsed.data -and $parsed.data.csrfToken) { $csrf = $parsed.data.csrfToken }
        }
        $request = New-Object System.Net.Http.HttpRequestMessage -ArgumentList ([System.Net.Http.HttpMethod]::Get), ($BaseUrl + '/api/v1/auth/me')
        if ($csrf) { [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $csrf) }
        $me = $client.SendAsync($request).GetAwaiter().GetResult()
        [void]$me.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        return [pscustomobject]@{
            LoginStatus = $loginCode
            MeStatus    = [int]$me.StatusCode
            CsrfToken   = [bool]$csrf
        }
    } finally {
        $client.Dispose()
        $handler.Dispose()
    }
}

# Полный текст ошибки API без вывода тела целиком.
function Get-NexusApiError {
    param([int]$Status, [string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return "HTTP $Status" }
    try {
        $parsed = $Text | ConvertFrom-Json
        if ($parsed.error) { return ("HTTP {0} {1}: {2}" -f $Status, $parsed.error.code, $parsed.error.message) }
    } catch { }
    if ($Text.Length -gt 300) { return ("HTTP {0}: {1}" -f $Status, $Text.Substring(0, 300)) }
    return ("HTTP {0}: {1}" -f $Status, $Text)
}
