#requires -Version 5.1
<#
.SYNOPSIS
  Собирает переносимый NEXUS WORKSPACE Standalone для Windows 11 x64.

.DESCRIPTION
  Production-сборка frontend/backend/runtime/launcher, portable PostgreSQL 17,
  runtime DLL, документация, лицензии, manifest.json и SHA256SUMS.
  Промежуточная сборка выполняется в build\standalone, затем переносится в
  NEXUS-Standalone-release только после проверок.

.PARAMETER Clean
  Удалить предыдущие build-артефакты (staging и целевую папку, если в ней нет
  пользовательских данных).

.PARAMETER SkipTests
  Не запускать frontend/backend/runtime тесты. Только если они уже выполнены.

.PARAMETER Force
  Разрешить замену целевой папки, даже если в ней найдены данные. Используйте
  осознанно: данные не копируются, папка будет заменена.

.PARAMETER PgZip
  Путь к официальному zip PostgreSQL 17 Windows x64 binaries.
#>
[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$SkipTests,
    [switch]$Force,
    [string]$PgZip = '',
    [string]$PgVersion = '17.6'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repo = $PSScriptRoot
$expectedTarget = [System.IO.Path]::GetFullPath((Join-Path $repo 'NEXUS-Standalone-release')).TrimEnd('\')
$buildRoot = Join-Path $repo 'build\standalone'
$stage = Join-Path $buildRoot 'NEXUS-Standalone-release'
$started = Get-Date
$testSummary = @()
$warnings = @()

function Step([string]$text) {
    Write-Host ''
    Write-Host "==> $text" -ForegroundColor Cyan
}
function Note([string]$text) { Write-Host "    $text" -ForegroundColor DarkGray }
function Warn([string]$text) {
    $script:warnings += $text
    Write-Host "    ВНИМАНИЕ: $text" -ForegroundColor Yellow
}
function Fail([string]$text) { throw $text }

function Invoke-Native([string]$file, [string[]]$arguments, [string]$workdir, [string]$label) {
    Push-Location $workdir
    try {
        & $file @arguments
        if ($LASTEXITCODE -ne 0) { Fail "$label завершился с кодом $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
}

function Assert-SafePath([string]$path, [string]$expected) {
    $full = [System.IO.Path]::GetFullPath($path).TrimEnd('\')
    if ($full -ne $expected) { Fail "Небезопасный путь: $full (ожидался $expected)" }
    if ($full -eq [System.IO.Path]::GetFullPath($repo).TrimEnd('\')) {
        Fail 'Отказ: попытка удалить корень проекта'
    }
    if (-not $full.StartsWith([System.IO.Path]::GetFullPath($repo), [StringComparison]::OrdinalIgnoreCase)) {
        Fail "Отказ: путь вне проекта: $full"
    }
}

function Test-UserData([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    foreach ($marker in @('config\secrets.json', 'data\postgres\PG_VERSION')) {
        if (Test-Path -LiteralPath (Join-Path $path $marker)) { return $true }
    }
    foreach ($dir in @('data', 'storage', 'backups')) {
        $candidate = Join-Path $path $dir
        if (Test-Path -LiteralPath $candidate) {
            $first = Get-ChildItem -LiteralPath $candidate -Recurse -File -ErrorAction SilentlyContinue |
                Select-Object -First 1
            if ($first) { return $true }
        }
    }
    return $false
}

function Write-Utf8([string]$path, [string]$text) {
    $dir = Split-Path -Parent $path
    if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}

Step 'Проверка инструментов сборки'
$env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
foreach ($tool in @('cargo', 'rustc', 'node', 'npm')) {
    $command = Get-Command $tool -ErrorAction SilentlyContinue
    if (-not $command) { Fail "Не найден $tool в PATH. Установите toolchain для сборочной машины." }
}
$rustVersion = ((& rustc --version) -join ' ').Trim()
$cargoVersion = ((& cargo --version) -join ' ').Trim()
$nodeVersion = ((& node --version) -join ' ').Trim()
$npmVersion = ((& npm --version) -join ' ').Trim()
Note $rustVersion
Note $cargoVersion
Note "node $nodeVersion, npm $npmVersion"

$rustParts = [regex]::Match($rustVersion, 'rustc (\d+)\.(\d+)').Groups
if ($rustParts.Count -ge 3) {
    $rustMajor = [int]$rustParts[1].Value
    $rustMinor = [int]$rustParts[2].Value
    if ($rustMajor -lt 1 -or ($rustMajor -eq 1 -and $rustMinor -lt 85)) {
        Fail "Нужен Rust 1.85+ (edition 2024), найден $rustVersion"
    }
}
$nodeMajor = [int](($nodeVersion -replace '^v', '') -split '\.')[0]
if ($nodeMajor -lt 20) { Fail "Нужен Node.js 20+, найден $nodeVersion" }

Step 'Проверка структуры проекта'
$required = @(
    'backend\Cargo.toml', 'backend\migrations', 'frontend\package.json',
    'runtime\Cargo.toml', 'launcher\Cargo.toml', 'standalone\Start.bat',
    'standalone\README-FIRST-RUN.txt', 'standalone\docs\STANDALONE.md'
)
foreach ($relative in $required) {
    if (-not (Test-Path -LiteralPath (Join-Path $repo $relative))) {
        Fail "Не найден обязательный элемент проекта: $relative"
    }
}

Step 'Проверка целевой папки и очистка staging'
Assert-SafePath $expectedTarget $expectedTarget
if (Test-Path -LiteralPath $expectedTarget) {
    if (Test-UserData $expectedTarget) {
        if (-not $Force) {
            Fail ("В $expectedTarget найдены пользовательские данные (config\secrets.json, data, storage или backups). " +
                  'Сборка не будет их удалять. Перенесите папку вручную или используйте -Force, если данные не нужны.')
        }
        Warn "Целевая папка содержит данные и будет заменена из-за -Force."
    }
}
if ($Clean -and (Test-Path -LiteralPath $buildRoot)) {
    Assert-SafePath (Join-Path $buildRoot 'NEXUS-Standalone-release') (Join-Path $buildRoot 'NEXUS-Standalone-release')
    Remove-Item -LiteralPath $buildRoot -Recurse -Force
}
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Step 'Frontend: typecheck и production build'
Push-Location (Join-Path $repo 'frontend')
try {
    if (-not (Test-Path 'node_modules')) {
        Note 'node_modules отсутствует — выполняется npm ci'
        & npm ci
        if ($LASTEXITCODE -ne 0) { Fail 'npm ci завершился с ошибкой' }
    }
    & npm run typecheck
    if ($LASTEXITCODE -ne 0) { Fail 'frontend typecheck завершился с ошибкой' }
    if (-not $SkipTests) {
        & npm test
        if ($LASTEXITCODE -ne 0) { Fail 'frontend tests завершились с ошибкой' }
        $testSummary += 'frontend: vitest run — PASS'
    }
    & npm run build
    if ($LASTEXITCODE -ne 0) { Fail 'frontend build завершился с ошибкой' }
} finally {
    Pop-Location
}
if (-not (Test-Path (Join-Path $repo 'frontend\dist\index.html'))) { Fail 'frontend\dist\index.html не создан' }

Step 'Backend: Rust check'
Invoke-Native 'cargo' @('check', '--all-targets', '--locked') (Join-Path $repo 'backend') 'cargo check (backend)'
$testSummary += 'backend: cargo check --all-targets — PASS'

if (-not $SkipTests) {
    Step 'Backend: tests (PostgreSQL nexus_test)'
    Invoke-Native 'cargo' @('test', '-j', '2', '--no-fail-fast') (Join-Path $repo 'backend') 'backend tests'
    $testSummary += 'backend: cargo test -j 2 — PASS'
}

Step 'Backend: release build'
Invoke-Native 'cargo' @('build', '--release', '--locked') (Join-Path $repo 'backend') 'cargo build --release (backend)'
$backendExe = Join-Path $repo 'backend\target\release\nexus.exe'
if (-not (Test-Path -LiteralPath $backendExe)) { Fail 'backend release exe не создан' }

Step 'AI Runtime: check, tests и release build'
Invoke-Native 'cargo' @('check', '--all-targets', '--locked') (Join-Path $repo 'runtime') 'cargo check (runtime)'
if (-not $SkipTests) {
    Invoke-Native 'cargo' @('test', '--all-targets') (Join-Path $repo 'runtime') 'runtime tests'
    $testSummary += 'AI Runtime: cargo test --all-targets — PASS'
}
Invoke-Native 'cargo' @('build', '--release', '--locked') (Join-Path $repo 'runtime') 'cargo build --release (runtime)'
$runtimeExe = Join-Path $repo 'runtime\target\release\nexus-ai-runtime.exe'
if (-not (Test-Path -LiteralPath $runtimeExe)) { Fail 'nexus-ai-runtime.exe не создан' }

Step 'Launcher: release build'
Invoke-Native 'cargo' @('build', '--release', '--locked') (Join-Path $repo 'launcher') 'cargo build --release (launcher)'
$launcherExe = Join-Path $repo 'launcher\target\release\NEXUS.exe'
if (-not (Test-Path -LiteralPath $launcherExe)) { Fail 'NEXUS.exe не создан' }
$testSummary += 'launcher: cargo build --release — PASS'

Step "PostgreSQL $PgVersion portable runtime"
if (-not $PgZip) { $PgZip = Join-Path $repo "build-cache\postgresql-$PgVersion-1-windows-x64-binaries.zip" }
if (-not (Test-Path -LiteralPath $PgZip)) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PgZip) | Out-Null
    $url = "https://get.enterprisedb.com/postgresql/postgresql-$PgVersion-1-windows-x64-binaries.zip"
    Note "Скачивание $url"
    & curl.exe -L --fail --silent --show-error -o $PgZip $url
    if ($LASTEXITCODE -ne 0) { Fail "Не удалось скачать PostgreSQL: $url" }
}
$pgSize = (Get-Item -LiteralPath $PgZip).Length
if ($pgSize -lt 250MB) { Fail "Архив PostgreSQL подозрительно мал: $pgSize байт" }
Note ("архив: {0:N0} байт" -f $pgSize)

Add-Type -AssemblyName System.IO.Compression.FileSystem
$databaseDir = Join-Path $stage 'database'
New-Item -ItemType Directory -Force -Path $databaseDir | Out-Null
$zip = [System.IO.Compression.ZipFile]::OpenRead($PgZip)
try {
    $extracted = 0
    foreach ($entry in $zip.Entries) {
        $match = [regex]::Match($entry.FullName, '^pgsql/(bin|lib|share)/(.+)$')
        if (-not $match.Success) { continue }
        $relative = $match.Groups[1].Value + '/' + $match.Groups[2].Value
        $destination = Join-Path $databaseDir ($relative.Replace('/', '\'))
        if ($entry.Name -eq '') {
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
        } else {
            $parent = Split-Path -Parent $destination
            New-Item -ItemType Directory -Force -Path $parent | Out-Null
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $destination, $true)
            $extracted++
        }
    }
    Note "извлечено файлов: $extracted"
} finally {
    $zip.Dispose()
}

foreach ($tool in @('postgres.exe', 'initdb.exe', 'pg_ctl.exe', 'psql.exe', 'pg_isready.exe', 'pg_dump.exe', 'pg_restore.exe', 'createdb.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $databaseDir "bin\$tool"))) {
        Fail "В portable PostgreSQL отсутствует $tool"
    }
}
if (-not (Test-Path -LiteralPath (Join-Path $databaseDir 'share\postgresql.conf.sample'))) {
    Fail 'В portable PostgreSQL отсутствует share\postgresql.conf.sample'
}

$pgLicenseSource = Join-Path $repo 'standalone\LICENSES\POSTGRESQL-LICENSE.txt'
Copy-Item -LiteralPath $pgLicenseSource -Destination (Join-Path $databaseDir 'server_license.txt') -Force
$pgThirdParty = 'C:\Program Files\PostgreSQL\17\commandlinetools_3rd_party_licenses.txt'
if (Test-Path -LiteralPath $pgThirdParty) {
    Copy-Item -LiteralPath $pgThirdParty -Destination (Join-Path $databaseDir 'commandlinetools_3rd_party_licenses.txt') -Force
} else {
    Warn 'Не найден commandlinetools_3rd_party_licenses.txt локального PostgreSQL; включена только PostgreSQL License.'
}
Write-Utf8 (Join-Path $databaseDir 'README-PORTABLE.txt') @"
Portable PostgreSQL $PgVersion для NEXUS WORKSPACE Standalone
=============================================================

Источник binaries: https://get.enterprisedb.com/postgresql/postgresql-$PgVersion-1-windows-x64-binaries.zip
Из архива включены только каталоги bin, lib и share (runtime-компоненты).
pgAdmin, StackBuilder, документация, include и примеры в релиз не входят.

Лицензия: PostgreSQL License (server_license.txt).
Сторонние компоненты: commandlinetools_3rd_party_licenses.txt (если включён).

Кластер создаётся лаунчером в ..\data\postgres при первом запуске.
Не запускайте postgres.exe вручную без -D: лаунчер управляет портом,
pg_hba.conf и остановкой.
"@

Step 'Копирование production-файлов'
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'backend') | Out-Null
Copy-Item -LiteralPath $backendExe -Destination (Join-Path $stage 'backend\nexus.exe') -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'frontend') | Out-Null
Copy-Item -Path (Join-Path $repo 'frontend\dist\*') -Destination (Join-Path $stage 'frontend') -Recurse -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'runtime') | Out-Null
Copy-Item -LiteralPath $runtimeExe -Destination (Join-Path $stage 'runtime\nexus-ai-runtime.exe') -Force
Copy-Item -LiteralPath (Join-Path $repo 'runtime\runtime.toml.example') -Destination (Join-Path $stage 'runtime\runtime.toml.example') -Force
foreach ($serviceScript in @('install-service.ps1', 'uninstall-service.ps1', 'update-service.ps1')) {
    $source = Join-Path $repo ('runtime\' + $serviceScript)
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $stage 'runtime') -Force }
}
Copy-Item -LiteralPath $launcherExe -Destination (Join-Path $stage 'NEXUS.exe') -Force
Copy-Item -Path (Join-Path $repo 'standalone\*') -Destination $stage -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repo 'runtime\README.md') -Destination (Join-Path $stage 'docs\AI_RUNTIME_FULL.md') -Force
Write-Utf8 (Join-Path $stage 'VERSION') "1.0.0`n"

Step 'Runtime DLL (Microsoft Visual C++)'
$dlls = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'concrt140.dll')
$foundDlls = @()
foreach ($dll in $dlls) {
    $source = Join-Path $env:SystemRoot "System32\$dll"
    if (Test-Path -LiteralPath $source) {
        $foundDlls += $dll
        foreach ($targetDir in @($stage, (Join-Path $stage 'backend'), (Join-Path $stage 'runtime'), (Join-Path $stage 'database\bin'))) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $targetDir $dll) -Force
        }
    }
}
foreach ($requiredDll in @('vcruntime140.dll', 'msvcp140.dll')) {
    if ($foundDlls -notcontains $requiredDll) { Fail "В System32 не найден $requiredDll — проверьте установленный VC++ Redistributable" }
}
Note ("скопировано: " + ($foundDlls -join ', '))

Step 'Лицензии и NOTICE'
$licenseTexts = @('Apache-2.0','MIT','BSD-2-Clause','BSD-3-Clause','ISC','Unicode-3.0','Zlib','BSL-1.0','CDLA-Permissive-2.0','MIT-0','Unlicense','0BSD','LLVM-exception','LGPL-2.1-or-later','ZPL-2.1','BlueOak-1.0.0','CC-BY-4.0','Python-2.0')
foreach ($id in $licenseTexts) {
    $source = Join-Path $repo "standalone\LICENSES\texts\$id.txt"
    if (-not (Test-Path -LiteralPath $source)) { Fail "Нет текста лицензии $id" }
    Copy-Item -LiteralPath $source -Destination (Join-Path $stage "LICENSES\texts\$id.txt") -Force
}

function Get-RustPackages([string]$manifestPath) {
    $raw = & cargo metadata --format-version 1 --locked --manifest-path $manifestPath 2>$null
    if ($LASTEXITCODE -ne 0) { Fail "cargo metadata не удался для $manifestPath" }
    return (($raw -join "`n") | ConvertFrom-Json).packages
}
$rustPackages = @{}
foreach ($manifest in @('backend\Cargo.toml', 'runtime\Cargo.toml', 'launcher\Cargo.toml')) {
    foreach ($package in (Get-RustPackages (Join-Path $repo $manifest))) {
        if (-not $package.name) { continue }
        $key = "$($package.name)@$($package.version)"
        $license = if ($package.license) { $package.license } else { 'unspecified (см. репозиторий пакета)' }
        $rustPackages[$key] = [pscustomobject]@{
            Name = $package.name
            Version = $package.version
            License = $license
            Repository = $package.repository
        }
    }
}
$rustLines = @('# Rust-зависимости NEXUS Standalone', '',
    'Включены в скомпилированные бинарники. Лицензии перечислены по метаданным Cargo;',
    'полные тексты стандартных лицензий — в LICENSES\texts\.', '',
    '| Пакет | Версия | Лицензия | Репозиторий |', '|---|---|---|---|')
foreach ($package in ($rustPackages.Values | Sort-Object Name, Version)) {
    $repoText = if ($package.Repository) { $package.Repository } else { '' }
    $rustLines += "| $($package.Name) | $($package.Version) | $($package.License) | $repoText |"
}
Write-Utf8 (Join-Path $stage 'LICENSES\RUST-DEPENDENCIES.md') (($rustLines -join "`r`n") + "`r`n")

$nodeScript = @'
const fs = require('fs');
const lock = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const seen = new Set();
for (const [path, value] of Object.entries(lock.packages || {})) {
  if (!value || !value.version) continue;
  let name = value.name;
  if (!name && path) {
    const marker = path.lastIndexOf('node_modules/');
    name = marker >= 0 ? path.slice(marker + 'node_modules/'.length) : path;
  }
  if (!name) continue;
  const key = name + '@' + value.version;
  if (seen.has(key)) continue;
  seen.add(key);
  const license = value.license || (path === '' ? 'MIT' : 'unspecified (см. пакет)');
  const dev = value.dev ? 'dev' : '';
  console.log([name, value.version, license, dev].join('\t'));
}
'@
$npmTsv = & node -e $nodeScript (Join-Path $repo 'frontend\package-lock.json')
if ($LASTEXITCODE -ne 0) { Fail 'Не удалось разобрать package-lock.json через Node.js' }
$npmLines = @('# npm-зависимости frontend', '',
    'Включены в production-бандл Vite. node_modules в релиз не входит.', '',
    '| Пакет | Версия | Лицензия | Dev |', '|---|---|---|---|')
foreach ($line in $npmTsv) {
    if (-not $line) { continue }
    $parts = $line -split "`t"
    if ($parts.Count -lt 3) { continue }
    $npmLines += "| $($parts[0]) | $($parts[1]) | $($parts[2]) | $($parts[3]) |"
}
Write-Utf8 (Join-Path $stage 'LICENSES\NPM-DEPENDENCIES.md') (($npmLines -join "`r`n") + "`r`n")

$notice = @"
NEXUS WORKSPACE 1.0.0 — сторонние компоненты
=============================================

NEXUS code: MIT License — LICENSES\NEXUS-LICENSE.txt
PostgreSQL ${PgVersion}: PostgreSQL License — LICENSES\POSTGRESQL-LICENSE.txt,
  источник https://get.enterprisedb.com/postgresql/postgresql-$PgVersion-1-windows-x64-binaries.zip
PostgreSQL third-party CLI: database\commandlinetools_3rd_party_licenses.txt
PostgreSQL bundled runtime libraries (Windows x64 EDB build):
  OpenSSL 3 (libssl/libcrypto)            Apache-2.0
  ICU 67                                  Unicode-3.0
  zlib                                    Zlib
  libxml2                                 MIT
  zstd                                    BSD-3-Clause
  lz4                                     BSD-2-Clause
  GNU libintl / libiconv                  LGPL-2.1-or-later
  mingw-w64 runtime (libwinpthread)       ZPL-2.1
  Sources: https://www.postgresql.org/ , the EDB binaries URL above and the
  respective upstream projects. These DLLs are unmodified runtime components.
Rust crates: LICENSES\RUST-DEPENDENCIES.md + LICENSES\texts\
npm packages: LICENSES\NPM-DEPENDENCIES.md + LICENSES\texts\
Microsoft VC++ runtime DLL: LICENSES\MICROSOFT-VC-RUNTIME.txt

Тексты лицензий: LICENSES\texts\ (SPDX license-list-data).
Компоненты с несовместимыми лицензиями в релиз не включены.
"@
Write-Utf8 (Join-Path $stage 'LICENSES\THIRD-PARTY-NOTICES.md') $notice

Step 'Проверка состава релиза'
foreach ($dir in @('node_modules', '.git', 'target', 'src', 'playwright-report', 'test-results')) {
    $candidate = Join-Path $stage $dir
    if (Test-Path -LiteralPath $candidate) { Fail "Release contains development directory: $candidate" }
}
foreach ($name in @('Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json', 'vite.config.ts', 'tsconfig.json', 'tsconfig.app.json', 'tsconfig.node.json', 'playwright.config.ts', 'vitest.config.ts', '.env')) {
    $hit = Get-ChildItem -LiteralPath $stage -Recurse -File -Filter $name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($hit) { Fail "Release contains development file: $($hit.FullName)" }
}
foreach ($pattern in @('*.rs', '*.ts', '*.tsx', '*.pdb', '*.rlib')) {
    $hit = Get-ChildItem -LiteralPath $stage -Recurse -File -Filter $pattern -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($hit) { Fail "Release contains development file: $($hit.FullName)" }
}
foreach ($dir in @('backend', 'frontend', 'database', 'runtime', 'config', 'docs', 'LICENSES', 'tools')) {
    if (-not (Test-Path -LiteralPath (Join-Path $stage $dir))) { Fail "В релизе отсутствует каталог $dir" }
}
$indexHtml = Get-Content -LiteralPath (Join-Path $stage 'frontend\index.html') -Raw
foreach ($assetMatch in [regex]::Matches($indexHtml, '(?:src|href)="(/assets/[^"]+)"')) {
    $asset = $assetMatch.Groups[1].Value.TrimStart('/').Replace('/', '\')
    if (-not (Test-Path -LiteralPath (Join-Path $stage "frontend\$asset"))) {
        Fail "frontend ссылается на отсутствующий asset: $asset"
    }
}
$devRefs = Get-ChildItem -LiteralPath (Join-Path $stage 'frontend\assets') -File -Filter *.js |
    Select-String -Pattern 'localhost:5173|127\.0\.0\.1:5173' -SimpleMatch:$false
if ($devRefs) { Fail 'В production frontend найден адрес Vite dev-server (5173)' }
$textExtensions = @('.txt', '.md', '.bat', '.toml', '.json', '.example', '.ps1')
$textFiles = Get-ChildItem -LiteralPath $stage -Recurse -File -ErrorAction SilentlyContinue |
    Where-Object { $textExtensions -contains $_.Extension }
$absoluteRefs = $textFiles | Select-String -Pattern 'C:\\Users\\max|MWS-NEXUS' -SimpleMatch:$false
if ($absoluteRefs) { Fail ("В текстовых файлах релиза найден абсолютный путь сборки:`n" + (($absoluteRefs | Select-Object -First 10 | ForEach-Object { $_.Path }) -join "`n")) }

Step 'Проверка запуска бинарников'
$launcherVersion = (& (Join-Path $stage 'NEXUS.exe') --version 2>&1) -join ' '
if ($LASTEXITCODE -ne 0 -or $launcherVersion -notmatch '1\.0\.0') { Fail "NEXUS.exe --version: $launcherVersion" }
Note $launcherVersion.Trim()
$env:DATABASE_URL = 'postgres://nexus:buildcheck@127.0.0.1:1/nexus'
$env:NEXUS_SECRET = 'build-check-secret-build-check-secret-0000'
$backendVersion = (& (Join-Path $stage 'backend\nexus.exe') --version 2>&1) -join ' '
if ($LASTEXITCODE -ne 0 -or $backendVersion -notmatch '1\.0\.0') { Fail "backend --version: $backendVersion" }
Note $backendVersion.Trim()
Remove-Item Env:DATABASE_URL -ErrorAction SilentlyContinue
Remove-Item Env:NEXUS_SECRET -ErrorAction SilentlyContinue
$pgVersionOutput = (& (Join-Path $stage 'database\bin\postgres.exe') --version 2>&1) -join ' '
if ($LASTEXITCODE -ne 0 -or $pgVersionOutput -notmatch '17\.') { Fail "postgres --version: $pgVersionOutput" }
Note $pgVersionOutput.Trim()
$runtimeVersion = (& (Join-Path $stage 'runtime\nexus-ai-runtime.exe') --version 2>&1) -join ' '
if ($LASTEXITCODE -ne 0) { Warn "nexus-ai-runtime.exe --version вернул $LASTEXITCODE" } else { Note $runtimeVersion.Trim() }

Step 'Build report и manifest'
$totalFiles = (Get-ChildItem -LiteralPath $stage -Recurse -File | Measure-Object).Count
$totalBytes = (Get-ChildItem -LiteralPath $stage -Recurse -File | Measure-Object Length -Sum).Sum
$testsText = if ($testSummary.Count) {
    ($testSummary | ForEach-Object { "  - $_" }) -join "`r`n"
} else {
    '  - пропущены (-SkipTests)'
}
$warningsText = if ($warnings.Count) {
    ($warnings | ForEach-Object { "  - $_" }) -join "`r`n"
} else {
    '  - нет'
}
$report = @"
NEXUS WORKSPACE 1.0.0 — STANDALONE BUILD REPORT
================================================

Собрано:          $($started.ToString('yyyy-MM-dd HH:mm:ss zzz'))
Длительность:     $([int]((Get-Date) - $started).TotalMinutes) мин
Целевая папка:    $expectedTarget
Платформа:        Windows 11 x64 (сборка на $([System.Environment]::OSVersion.VersionString))
Rust:             $rustVersion
Cargo:            $cargoVersion
Node.js:          $nodeVersion
npm:              $npmVersion
PostgreSQL:       $PgVersion (EDB Windows x64 binaries)
Backend:          production, cargo build --release
Frontend:         production, Vite + TypeScript
AI Runtime:       nexus-ai-runtime.exe (release), внешняя PolyStrata/enrollment
Launcher:         native NEXUS.exe (Win32/GDI, Rust)
Файлов:           $totalFiles
Размер (до checksum-файлов): $("{0:N0}" -f $totalBytes) байт

Тесты:
$testsText

Предупреждения:
$warningsText

Состав:
  NEXUS.exe, backend\nexus.exe, frontend\, database\ (PostgreSQL $PgVersion),
  runtime\nexus-ai-runtime.exe, config\ (templates), docs\, LICENSES\,
  tools\, BAT-сценарии, README-FIRST-RUN.txt.

Ограничения:
  - Чистая виртуальная машина Windows 11 без dev-инструментов недоступна
    в среде сборки; независимость от dev-среды проверяется запуском
    с очищенным PATH и после переноса папки.
  - AI Runtime требует внешней PolyStrata и enrollment; см. docs\AI_RUNTIME.md.
  - Автоматическое обновление и Windows-служба не входят в базовый режим.
"@
Write-Utf8 (Join-Path $stage 'BUILD-REPORT.txt') $report

$fileHashes = [ordered]@{}
foreach ($file in (Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName)) {
    $relative = $file.FullName.Substring($stage.Length).TrimStart('\').Replace('\', '/')
    if ($relative -eq 'manifest.json' -or $relative -eq 'SHA256SUMS') { continue }
    $fileHashes[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
$manifest = [ordered]@{
    product = 'NEXUS WORKSPACE'
    version = '1.0.0'
    builtAt = (Get-Date).ToString('o')
    platform = 'windows-x64'
    postgres = $PgVersion
    rust = $rustVersion
    node = $nodeVersion
    files = $fileHashes
}
Write-Utf8 (Join-Path $stage 'manifest.json') ($manifest | ConvertTo-Json -Depth 6)
Note "manifest.json: $($fileHashes.Count) файлов"

Step 'SHA256SUMS'
$sumLines = Get-ChildItem -LiteralPath $stage -Recurse -File |
    Where-Object { $_.Name -ne 'SHA256SUMS' } |
    Sort-Object FullName |
    ForEach-Object {
        $relative = $_.FullName.Substring($stage.Length).TrimStart('\').Replace('\', '/')
        '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $relative
    }
$sumLines | Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS') -Encoding ASCII
Note ("строк: " + $sumLines.Count)

Step 'Финальная проверка и перенос релиза'
$finalIndex = Join-Path $stage 'frontend\index.html'
if (-not (Test-Path -LiteralPath $finalIndex)) { Fail 'frontend\index.html отсутствует в staging' }
if (-not (Test-Path -LiteralPath (Join-Path $stage 'LICENSES'))) { Fail 'LICENSES отсутствует' }

Assert-SafePath $expectedTarget $expectedTarget
if (Test-Path -LiteralPath $expectedTarget) {
    if ((Test-UserData $expectedTarget) -and -not $Force) {
        Fail 'Пользовательские данные появились в целевой папке во время сборки; перенос отменён.'
    }
    Remove-Item -LiteralPath $expectedTarget -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $expectedTarget) | Out-Null
Move-Item -LiteralPath $stage -Destination $expectedTarget

$finalFiles = (Get-ChildItem -LiteralPath $expectedTarget -Recurse -File | Measure-Object).Count
$finalBytes = (Get-ChildItem -LiteralPath $expectedTarget -Recurse -File | Measure-Object Length -Sum).Sum
Step 'Готово'
Write-Host "  Релиз:    $expectedTarget"
Write-Host ("  Файлов:   {0}" -f $finalFiles)
Write-Host ("  Размер:   {0:N0} байт ({1:N1} МБ)" -f $finalBytes, ($finalBytes / 1MB))
Write-Host "  Проверка: powershell -ExecutionPolicy Bypass -File tools\Verify-Integrity.ps1"
Write-Host "  Запуск:   NEXUS.exe или Start.bat"
if ($warnings.Count) {
    Write-Host ''
    Write-Host '  Предупреждения:' -ForegroundColor Yellow
    $warnings | ForEach-Object { Write-Host "   - $_" -ForegroundColor Yellow }
}
