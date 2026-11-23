<#
.SYNOPSIS
  Обновляет бинарник NEXUS AI Runtime и перезапускает службу.

.PARAMETER ServiceName
  Имя службы.

.PARAMETER BinaryPath
  Новый nexus-ai-runtime.exe.

.PARAMETER ConfigPath
  Путь к runtime.toml (для проверки после обновления).

.PARAMETER SkipCheck
  Не выполнять `check` после обновления.
#>
[CmdletBinding()]
param(
  [string]$ServiceName = "NexusAIRuntime",
  [string]$BinaryPath = "$PSScriptRoot\nexus-ai-runtime.exe",
  [string]$ConfigPath = "C:\NexusAI\runtime.toml",
  [switch]$SkipCheck
)

$ErrorActionPreference = "Stop"
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "Запустите PowerShell от имени администратора."
}

$BinaryPath = (Resolve-Path $BinaryPath).Path
if (-not (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)) {
  throw "Служба $ServiceName не установлена."
}

$service = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
if (-not $service) { throw "Не удалось прочитать службу $ServiceName" }
$current = $service.PathName
if ($current -notmatch '^"([^"]+)"') { throw "Не удалось определить текущий executable службы." }
$target = $Matches[1]
$targetDir = Split-Path $target -Parent
$backup = Join-Path $targetDir ("nexus-ai-runtime.{0}.bak.exe" -f (Get-Date -Format "yyyyMMddHHmmss"))

Stop-Service -Name $ServiceName -Force
Copy-Item $target $backup -Force
Copy-Item $BinaryPath $target -Force
Start-Service -Name $ServiceName

if (-not $SkipCheck) {
  Start-Sleep -Seconds 2
  & $target check --config $ConfigPath --json
  if ($LASTEXITCODE -ne 0) {
    Write-Warning "Проверка после обновления не прошла (код $LASTEXITCODE). Backup: $backup"
  }
}

Write-Host "Обновление завершено. Backup: $backup"
