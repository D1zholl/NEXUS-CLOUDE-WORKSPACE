<#
.SYNOPSIS
  Удаляет службу NEXUS AI Runtime.

.PARAMETER RemoveData
  Удалить state и logs (device token будет потерян; потребуется новый enrollment).
#>
[CmdletBinding()]
param(
  [string]$ServiceName = "NexusAIRuntime",
  [switch]$RemoveData
)

$ErrorActionPreference = "Stop"
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "Запустите PowerShell от имени администратора."
}

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
  Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
  & sc.exe delete $ServiceName | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "sc.exe delete завершился с кодом $LASTEXITCODE" }
  Write-Host "Служба $ServiceName удалена."
} else {
  Write-Host "Служба $ServiceName не найдена."
}

if ($RemoveData) {
  $dataRoot = "C:\NexusAI"
  if (Test-Path $dataRoot) {
    Write-Warning "Удаление $dataRoot (state, logs, workspaces!)."
    Remove-Item -Recurse -Force $dataRoot
  }
}
