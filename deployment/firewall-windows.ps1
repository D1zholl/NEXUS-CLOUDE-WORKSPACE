# NEXUS WORKSPACE - входящие правила Windows Firewall.
#
# По умолчанию открывает 80/443 для Caddy/обратного прокси. Правило для
# backend-порта (8080) создаётся только при явном -ExposeBackend: при
# локальном прокси оно не нужно, при удалённом ограничивается -ProxyAddress.
#
# Скрипт идемпотентен: существующие правила NEXUS удаляются и создаются заново
# с актуальными портами/профилем. -Remove удаляет только правила с префиксом.
#
# Примеры:
#   powershell -File deployment\firewall-windows.ps1
#   powershell -File deployment\firewall-windows.ps1 -HttpPort 8080 -SkipHttps
#   powershell -File deployment\firewall-windows.ps1 -ExposeBackend -ProxyAddress 10.0.0.10
#   powershell -File deployment\firewall-windows.ps1 -Remove

[CmdletBinding()]
param(
    [int]$HttpPort = 80,
    [int]$HttpsPort = 443,
    [int]$BackendPort = 8080,
    [switch]$SkipHttp,
    [switch]$SkipHttps,
    [switch]$ExposeBackend,
    # Для -ExposeBackend: адрес(а) прокси, например 10.0.0.10 или 10.0.0.0/24.
    [string[]]$ProxyAddress = @('Any'),
    [ValidateSet('Any', 'Domain', 'Private', 'Public')]
    [string]$FirewallProfile = 'Any',
    [string]$DisplayNamePrefix = 'NEXUS WORKSPACE',
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Запустите скрипт от имени администратора.'
}

function New-NexusRuleName {
    param([string]$Kind, [int]$Port)
    return ("{0} - {1} {2}" -f $DisplayNamePrefix, $Kind, $Port)
}

function Sync-NexusFirewallRule {
    param(
        [string]$Kind,
        [int]$Port,
        [string[]]$RemoteAddress
    )
    $name = New-NexusRuleName -Kind $Kind -Port $Port
    $existing = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
    if ($Remove) {
        if ($existing) {
            Remove-NetFirewallRule -DisplayName $name
            Write-Host ("  удалено: {0}" -f $name)
        }
        return
    }

    # Пересоздаём правило, чтобы обновить порт/профиль/remote address.
    if ($existing) {
        Remove-NetFirewallRule -DisplayName $name
    }
    $params = @{
        DisplayName  = $name
        Description  = 'NEXUS WORKSPACE (создано deployment/firewall-windows.ps1)'
        Direction    = 'Inbound'
        Action       = 'Allow'
        Protocol     = 'TCP'
        LocalPort    = $Port
        Profile      = $FirewallProfile
        RemoteAddress = $RemoteAddress
        Enabled      = 'True'
    }
    New-NetFirewallRule @params | Out-Null
    Write-Host ("  разрешено: {0} → TCP {1}" -f $name, $Port)
}

if ($Remove) {
    Write-Host 'Удаление правил NEXUS WORKSPACE' -ForegroundColor Cyan
} else {
    Write-Host 'Правила Windows Firewall для NEXUS WORKSPACE' -ForegroundColor Cyan
}

if (-not $SkipHttp) {
    Sync-NexusFirewallRule -Kind 'HTTP' -Port $HttpPort -RemoteAddress @('Any')
}
if (-not $SkipHttps) {
    Sync-NexusFirewallRule -Kind 'HTTPS' -Port $HttpsPort -RemoteAddress @('Any')
}
if ($ExposeBackend -or $Remove) {
    # При -Remove пробуем удалить и backend-правило (укажите фактический
    # -BackendPort, если он отличается от 8080).
    $remote = if ($ProxyAddress -and $ProxyAddress.Count -gt 0) { $ProxyAddress } else { @('Any') }
    Sync-NexusFirewallRule -Kind 'backend' -Port $BackendPort -RemoteAddress $remote
} else {
    Write-Host '  backend-порт не открыт (локальный прокси): используйте -ExposeBackend при удалённом прокси.'
}

if ($Remove) {
    Write-Host 'Готово.' -ForegroundColor Green
} else {
    Write-Host ''
    Write-Host 'Проверка:' -ForegroundColor Cyan
    Get-NetFirewallRule -DisplayName "$DisplayNamePrefix*" -ErrorAction SilentlyContinue |
        Select-Object DisplayName, Enabled, Direction, Action |
        Format-Table -AutoSize
}
