# NEXUS WORKSPACE - проверка целостности файлов релиза по manifest.json.
# Запуск: powershell -ExecutionPolicy Bypass -File tools\Verify-Integrity.ps1
[CmdletBinding()]
param([string]$Root = '')

$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
$manifestPath = Join-Path $Root 'manifest.json'
if (-not (Test-Path -LiteralPath $manifestPath)) {
    Write-Host "manifest.json не найден: $manifestPath" -ForegroundColor Red
    exit 2
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$bad = 0
$checked = 0
foreach ($property in $manifest.files.PSObject.Properties) {
    $relative = $property.Name.Replace('/', '\')
    $path = Join-Path $Root $relative
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Host "ОТСУТСТВУЕТ: $relative" -ForegroundColor Red
        $bad++
        continue
    }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $property.Value) {
        Write-Host "ИЗМЕНЁН:    $relative" -ForegroundColor Yellow
        $bad++
    } else {
        $checked++
    }
}
Write-Host ""
Write-Host "Проверено файлов: $checked, проблем: $bad"
if ($bad -gt 0) { exit 1 }
exit 0
