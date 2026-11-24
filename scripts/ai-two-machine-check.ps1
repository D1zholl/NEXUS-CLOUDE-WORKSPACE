# Проверка реального сценария на двух компьютерах.
#
# Запускать НА КОМПЬЮТЕРЕ RUNTIME (PC-2), где установлена PolyStrata.
# Скрипт не может подтвердить online-статус в NEXUS без административной
# сессии; он проверяет локальную конфигурацию, NEXUS health, PolyStrata,
# enrollment и запуск Runtime. Финальную проверку UI/задач выполняет оператор.
#
# Пример:
#   .\scripts\ai-two-machine-check.ps1 `
#     -BackendUrl https://nexus.example `
#     -EnrollmentCode nxe_xxx `
#     -PolyStrataUrl http://127.0.0.1:8080/v1 `
#     -ConfigPath C:\NexusAI\runtime.toml

param(
    [Parameter(Mandatory = $true)] [string] $BackendUrl,
    [string] $EnrollmentCode = '',
    [string] $PolyStrataUrl = 'http://127.0.0.1:8080/v1',
    [string] $ConfigPath = 'C:\NexusAI\runtime.toml',
    [string] $RuntimeExe = '',
    [int] $RunSeconds = 0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $RuntimeExe) {
    $RuntimeExe = Join-Path $root 'runtime\target\release\nexus-ai-runtime.exe'
}
$BackendUrl = $BackendUrl.TrimEnd('/')

function Step($name) { Write-Host "==> $name" }
function Fail($message) { Write-Host "FAIL: $message" -ForegroundColor Red; exit 1 }
function Ok($message) { Write-Host "OK: $message" -ForegroundColor Green }

Step 'NEXUS health'
try {
    $health = Invoke-RestMethod -Uri "$BackendUrl/api/v1/health" -TimeoutSec 15
    Ok "NEXUS доступен: $($health | ConvertTo-Json -Compress)"
} catch {
    Fail "NEXUS health недоступен: $($_.Exception.Message)"
}

Step 'PolyStrata models/health/status'
foreach ($path in @("$PolyStrataUrl/models", ($PolyStrataUrl -replace '/v1$', '') + '/health', ($PolyStrataUrl -replace '/v1$', '') + '/status')) {
    try {
        $response = Invoke-WebRequest -Uri $path -TimeoutSec 15 -UseBasicParsing
        Ok "$path -> HTTP $($response.StatusCode)"
    } catch {
        Write-Host "WARN: $path -> $($_.Exception.Message)" -ForegroundColor Yellow
    }
}

if (-not (Test-Path $RuntimeExe)) {
    Fail "Runtime executable не найден: $RuntimeExe (соберите cargo build --release в runtime)"
}
if (-not (Test-Path $ConfigPath)) {
    Fail "Конфигурация Runtime не найдена: $ConfigPath"
}

Step 'runtime check'
& $RuntimeExe --config $ConfigPath check --json
if ($LASTEXITCODE -ne 0) {
    Fail "runtime check вернул код $LASTEXITCODE (см. problems выше)"
}

if ($EnrollmentCode) {
    Step 'enrollment'
    $tokenPath = Join-Path (Split-Path -Parent $ConfigPath) 'state\device.json'
    if (Test-Path $tokenPath) {
        $backup = "$tokenPath.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
        Copy-Item $tokenPath $backup
        Write-Host "Существующий device token сохранён: $backup"
    }
    & $RuntimeExe --config $ConfigPath enroll --code $EnrollmentCode --name $env:COMPUTERNAME
    if ($LASTEXITCODE -ne 0) { Fail "enrollment завершился с кодом $LASTEXITCODE" }
    Ok 'enrollment выполнен; код использован и больше недействителен'
}

if ($RunSeconds -gt 0) {
    Step "Запуск Runtime на $RunSeconds секунд (проверка WSS/heartbeat)"
    $process = Start-Process -FilePath $RuntimeExe `
        -ArgumentList @('--config', $ConfigPath, 'run') `
        -PassThru -NoNewWindow
    Start-Sleep -Seconds $RunSeconds
    if ($process.HasExited) {
        Fail "Runtime завершился с кодом $($process.ExitCode). Проверьте logs и runtime check."
    }
    Stop-Process -Id $process.Id -Force
    Ok 'Runtime работал без немедленного падения; online-статус проверьте в AI Management'
}

Write-Host ''
Write-Host 'Ручные шаги приёмки (обязательны):'
Write-Host '1. System Management -> AI Management: Runtime online, heartbeat свежий.'
Write-Host '2. Chat: потоковый ответ реальной PolyStrata.'
Write-Host '3. Agent: fs.read/fs.write, затем fs.delete с approval.'
Write-Host '4. Cancel: статус cancelled + cancelStatus=confirmed.'
Write-Host '5. Restart-Service NexusAIRuntime -> reconnect без потери задач.'
Write-Host '6. Revoke token -> новое подключение отклонено.'
Write-Host '7. Usage/MCP: refresh usage и discovery доверенного MCP.'
Write-Host ''
Write-Host 'Результат приложите к docs/AI_PRODUCTION_AUDIT.md с датой и версией PolyStrata.'
