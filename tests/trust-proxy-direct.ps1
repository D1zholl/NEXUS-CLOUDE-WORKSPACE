# NEXUS WORKSPACE - проверка J2: при прямом доступе к :8080 (без Caddy)
# NEXUS_TRUST_PROXY=false, поэтому X-Forwarded-For не учитывается и в аудите
# остаётся адрес TCP-соединения (127.0.0.1), а не подставленный клиентом.
#
# Запуск от администратора:
#   $env:NEXUS_TEST_PASSWORD = '<пароль>'
#   powershell -ExecutionPolicy Bypass -File tests\trust-proxy-direct.ps1

[CmdletBinding()]
param(
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$Username = 'j2admin',
    [string]$PasswordEnv = 'NEXUS_TEST_PASSWORD',
    [string]$SpoofedIp = '203.0.113.77',
    [string]$ResultPath = 'docs\reports\logs\j2-trust-proxy-direct.json'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http

function Write-Step($text) { Write-Host ""; Write-Host "==> $text" -ForegroundColor Cyan }

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Запустите скрипт от имени администратора."
}
if (-not (Test-Path ("Env:" + $PasswordEnv))) {
    throw "Переменная окружения $PasswordEnv не задана."
}
$password = (Get-Item ("Env:" + $PasswordEnv)).Value

$envPath = Join-Path $InstallDir '.env'
$original = [System.IO.File]::ReadAllText($envPath, [System.Text.Encoding]::UTF8)
$result = [ordered]@{
    startedAt = (Get-Date).ToString('o')
    installDir = $InstallDir
    service = $ServiceName
}

function Set-TrustProxy {
    param([string]$Value)
    $text = [System.IO.File]::ReadAllText($envPath, [System.Text.Encoding]::UTF8)
    if ($text -match '(?m)^NEXUS_TRUST_PROXY=') {
        $text = [regex]::Replace($text, '(?m)^NEXUS_TRUST_PROXY=.*$', "NEXUS_TRUST_PROXY=$Value")
    } else {
        $text = $text.TrimEnd() + [Environment]::NewLine + "NEXUS_TRUST_PROXY=$Value" + [Environment]::NewLine
    }
    [System.IO.File]::WriteAllText($envPath, $text, (New-Object System.Text.UTF8Encoding($false)))
}

function Wait-Health {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri 'http://127.0.0.1:8080/api/v1/health' -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -eq 200) { return $true }
        } catch { Start-Sleep -Milliseconds 500 }
    }
    return $false
}

function New-JsonContent($Body) {
    $json = $Body | ConvertTo-Json -Compress
    return New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
}

try {
    Write-Step "NEXUS_TRUST_PROXY=false и перезапуск службы"
    Set-TrustProxy -Value 'false'
    Restart-Service -Name $ServiceName -Force
    (Get-Service -Name $ServiceName).WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Running,
        [TimeSpan]::FromSeconds(30))
    if (-not (Wait-Health)) { throw "Сервер не отвечает на :8080 после перезапуска." }
    $result.trustProxy = 'false'
    $result.healthAfterRestart = 200

    Write-Step "Неудачный вход напрямую в :8080 с подставным X-Forwarded-For"
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $client = New-Object System.Net.Http.HttpClient($handler)
    $request = New-Object System.Net.Http.HttpRequestMessage
    $request.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'POST'
    $request.RequestUri = [uri]'http://127.0.0.1:8080/api/v1/auth/login'
    [void]$request.Headers.TryAddWithoutValidation('X-Forwarded-For', $SpoofedIp)
    [void]$request.Headers.TryAddWithoutValidation('X-Real-IP', $SpoofedIp)
    $request.Content = New-JsonContent @{ username = $Username; password = 'definitely-wrong-password' }
    $response = $client.SendAsync($request).GetAwaiter().GetResult()
    $result.directLoginStatus = [int]$response.StatusCode
    $response.Dispose(); $request.Dispose()

    Write-Step "Вход администратора по HTTPS и чтение аудита"
    $loginRequest = New-Object System.Net.Http.HttpRequestMessage
    $loginRequest.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'POST'
    $loginRequest.RequestUri = [uri]'https://localhost/api/v1/auth/login'
    $loginRequest.Content = New-JsonContent @{ username = $Username; password = $password }
    $loginResponse = $client.SendAsync($loginRequest).GetAwaiter().GetResult()
    $loginText = $loginResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    $result.httpsLoginStatus = [int]$loginResponse.StatusCode
    $loginResponse.Dispose(); $loginRequest.Dispose()
    $csrf = ($loginText | ConvertFrom-Json).data.csrfToken

    Start-Sleep -Seconds 1
    $auditRequest = New-Object System.Net.Http.HttpRequestMessage
    $auditRequest.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'GET'
    $auditRequest.RequestUri = [uri]'https://localhost/api/v1/system/audit?perPage=25&eventType=auth.login.failed'
    [void]$auditRequest.Headers.TryAddWithoutValidation('X-CSRF-Token', $csrf)
    $auditResponse = $client.SendAsync($auditRequest).GetAwaiter().GetResult()
    $auditText = $auditResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    $result.auditStatus = [int]$auditResponse.StatusCode
    $auditResponse.Dispose(); $auditRequest.Dispose(); $client.Dispose(); $handler.Dispose()

    $items = @(($auditText | ConvertFrom-Json).data.items)
    $latest = $items | Where-Object { $_.actorUsername -eq $Username } | Select-Object -First 1
    $result.auditEvent = $latest
    $result.spoofedIpSeen = [bool]($items | Where-Object { $_.ip -eq $SpoofedIp } | Select-Object -First 1)
    if (-not $latest) { throw "В аудите нет auth.login.failed для $Username." }
    if ($latest.ip -ne '127.0.0.1') {
        throw "Ожидался IP 127.0.0.1 при trust_proxy=false, получено $($latest.ip)."
    }
    if ($result.spoofedIpSeen) {
        throw "Подставной IP $SpoofedIp попал в аудит при trust_proxy=false."
    }
    Write-Host "  [PASS] аудит: ip=$($latest.ip), подставной $SpoofedIp отброшен" -ForegroundColor Green
    $result.pass = $true
} finally {
    Write-Step "Возврат NEXUS_TRUST_PROXY=true и запуск службы"
    [System.IO.File]::WriteAllText($envPath, $original, (New-Object System.Text.UTF8Encoding($false)))
    if ((Get-Service -Name $ServiceName).Status -ne 'Running') {
        Start-Service -Name $ServiceName
    } else {
        Restart-Service -Name $ServiceName -Force
    }
    (Get-Service -Name $ServiceName).WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Running,
        [TimeSpan]::FromSeconds(30))
    $result.restoredHealth = if (Wait-Health) { 200 } else { 0 }
}

$result.finishedAt = (Get-Date).ToString('o')
$dir = Split-Path -Parent $ResultPath
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
Write-Step "Готово"
Write-Host ("  JSON: {0}" -f $ResultPath)
if (-not $result.pass) { exit 1 } else { exit 0 }
