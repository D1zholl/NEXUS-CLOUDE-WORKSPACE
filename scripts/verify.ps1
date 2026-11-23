<#
.SYNOPSIS
    Локальный quality-gate NEXUS WORKSPACE (замена CI).

.DESCRIPTION
    Последовательно выполняет проверки backend и frontend:
    fmt, clippy, test, typecheck, lint, vitest, build.
    Любая упавшая фаза останавливает прогон с её кодом выхода.

.PARAMETER SkipBackend
    Не запускать проверки Rust (например, без установленного PostgreSQL).

.PARAMETER SkipFrontend
    Не запускать проверки frontend.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\verify.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipBackend,
    [switch]$SkipFrontend
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$script:failures = New-Object System.Collections.Generic.List[string]

function Invoke-Phase {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )

    Write-Host ''
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $started = Get-Date
    Push-Location $WorkingDirectory
    try {
        & $Action
        if ($LASTEXITCODE -ne 0) {
            throw "команда завершилась с кодом $LASTEXITCODE"
        }
        $elapsed = (Get-Date) - $started
        Write-Host ("OK  {0} ({1:n1} с)" -f $Name, $elapsed.TotalSeconds) -ForegroundColor Green
    }
    catch {
        $elapsed = (Get-Date) - $started
        Write-Host ("FAIL {0} ({1:n1} с): {2}" -f $Name, $elapsed.TotalSeconds, $_) -ForegroundColor Red
        $script:failures.Add($Name)
    }
    finally {
        Pop-Location
    }
}

if (-not $SkipBackend) {
    Invoke-Phase -Name 'backend: fmt' -WorkingDirectory (Join-Path $root 'backend') -Action {
        cargo fmt --all -- --check
    }
    Invoke-Phase -Name 'backend: clippy' -WorkingDirectory (Join-Path $root 'backend') -Action {
        cargo clippy -j 2 --all-targets -- -D warnings
    }
    Invoke-Phase -Name 'backend: test' -WorkingDirectory (Join-Path $root 'backend') -Action {
        cargo test -j 2
    }
}

if (-not $SkipFrontend) {
    $frontend = Join-Path $root 'frontend'
    Invoke-Phase -Name 'frontend: typecheck' -WorkingDirectory $frontend -Action {
        npm run typecheck
    }
    Invoke-Phase -Name 'frontend: lint' -WorkingDirectory $frontend -Action {
        npm run lint
    }
    Invoke-Phase -Name 'frontend: format:check' -WorkingDirectory $frontend -Action {
        npm run format:check
    }
    Invoke-Phase -Name 'frontend: test' -WorkingDirectory $frontend -Action {
        npm run test
    }
    Invoke-Phase -Name 'frontend: build' -WorkingDirectory $frontend -Action {
        npm run build
    }
}

Write-Host ''
if ($script:failures.Count -eq 0) {
    Write-Host 'Все проверки пройдены.' -ForegroundColor Green
    exit 0
}

Write-Host ("Не пройдено фаз: {0}" -f $script:failures.Count) -ForegroundColor Red
foreach ($failure in $script:failures) {
    Write-Host "  - $failure" -ForegroundColor Red
}
exit 1
