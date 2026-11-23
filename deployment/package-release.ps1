# NEXUS WORKSPACE - сборка release-архива для Windows.
#
# Создаёт dist\nexus-<version>-windows-x64.zip:
#   nexus.exe, frontend\, deployment\ (установка/backup/restore/update/Caddy для
#   Windows, шаблон nexus.production.toml), docs\INSTALL-WINDOWS.md, README-RELEASE.txt,
#   VERSION, SHA256SUMS (Get-FileHash) и внешний <zip>.sha256.
# nexus.toml создаётся установщиком из deployment\nexus.production.toml.
#
# Запуск:
#   powershell -ExecutionPolicy Bypass -File deployment\package-release.ps1
#   powershell -ExecutionPolicy Bypass -File deployment\package-release.ps1 -SkipBuild -Version 1.0.0

[CmdletBinding()]
param(
    [string]$Version = '1.0.0',
    [string]$OutputDir = 'dist',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Write-Step($text) {
    Write-Host ""
    Write-Host "==> $text" -ForegroundColor Cyan
}

$repo = Split-Path -Parent $PSScriptRoot
$name = "nexus-$Version-windows-x64"
$stage = Join-Path $repo (Join-Path $OutputDir $name)
$zipPath = Join-Path $repo (Join-Path $OutputDir ($name + '.zip'))

if (-not $SkipBuild) {
    Write-Step "Сборка backend (release)"
    $env:PATH = "C:\Program Files\nodejs;" + $env:PATH
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (-not ($env:PATH -like "*$cargoBin*")) { $env:PATH = "$cargoBin;" + $env:PATH }
    Push-Location (Join-Path $repo 'backend')
    try { cargo build --release } finally { Pop-Location }

    Write-Step "Сборка frontend"
    Push-Location (Join-Path $repo 'frontend')
    try {
        if (-not (Test-Path 'node_modules')) { npm install }
        npm run build
    } finally { Pop-Location }
}

$exe = Join-Path $repo 'backend\target\release\nexus.exe'
$frontend = Join-Path $repo 'frontend\dist'
foreach ($required in @($exe, $frontend)) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Нет обязательного артефакта: $required" }
}

Write-Step "Подготовка $stage"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'nexus.exe') -Force
Copy-Item -LiteralPath $frontend -Destination (Join-Path $stage 'frontend') -Recurse -Force

$deployment = Join-Path $stage 'deployment'
New-Item -ItemType Directory -Force -Path $deployment | Out-Null
$deploymentFiles = @(
    'install-windows.ps1',
    'backup.ps1',
    'restore.ps1',
    'backup-objects.ps1',
    'restore-objects.ps1',
    'objects-common.ps1',
    'update-windows.ps1',
    'nexus-common.ps1',
    'check-ports.ps1',
    'firewall-windows.ps1',
    'Caddyfile.windows',
    'env.production.example',
    'nexus.production.toml'
)
foreach ($file in $deploymentFiles) {
    $source = Join-Path $PSScriptRoot $file
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $deployment -Force }
}

New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null
foreach ($doc in @('INSTALL-WINDOWS.md')) {
    $source = Join-Path $repo ('docs\' + $doc)
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $stage 'docs') -Force }
}

$versionText = (& (Join-Path $stage 'nexus.exe') --version 2>&1) -join ' '
$releaseNotes = @"
NEXUS WORKSPACE - release $Version (windows-x64)

Артефакт:      $name.zip
Сборка:        $versionText
Собран:        $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss zzz'))
Платформа:     Windows 11 x64 (Windows Server 2022+)

Быстрая установка (PowerShell от администратора):
  Expand-Archive .\$name.zip -DestinationPath C:\
  cd C:\$name
  powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1 -SkipBuild `
      -DatabaseUrl "postgres://nexus:ПАРОЛЬ@127.0.0.1:5432/nexus" `
      -AdminPassword "ПарольАдминистратора" -TestServiceControl

Подробно: docs\INSTALL-WINDOWS.md
Проверка целостности: Get-FileHash .\$name.zip -Algorithm SHA256
"@
[System.IO.File]::WriteAllText((Join-Path $stage 'README-RELEASE.txt'), $releaseNotes, (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText((Join-Path $stage 'VERSION'), "$Version`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Step "SHA256SUMS (Get-FileHash)"
$lines = Get-ChildItem -LiteralPath $stage -Recurse -File |
    Where-Object { $_.Name -ne 'SHA256SUMS' } |
    Sort-Object FullName |
    ForEach-Object {
        $relative = $_.FullName.Substring($stage.Length).TrimStart('\').Replace('\', '/')
        '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $relative
    }
$lines | Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS') -Encoding ASCII
Write-Host ("    файлов: {0}" -f $lines.Count)

Write-Step "Упаковка $zipPath"
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $zipPath) | Out-Null
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zipPath -CompressionLevel Optimal

$zipHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$zipSize = (Get-Item -LiteralPath $zipPath).Length
"$zipHash  $name.zip" | Set-Content -LiteralPath ($zipPath + '.sha256') -Encoding ASCII

Write-Step "Готово"
Write-Host "  Архив:  $zipPath"
Write-Host ("  Размер: {0:N0} байт" -f $zipSize)
Write-Host "  SHA-256: $zipHash"
Write-Host "  Файл:   $zipPath.sha256"
