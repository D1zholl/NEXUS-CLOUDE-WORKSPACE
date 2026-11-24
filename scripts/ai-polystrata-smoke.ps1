# Реальная проверка PolyStrata (не stub).
#
# Требуется запущенная PolyStrata и выбранная модель, поддерживающая chat.
# Запускать на компьютере Runtime.
#
# Пример:
#   $env:NEXUS_POLYSTRATA_URL='http://127.0.0.1:8080/v1'
#   $env:NEXUS_POLYSTRATA_MODEL='your-model'
#   .\scripts\ai-polystrata-smoke.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $env:NEXUS_POLYSTRATA_URL) {
    $env:NEXUS_POLYSTRATA_URL = 'http://127.0.0.1:8080/v1'
}

Write-Host "==> PolyStrata: $env:NEXUS_POLYSTRATA_URL"
try {
    $models = Invoke-RestMethod -Uri "$($env:NEXUS_POLYSTRATA_URL.TrimEnd('/'))/models" -TimeoutSec 15
    Write-Host ($models | ConvertTo-Json -Depth 4 -Compress)
} catch {
    throw "PolyStrata /models недоступен: $($_.Exception.Message)"
}

if (-not $env:NEXUS_POLYSTRATA_MODEL) {
    Write-Host 'WARN: NEXUS_POLYSTRATA_MODEL не задан; тест выберет первую модель из /models.' -ForegroundColor Yellow
}

cargo test --manifest-path (Join-Path $root 'runtime\Cargo.toml') `
    real_polystrata_smoke -- --ignored --nocapture
if ($LASTEXITCODE -ne 0) {
    throw "real_polystrata_smoke завершился с кодом $LASTEXITCODE"
}
Write-Host 'real_polystrata_smoke: OK' -ForegroundColor Green
