# NEXUS WORKSPACE - проверка портов перед установкой/обновлением Windows.
#
# Проверяет:
#   * кто слушает порт (PID, процесс, признак HTTP.sys/IIS);
#   * исключённые диапазоны TCP Hyper-V/WSL (`netsh interface ipv4/ipv6
#     show excludedportrange protocol=tcp`) — на Windows 11 порт из такого
#     диапазона не биндится даже администратором;
#   * URL ACL HTTP.sys и активные URL на проверяемых портах;
#   * фактический bind (TcpListener), чтобы отличить «занято» от
#     «зарезервировано/отказано в доступе».
#
# Код возврата: 0 — критичных проблем нет; 1 — порт исключён, bind отклонён
# (AccessDenied) или -RequireFree, а порт занят.
#
# Примеры:
#   powershell -File deployment\check-ports.ps1
#   powershell -File deployment\check-ports.ps1 -Ports 80,443 -Json
#   powershell -File deployment\check-ports.ps1 -Ports 8080 -RequireFree

[CmdletBinding()]
param(
    # int[] из вызова в PowerShell или строка "80,443" из командной строки.
    [object]$Ports = @(80, 443, 8080),
    [switch]$RequireFree,
    [switch]$Json,
    [switch]$NoBindTest
)

$ErrorActionPreference = 'Stop'

function ConvertTo-PortList {
    param([object]$Value)
    $result = New-Object System.Collections.Generic.List[int]
    foreach ($item in @($Value)) {
        foreach ($part in ([string]$item -split '[,\s]+')) {
            if ($part -match '^\d{1,5}$') {
                $port = [int]$part
                if ($port -ge 1 -and $port -le 65535) { $result.Add($port) }
            }
        }
    }
    return @($result | Sort-Object -Unique)
}

$Ports = ConvertTo-PortList $Ports
if ($Ports.Count -eq 0) { throw 'Не задан ни один корректный порт.' }

function Get-ExcludedRanges {
    param([string]$Family)
    $ranges = New-Object System.Collections.Generic.List[object]
    $lines = & netsh interface $Family show excludedportrange protocol=tcp 2>$null
    foreach ($line in $lines) {
        # Строка вида " 18091 18092 *" (звёздочка — управляемое исключение).
        if ($line -match '^\s*(\d{1,5})\s+(\d{1,5})(?:\s+\*)?\s*$') {
            $ranges.Add([pscustomobject]@{
                    start  = [int]$Matches[1]
                    end    = [int]$Matches[2]
                    family = $Family
                })
        }
    }
    return $ranges
}

function Get-HttpSysUrls {
    $urls = New-Object System.Collections.Generic.List[string]
    $text = & netsh http show servicestate view=session 2>$null
    foreach ($line in $text) {
        foreach ($match in [regex]::Matches($line, '(?i)https?://[^\s]+')) {
            $urls.Add($match.Value.TrimEnd('/', ')'))
        }
    }
    return $urls
}

function Get-UrlAclPorts {
    $ports = @{}
    $text = & netsh http show urlacl 2>$null
    foreach ($line in $text) {
        foreach ($match in [regex]::Matches($line, '(?i)https?://[^\s]+')) {
            if ($match.Value -match ':(\d+)') {
                $ports[[int]$Matches[1]] = $true
            }
        }
    }
    return $ports
}

function Test-PortBind {
    param([int]$Port)
    $listener = $null
    try {
        $listener = New-Object System.Net.Sockets.TcpListener([Net.IPAddress]::Any, $Port)
        $listener.Start()
        return [pscustomobject]@{ ok = $true; error = ''; message = '' }
    } catch [System.Net.Sockets.SocketException] {
        return [pscustomobject]@{
            ok      = $false
            error   = $_.Exception.SocketErrorCode.ToString()
            message = $_.Exception.Message
        }
    } catch {
        return [pscustomobject]@{
            ok      = $false
            error   = $_.Exception.GetType().Name
            message = $_.Exception.Message
        }
    } finally {
        if ($listener) { try { $listener.Stop() } catch { } }
    }
}

$excluded = @()
$excluded += Get-ExcludedRanges 'ipv4'
$excluded += Get-ExcludedRanges 'ipv6'
$httpUrls = Get-HttpSysUrls
$urlAclPorts = Get-UrlAclPorts

$iis = Get-Service W3SVC -ErrorAction SilentlyContinue
$httpSysService = Get-Service HTTP -ErrorAction SilentlyContinue

$listeners = @()
try {
    $listeners = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
        Where-Object { $Ports -contains [int]$_.LocalPort })
} catch {
    $listeners = @()
}

$report = New-Object System.Collections.Generic.List[object]
$fatal = $false
$warnings = New-Object System.Collections.Generic.List[string]

foreach ($port in ($Ports | Sort-Object -Unique)) {
    $portListeners = @($listeners | Where-Object { [int]$_.LocalPort -eq $port })
    $portExcluded = @($excluded | Where-Object { $port -ge $_.start -and $port -le $_.end })
    $portHttpUrls = @($httpUrls | Where-Object { $_ -match ":$port([/:]|$)" })
    $hasAcl = $urlAclPorts.ContainsKey($port)

    $bind = [pscustomobject]@{ ok = $null; error = ''; message = '' }
    if (-not $NoBindTest) {
        $bind = Test-PortBind $port
    }

    $details = New-Object System.Collections.Generic.List[object]
    foreach ($connection in $portListeners) {
        $processName = '?'
        $processPath = ''
        try {
            $process = Get-Process -Id $connection.OwningProcess -ErrorAction Stop
            $processName = $process.ProcessName
            $processPath = $process.Path
        } catch { }
        if ($connection.OwningProcess -eq 4) {
            $processName = 'System (HTTP.sys)'
        }
        $details.Add([pscustomobject]@{
                address = [string]$connection.LocalAddress
                pid     = [int]$connection.OwningProcess
                process = $processName
                path    = $processPath
            })
    }

    $status = 'free'
    if ($portExcluded.Count -gt 0) {
        $status = 'excluded'
        $fatal = $true
    } elseif (-not $NoBindTest -and -not $bind.ok -and $bind.error -eq 'AccessDenied') {
        $status = 'bind-denied'
        $fatal = $true
    } elseif ($portListeners.Count -gt 0) {
        $status = 'listening'
    } elseif (-not $NoBindTest -and -not $bind.ok) {
        $status = 'bind-failed'
    }

    if ($RequireFree -and $portListeners.Count -gt 0) {
        $status = 'busy'
        $fatal = $true
    }
    if ($hasAcl -and $portListeners.Count -eq 0 -and $port -in @(80, 443)) {
        $warnings.Add("порт $port имеет URL ACL HTTP.sys; для HttpListener нужен отдельный urlacl, для обычного TCP-сокета это не блокирует bind")
    }
    if ($iis -and $iis.Status -eq 'Running' -and $port -in @(80, 443) -and $portListeners.Count -eq 0) {
        $warnings.Add("IIS (W3SVC) запущена; если она займёт $port позже, Caddy/NEXUS не сможет выполнить bind")
    }

    $report.Add([pscustomobject]@{
            port           = $port
            status         = $status
            listeners      = $details
            excludedRanges = $portExcluded
            httpSysUrls    = $portHttpUrls
            hasUrlAcl      = $hasAcl
            bind           = $bind
        })
}

$summary = [pscustomobject]@{
    ok             = -not $fatal
    checkedAt      = (Get-Date).ToString('o')
    ports          = $report
    excludedRanges = $excluded
    httpSys        = [pscustomobject]@{
        service = if ($httpSysService) { [string]$httpSysService.Status } else { 'missing' }
        urls    = $httpUrls
    }
    iis            = [pscustomobject]@{
        installed = [bool]$iis
        status    = if ($iis) { [string]$iis.Status } else { 'not-installed' }
    }
    warnings       = $warnings
}

if ($Json) {
    $summary | ConvertTo-Json -Depth 8
} else {
    Write-Host 'NEXUS WORKSPACE - проверка портов' -ForegroundColor Cyan
    Write-Host ("Исключённые диапазоны TCP: " + $(if ($excluded.Count) {
                ($excluded | ForEach-Object { "$($_.family) $($_.start)-$($_.end)" }) -join ', '
            } else { 'нет' }))
    Write-Host ("HTTP.sys: " + $summary.httpSys.service + "; активных URL: " + $httpUrls.Count)
    Write-Host ("IIS: " + $summary.iis.status)
    Write-Host ''
    foreach ($item in $report) {
        $color = switch ($item.status) {
            'free' { 'Green' }
            'listening' { 'Yellow' }
            'excluded' { 'Red' }
            'bind-denied' { 'Red' }
            default { 'Red' }
        }
        Write-Host ("Порт {0}: {1}" -f $item.port, $item.status) -ForegroundColor $color
        foreach ($listener in $item.listeners) {
            Write-Host ("  слушает {0} PID {1} {2}" -f $listener.address, $listener.pid, $listener.process)
            if ($listener.path) { Write-Host ("    " + $listener.path) }
        }
        foreach ($range in $item.excludedRanges) {
            Write-Host ("  исключён диапазоном {0}-{1} ({2})" -f $range.start, $range.end, $range.family) -ForegroundColor Red
        }
        if ($item.hasUrlAcl) { Write-Host '  есть URL ACL HTTP.sys' }
        if ($item.bind.ok -eq $true) { Write-Host '  bind: OK' -ForegroundColor Green }
        elseif ($item.bind.ok -eq $false) {
            Write-Host ("  bind: {0} — {1}" -f $item.bind.error, $item.bind.message) -ForegroundColor Red
        }
    }
    foreach ($warning in $warnings) {
        Write-Host ('  внимание: ' + $warning) -ForegroundColor Yellow
    }
    Write-Host ''
    if ($summary.ok) {
        Write-Host 'Порты пригодны для запуска.' -ForegroundColor Green
    } else {
        Write-Host 'Есть критичная проблема с портом: освободите порт/диапазон или выберите другой.' -ForegroundColor Red
    }
}

if (-not $summary.ok) { exit 1 }
exit 0
