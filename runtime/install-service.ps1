<#
.SYNOPSIS
  Устанавливает NEXUS AI Runtime как службу Windows.

.DESCRIPTION
  Требуется запуск от имени администратора. Служба запускает тот же executable
  с `run --service`, то есть без входящего HTTP-порта. Device token хранится в
  state\device.json; каталог состояния получает ограниченный ACL.

.PARAMETER ServiceName
  Имя службы (по умолчанию NexusAIRuntime).

.PARAMETER BinaryPath
  Путь к nexus-ai-runtime.exe.

.PARAMETER ConfigPath
  Путь к runtime.toml.

.PARAMETER ServiceAccount
  Необязательная непривилегированная учётная запись, например ".\NexusAIRuntime".

.PARAMETER Password
  Пароль service account (SecureString не поддерживается sc.exe; передавайте через
  защищённый канал/интерактивный ввод).

.EXAMPLE
  .\install-service.ps1 -BinaryPath C:\NexusAI\nexus-ai-runtime.exe -ConfigPath C:\NexusAI\runtime.toml
#>
[CmdletBinding()]
param(
  [string]$ServiceName = "NexusAIRuntime",
  [string]$BinaryPath = "$PSScriptRoot\nexus-ai-runtime.exe",
  [string]$ConfigPath = "C:\NexusAI\runtime.toml",
  [string]$ServiceAccount = "",
  [string]$Password = "",
  [switch]$SkipStart
)

$ErrorActionPreference = "Stop"

function Assert-Admin {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Запустите PowerShell от имени администратора."
  }
}

Assert-Admin

$BinaryPath = (Resolve-Path $BinaryPath).Path
$ConfigPath = (Resolve-Path $ConfigPath).Path
if (-not (Test-Path $BinaryPath -PathType Leaf)) { throw "Не найден $BinaryPath" }
if (-not (Test-Path $ConfigPath -PathType Leaf)) { throw "Не найден $ConfigPath" }

$configText = Get-Content $ConfigPath -Raw
if ($configText -match '(?m)^\s*allow_insecure_ws\s*=\s*true') {
  Write-Warning "В конфигурации разрешён ws://. Для production используйте wss:// и allow_insecure_ws = false."
}

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
  throw "Служба $ServiceName уже существует. Используйте update-service.ps1 или uninstall-service.ps1."
}

$configDir = Split-Path $ConfigPath -Parent
$stateDir = Join-Path $configDir "state"
$logDir = Join-Path $configDir "logs"
New-Item -ItemType Directory -Force -Path $stateDir, $logDir | Out-Null

# Ограничиваем доступ к каталогу состояния и журналам.
$aclUser = if ($ServiceAccount) { $ServiceAccount } else { "$env:USERDOMAIN\$env:USERNAME" }
& icacls $stateDir /inheritance:r /grant:r "${aclUser}:(OI)(CI)F" /grant:r "SYSTEM:(OI)(CI)F" | Out-Null
& icacls $logDir /inheritance:r /grant:r "${aclUser}:(OI)(CI)F" /grant:r "SYSTEM:(OI)(CI)F" | Out-Null

$binPath = "`"$BinaryPath`" run --service --service-name `"$ServiceName`" --config `"$ConfigPath`""
$arguments = @("create", $ServiceName, "binPath=", $binPath, "start=", "auto", "DisplayName=", "NEXUS AI Runtime")
if ($ServiceAccount) {
  if (-not $Password) { throw "Для ServiceAccount требуется -Password." }
  $arguments += @("obj=", $ServiceAccount, "password=", $Password)
}
& sc.exe @arguments | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc.exe create завершился с кодом $LASTEXITCODE" }

& sc.exe description $ServiceName "NEXUS AI Runtime: исходящее WSS-соединение с NEXUS, локальная PolyStrata и инструменты в изолированных workspace." | Out-Null
& sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/15000/restart/60000 | Out-Null

if (-not $SkipStart) {
  Start-Service -Name $ServiceName
  Start-Sleep -Seconds 2
  Get-Service -Name $ServiceName | Format-Table Name, Status, StartType
}

Write-Host "Служба $ServiceName установлена."
Write-Host "Проверка:  nexus-ai-runtime check --config `"$ConfigPath`" --json"
Write-Host "Журнал:    $logDir"
