# Автоматизированный стенд Backend + реальный процесс NEXUS AI Runtime.
#
# Это проверка на ОДНОЙ машине: отдельный nexus-ai-runtime.exe подключается к
# реальному Axum backend по WebSocket, но PolyStrata заменена локальным
# OpenAI-compatible stub. Результат нельзя выдавать за проверку второго
# компьютера или реальной PolyStrata.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$runtimeExe = Join-Path $root 'runtime\target\debug\nexus-ai-runtime.exe'

Write-Host '==> Сборка Runtime'
cargo build --manifest-path (Join-Path $root 'runtime\Cargo.toml')
if (-not (Test-Path $runtimeExe)) {
    throw "Runtime executable не найден: $runtimeExe"
}

$env:NEXUS_RUNTIME_BIN = $runtimeExe
Write-Host '==> Запуск automated runtime stand'
cargo test --manifest-path (Join-Path $root 'backend\Cargo.toml') `
    --test ai_runtime_stand -- --ignored --nocapture

Write-Host ''
Write-Host 'Стенд завершён. Это проверка одной машины; для Production нужен'
Write-Host 'реальный второй компьютер + реальная PolyStrata (scripts/ai-two-machine-check.ps1).'
