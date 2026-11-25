#requires -Version 5.1
<#
.SYNOPSIS
  NEXUS Agent Mode — two-machine integration check (run on the AI PC / PC-2).

.DESCRIPTION
  PC-1 hosts NEXUS Backend + PostgreSQL + Frontend + HTTPS (Caddy).
  PC-2 hosts NEXUS AI Runtime + PolyStrata + the local model.

  This script runs on PC-2, talks to PC-1 over HTTPS and uses the real
  `nexus-ai-runtime.exe` and the real PolyStrata. It automates everything that
  is deterministic (health, TLS, enrollment, capabilities, workspace allowlist,
  task lifecycle, events, cancel, service restart/reconnect) and reports
  WARN for steps whose outcome depends on the model choosing a tool.

  It never substitutes a mock model. It does not modify the production DB
  schema; it creates a dedicated provider/workspace with an explicit name.

.EXAMPLE
  .\scripts\ai-two-machine-verify.ps1 `
    -BackendUrl https://nexus.example `
    -AdminUser admin `
    -AdminPasswordEnv NEXUS_ADMIN_PASSWORD `
    -ProviderUrl http://127.0.0.1:8080/v1 `
    -Model qwen2.5-coder `
    -RuntimeServiceName NexusAiRuntime
#>
param(
    [Parameter(Mandatory = $true)][string] $BackendUrl,
    [Parameter(Mandatory = $true)][string] $AdminUser,
    [string] $AdminPasswordEnv = 'NEXUS_ADMIN_PASSWORD',
    [string] $AdminPassword = '',
    [string] $RuntimeNodeName = $env:COMPUTERNAME,
    [string] $RuntimeExe = '',
    [string] $RuntimeConfig = 'C:\NexusAI\runtime.toml',
    [string] $RuntimeServiceName = 'NexusAiRuntime',
    [string] $EnrollmentCode = '',
    [string] $WorkspaceRoot = 'C:\NexusAI\workspaces\agent-check',
    [string] $WorkspaceName = 'Two-machine Agent Check',
    [string] $ProviderName = 'Two-machine PolyStrata',
    [string] $ProviderUrl = 'http://127.0.0.1:8080/v1',
    [string] $ProviderApiKey = '',
    [string] $Model = 'default',
    [string] $SkillSlug = '',
    [int] $TaskTimeoutSeconds = 240,
    [switch] $AllowInsecureHttp,
    [switch] $SkipRestart,
    [switch] $KeepArtifacts
)

$ErrorActionPreference = 'Stop'
$script:Failures = 0
$script:Warnings = 0
$script:Passed = 0
$runTag = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = Split-Path -Parent $PSScriptRoot

function Step([string] $name) {
    Write-Host ''
    Write-Host "==> $name" -ForegroundColor Cyan
}
function Pass([string] $message) {
    $script:Passed++
    Write-Host "PASS: $message" -ForegroundColor Green
}
function Warn([string] $message) {
    $script:Warnings++
    Write-Host "WARN: $message" -ForegroundColor Yellow
}
function Fail([string] $message) {
    $script:Failures++
    Write-Host "FAIL: $message" -ForegroundColor Red
}
function Require([bool] $condition, [string] $message) {
    if ($condition) { Pass $message } else { Fail $message }
}

$BackendUrl = $BackendUrl.TrimEnd('/')
if (-not $RuntimeExe) {
    $RuntimeExe = Join-Path $root 'runtime\target\release\nexus-ai-runtime.exe'
}
if (-not (Test-Path $RuntimeExe)) {
    Fail "Runtime executable not found: $RuntimeExe"
    exit 2
}

if (-not $AllowInsecureHttp -and $BackendUrl -notmatch '^https://') {
    Fail 'BackendUrl must use https:// for the two-machine check (or pass -AllowInsecureHttp).'
    exit 2
}

# ---------------------------------------------------------------------------
# 1. Transport, TLS and backend health
# ---------------------------------------------------------------------------
Step 'PC-1 transport and NEXUS health'
try {
    $healthResponse = Invoke-WebRequest -Uri "$BackendUrl/api/v1/health" -TimeoutSec 20 -UseBasicParsing
    Require ($healthResponse.StatusCode -eq 200) "GET /api/v1/health -> HTTP $($healthResponse.StatusCode)"
    if ($BackendUrl -match '^https://') {
        Pass 'TLS handshake with PC-1 succeeded (HTTPS)'
        try {
            $cert = [System.Net.Http.HttpClient]::new().GetAsync($BackendUrl).GetAwaiter().GetResult()
            if ($cert.IsSuccessStatusCode) { Pass "TLS endpoint reachable: $($cert.StatusCode)" }
        } catch {
            Warn "TLS certificate introspection unavailable: $($_.Exception.Message)"
        }
    } else {
        Warn 'HTTPS disabled: only acceptable for an isolated test network.'
    }
} catch {
    Fail "NEXUS health is unreachable: $($_.Exception.Message)"
    exit 1
}

# ---------------------------------------------------------------------------
# 2. Real PolyStrata
# ---------------------------------------------------------------------------
Step 'PolyStrata on PC-2'
$polyBase = $ProviderUrl -replace '/v1/?$', ''
try {
    $models = Invoke-RestMethod -Uri "$ProviderUrl/models" -TimeoutSec 20
    $count = @($models.data).Count
    Require ($count -gt 0) "PolyStrata /models returned $count model(s)"
} catch {
    Fail "PolyStrata /models failed: $($_.Exception.Message)"
}
foreach ($probe in @("$polyBase/health", "$polyBase/status")) {
    try {
        $response = Invoke-WebRequest -Uri $probe -TimeoutSec 15 -UseBasicParsing
        Pass "$probe -> HTTP $($response.StatusCode)"
    } catch {
        Warn "$probe -> $($_.Exception.Message)"
    }
}

# ---------------------------------------------------------------------------
# 3. Runtime configuration and capabilities
# ---------------------------------------------------------------------------
Step 'Runtime configuration'
if (-not (Test-Path $RuntimeConfig)) {
    Fail "Runtime config not found: $RuntimeConfig"
    exit 2
}
$checkJson = & $RuntimeExe --config $RuntimeConfig check --json 2>&1
if ($LASTEXITCODE -ne 0) {
    Fail "runtime check failed with exit code $LASTEXITCODE"
    Write-Host ($checkJson -join "`n")
} else {
    Pass 'runtime check --json succeeded'
    try {
        $parsed = ($checkJson -join "`n") | ConvertFrom-Json
        if ($parsed.protocolVersion) { Pass "protocolVersion = $($parsed.protocolVersion)" }
        if ($parsed.PSObject.Properties.Name -contains 'capabilities') {
            Pass 'capabilities are reported by runtime check'
        }
    } catch {
        Warn 'runtime check output is not valid JSON; inspect manually.'
    }
}

# ---------------------------------------------------------------------------
# 4. Admin session (cookie + CSRF)
# ---------------------------------------------------------------------------
Step 'Admin session over HTTPS'
if (-not $AdminPassword) {
    $AdminPassword = [Environment]::GetEnvironmentVariable($AdminPasswordEnv)
}
if (-not $AdminPassword) {
    Fail "Admin password is not set (env $AdminPasswordEnv or -AdminPassword)."
    exit 2
}
$session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
try {
    $login = Invoke-RestMethod -Method Post -Uri "$BackendUrl/api/v1/auth/login" `
        -WebSession $session -ContentType 'application/json' `
        -Body (@{ username = $AdminUser; password = $AdminPassword } | ConvertTo-Json)
    $csrf = $login.data.csrfToken
    Require ([bool]$csrf) 'login succeeded, CSRF token received'
} catch {
    Fail "login failed: $($_.Exception.Message)"
    exit 1
}

function Api([string] $method, [string] $path, $body = $null) {
    $params = @{
        Method     = $method
        Uri        = "$BackendUrl/api/v1$path"
        WebSession = $session
        Headers    = @{ 'X-CSRF-Token' = $csrf; Accept = 'application/json' }
        TimeoutSec = 60
    }
    if ($null -ne $body) {
        $params.ContentType = 'application/json'
        $params.Body = ($body | ConvertTo-Json -Depth 10)
    }
    $response = Invoke-RestMethod @params
    if ($response.success -eq $false) {
        throw "API $method $path failed: $($response.error.message)"
    }
    return $response.data
}

$me = Api 'GET' '/auth/me'
Pass "authenticated as $($me.username) ($($me.id))"

# ---------------------------------------------------------------------------
# 5. Enrollment / Runtime connection
# ---------------------------------------------------------------------------
Step 'Runtime enrollment and WSS connection'
function Get-RuntimeNode {
    $page = Api 'GET' '/ai/admin/runtime-nodes'
    return @($page.items | Where-Object { $_.name -eq $RuntimeNodeName })[0]
}
$node = Get-RuntimeNode
if (-not $node) {
    if (-not $EnrollmentCode) {
        Fail "Runtime '$RuntimeNodeName' is not enrolled. Create an enrollment code in AI Management and pass -EnrollmentCode."
        exit 1
    }
    & $RuntimeExe --config $RuntimeConfig enroll --code $EnrollmentCode --name $RuntimeNodeName
    if ($LASTEXITCODE -ne 0) { Fail 'enrollment failed'; exit 1 }
    Pass 'enrollment completed; device token stored in state/device.json'
}
if ($SkipRestart -eq $false) {
    try {
        $service = Get-Service -Name $RuntimeServiceName -ErrorAction Stop
        if ($service.Status -ne 'Running') { Start-Service -Name $RuntimeServiceName }
        Pass "service $RuntimeServiceName is running"
    } catch {
        Warn "service $RuntimeServiceName not found; start the Runtime manually (runtime run)."
    }
}
$deadline = (Get-Date).AddSeconds(60)
do {
    Start-Sleep -Milliseconds 500
    $node = Get-RuntimeNode
} while ((-not $node -or $node.status -ne 'online') -and (Get-Date) -lt $deadline)
Require ($node -and $node.status -eq 'online') "Runtime '$RuntimeNodeName' is online"
if (-not $node) { exit 1 }

$capabilities = $node.capabilities
if ($capabilities.protocolVersion) { Pass "negotiated protocolVersion = $($capabilities.protocolVersion)" }
$toolIds = @($capabilities.tools | ForEach-Object { $_.toolId })
foreach ($required in @('fs.read', 'fs.write', 'fs.stat', 'fs.apply_patch', 'program.run')) {
    if ($toolIds -contains $required) { Pass "capability: $required" }
    else { Warn "capability missing on this Runtime: $required" }
}

# ---------------------------------------------------------------------------
# 6. Provider and workspace (PC-1 metadata, PC-2 path)
# ---------------------------------------------------------------------------
Step 'Provider and workspace'
$providers = Api 'GET' '/ai/admin/providers'
$provider = @($providers.items | Where-Object { $_.name -eq $ProviderName })[0]
if (-not $provider) {
    $provider = Api 'POST' '/ai/admin/providers' @{
        name             = $ProviderName
        baseUrl          = $ProviderUrl
        apiKey           = if ($ProviderApiKey) { $ProviderApiKey } else { $null }
        defaultModel     = $Model
        runtimeNodeId    = $node.id
        enabled          = $true
    }
    Pass "provider created: $($provider.id)"
} else {
    Pass "provider reused: $($provider.id)"
}

if (-not (Test-Path $WorkspaceRoot)) {
    New-Item -ItemType Directory -Path $WorkspaceRoot -Force | Out-Null
}
$workspaces = Api 'GET' '/ai/admin/workspaces'
$workspace = @($workspaces.items | Where-Object { $_.name -eq $WorkspaceName })[0]
if (-not $workspace) {
    $workspace = Api 'POST' '/ai/admin/workspaces' @{
        ownerId          = $me.id
        runtimeNodeId    = $node.id
        name             = $WorkspaceName
        rootPath         = (Resolve-Path $WorkspaceRoot).Path
        allowedPrograms  = @()
        maxParallelTasks = 1
        maxTaskSeconds   = 600
    }
    Pass "workspace created: $($workspace.id)"
} else {
    Pass "workspace reused: $($workspace.id)"
}

$missing = @($workspace.missingSafeTools)
if ($missing.Count -gt 0) {
    Warn "workspace lacks new safe tools: $($missing -join ', '); granting explicitly (admin action)."
    $workspace = Api 'POST' "/ai/admin/workspaces/$($workspace.id)/tools/grant-defaults" @{}
    $missing = @($workspace.missingSafeTools)
}
Require ($missing.Count -eq 0) 'workspace allowlist contains every safe tool (fs.stat, skills.list, ...)'
foreach ($required in @('fs.read', 'fs.write', 'fs.stat', 'fs.apply_patch', 'skills.list', 'skill.load')) {
    Require ($workspace.allowedTools -contains $required) "workspace allows $required"
}

# ---------------------------------------------------------------------------
# 7. Agent tasks
# ---------------------------------------------------------------------------
function New-AgentConversation([string[]] $skillIds = @()) {
    return Api 'POST' '/ai/conversations' @{
        mode        = 'agent'
        model       = $Model
        workspaceId = $workspace.id
        providerId  = $provider.id
        skillIds    = $skillIds
    }
}

function Send-AgentTask([string] $conversationId, [string] $content, [string[]] $skillIds = @()) {
    return Api 'POST' "/ai/conversations/$conversationId/messages" @{
        content  = $content
        skillIds = $skillIds
    }
}

function Get-Task([string] $taskId) {
    return Api 'GET' "/ai/tasks/$taskId"
}

function Wait-Task([string] $taskId, [string[]] $statuses, [int] $timeoutSeconds = $TaskTimeoutSeconds) {
    $until = (Get-Date).AddSeconds($timeoutSeconds)
    $last = $null
    while ((Get-Date) -lt $until) {
        $last = Get-Task $taskId
        if ($statuses -contains $last.status) { return $last }
        Start-Sleep -Milliseconds 500
    }
    return $last
}

$probeName = "probe-$runTag.txt"
$probeContent = "NEXUS-TWO-MACHINE-$runTag"
Set-Content -Path (Join-Path $WorkspaceRoot $probeName) -Value $probeContent -Encoding UTF8
Pass "probe file created on PC-2: $probeName"

Step 'Agent: read-only task, streaming and events'
$conversation = New-AgentConversation
$sent = Send-AgentTask $conversation.id "Прочитай файл $probeName инструментом fs_read и перескажи содержимое. Не изменяй файлы."
$task = Wait-Task $sent.task.id @('completed', 'failed', 'cancelled')
Require ($task.status -eq 'completed') "read task finished with status '$($task.status)'"
$events = Api 'GET' "/ai/tasks/$($sent.task.id)/events?perPage=200"
$eventIds = @($events.items | ForEach-Object { $_.id })
Require ($eventIds.Count -gt 0) "task events recorded: $($eventIds.Count)"
Require (($eventIds | Sort-Object -Unique).Count -eq $eventIds.Count) 'event ids are unique'
Require (($eventIds -join ',') -eq (($eventIds | Sort-Object) -join ',')) 'events are ordered ascending'
$executions = Api 'GET' "/ai/tasks/$($sent.task.id)/executions"
$readCalls = @($executions.items | Where-Object { $_.toolId -eq 'fs.read' })
if ($readCalls.Count -gt 0) {
    Pass "model called fs.read; result length $($readCalls[0].resultText.Length)"
    Require ($readCalls[0].resultText -like "*$probeContent*") 'fs.read returned the probe content'
} else {
    Warn 'model answered without fs.read; tool calling is not proven by this run (manual review).'
}

Step 'Agent: file write and diff'
$writeName = "written-$runTag.txt"
$writeConversation = New-AgentConversation
$writeSent = Send-AgentTask $writeConversation.id "Создай файл $writeName с текстом 'two-machine-ok' инструментом fs_write."
$writeTask = Wait-Task $writeSent.task.id @('completed', 'failed', 'cancelled')
Require ($writeTask.status -eq 'completed') "write task finished with status '$($writeTask.status)'"
$writtenPath = Join-Path $WorkspaceRoot $writeName
if (Test-Path $writtenPath) {
    Pass "file created by Runtime on PC-2: $writeName"
    $writtenExecutions = Api 'GET' "/ai/tasks/$($writeSent.task.id)/executions"
    $writeCall = @($writtenExecutions.items | Where-Object { $_.toolId -eq 'fs.write' })[0]
    if ($writeCall) { Pass "fs.write execution status: $($writeCall.status)" }
} else {
    Warn 'file was not created: the model may not have called fs_write (manual review).'
}

Step 'Agent: approval (reject then approve)'
$patchConversation = New-AgentConversation
$before = Get-Content -Path $writtenPath -Raw -ErrorAction SilentlyContinue
$patchSent = Send-AgentTask $patchConversation.id "Измени файл $writeName инструментом fs_apply_patch: добавь строку 'approved-line'."
$approval = $null
$until = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $until) {
    $pending = Api 'GET' '/ai/approvals/pending'
    $approval = @($pending.items | Where-Object { $_.taskId -eq $patchSent.task.id })[0]
    if ($approval) { break }
    Start-Sleep -Milliseconds 500
}
if (-not $approval) {
    Warn 'model did not request a risky tool; approval flow was not exercised (manual review).'
} else {
    Pass "approval requested for $($approval.toolId) risk $($approval.risk)"
    Api 'POST' "/ai/approvals/$($approval.id)/reject" @{ reason = 'two-machine reject check' } | Out-Null
    $rejected = Wait-Task $patchSent.task.id @('cancelled', 'failed')
    Require ($rejected.status -eq 'cancelled') "reject cancelled the task (status '$($rejected.status)')"
    $afterReject = Get-Content -Path $writtenPath -Raw -ErrorAction SilentlyContinue
    Require ($afterReject -eq $before) 'rejected patch did not modify the file'

    $secondConversation = New-AgentConversation
    $secondSent = Send-AgentTask $secondConversation.id "Измени файл $writeName инструментом fs_apply_patch: добавь строку 'approved-line'."
    $secondApproval = $null
    $until = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $until) {
        $pending = Api 'GET' '/ai/approvals/pending'
        $secondApproval = @($pending.items | Where-Object { $_.taskId -eq $secondSent.task.id })[0]
        if ($secondApproval) { break }
        Start-Sleep -Milliseconds 500
    }
    if ($secondApproval) {
        Api 'POST' "/ai/approvals/$($secondApproval.id)/approve" @{ reason = 'two-machine approve check' } | Out-Null
        $approved = Wait-Task $secondSent.task.id @('completed', 'failed', 'cancelled')
        if ($approved.status -eq 'completed') {
            Pass 'approved patch completed'
            $afterApprove = Get-Content -Path $writtenPath -Raw
            if ($afterApprove -match 'approved-line') { Pass 'approved patch is visible on PC-2 disk' }
            else { Fail 'approved patch did not change the file' }
        } else {
            Fail "approved task ended as '$($approved.status)'"
        }
    } else {
        Warn 'second approval was not requested (manual review).'
    }
}

Step 'Agent: cancel'
$cancelConversation = New-AgentConversation
$cancelSent = Send-AgentTask $cancelConversation.id 'Выполни длинный анализ: прочитай все файлы в workspace по одному и составь подробный отчёт.'
Start-Sleep -Seconds 2
Api 'POST' "/ai/tasks/$($cancelSent.task.id)/cancel" @{} | Out-Null
$cancelled = Wait-Task $cancelSent.task.id @('cancelled', 'failed') 120
if ($cancelled.status -eq 'cancelled' -and $cancelled.cancelStatus -eq 'confirmed') {
    Pass 'cancel confirmed by Runtime'
} elseif ($cancelled.status -eq 'cancelled') {
    Warn "cancel reached terminal state '$($cancelled.status)' but cancelStatus = '$($cancelled.cancelStatus)'"
} else {
    Fail "cancel did not reach terminal state: '$($cancelled.status)'"
}

# ---------------------------------------------------------------------------
# 8. Restart / reconnect / recovery
# ---------------------------------------------------------------------------
if ($SkipRestart) {
    Warn 'runtime restart check skipped (-SkipRestart)'
} else {
    Step 'Runtime service restart, reconnect and recovery'
    try {
        Stop-Service -Name $RuntimeServiceName -Force -ErrorAction Stop
        $offline = $false
        $until = (Get-Date).AddSeconds(40)
        while ((Get-Date) -lt $until) {
            Start-Sleep -Milliseconds 500
            $current = Get-RuntimeNode
            if ($current -and $current.status -ne 'online') { $offline = $true; break }
        }
        Require $offline 'backend observed Runtime disconnect'
        Start-Service -Name $RuntimeServiceName
        $online = $false
        $until = (Get-Date).AddSeconds(90)
        while ((Get-Date) -lt $until) {
            Start-Sleep -Milliseconds 500
            $current = Get-RuntimeNode
            if ($current -and $current.status -eq 'online') { $online = $true; break }
        }
        Require $online 'Runtime reconnected with the same device token (no re-enrollment)'

        $recoveryConversation = New-AgentConversation
        $recoverySent = Send-AgentTask $recoveryConversation.id "Прочитай файл $probeName инструментом fs_read."
        $recoveryTask = Wait-Task $recoverySent.task.id @('completed', 'failed', 'cancelled')
        Require ($recoveryTask.status -eq 'completed') 'task after restart completed'
    } catch {
        Fail "service restart failed: $($_.Exception.Message)"
    }
}

# ---------------------------------------------------------------------------
# 9. Long multi-step run
# ---------------------------------------------------------------------------
Step 'Agent: long multi-step run (several sequential tools)'
$multiConversation = New-AgentConversation
$multiSent = Send-AgentTask $multiConversation.id @"
Выполни последовательно:
1) прочитай $probeName инструментом fs_read;
2) создай файл summary-$runTag.txt инструментом fs_write с кратким итогом;
3) прочитай summary-$runTag.txt инструментом fs_read и подтверди результат.
Не выполняй эти шаги одним инструментом.
"@
$multiTask = Wait-Task $multiSent.task.id @('completed', 'failed', 'cancelled') 300
Require ($multiTask.status -eq 'completed') "long run finished with status '$($multiTask.status)'"
$multiExecutions = Api 'GET' "/ai/tasks/$($multiSent.task.id)/executions"
$toolCount = @($multiExecutions.items).Count
if ($toolCount -ge 2) { Pass "multi-step run executed $toolCount tool call(s)" }
else { Warn "multi-step run executed $toolCount tool call(s); model did not chain tools (manual review)." }
Require (@($multiExecutions.items | Where-Object { $_.status -eq 'failed' -and $_.errorCode -eq 'AI_EXECUTION_UNKNOWN' }).Count -eq 0) 'no unknown execution results in the long run'

# ---------------------------------------------------------------------------
# 10. Skills
# ---------------------------------------------------------------------------
Step 'Skills'
$skills = Api 'GET' "/ai/skills"
if ($SkillSlug) {
    $skill = @($skills.items | Where-Object { $_.slug -eq $SkillSlug })[0]
    if (-not $skill) {
        Warn "assigned Skill '$SkillSlug' is not visible to the admin; assign it to the user first."
    } else {
        Pass "Skill visible: $($skill.slug) v$($skill.version)"
        $skillConversation = New-AgentConversation @($skill.id)
        $skillSent = Send-AgentTask $skillConversation.id "Загрузи Skill $($skill.slug) инструментом skill_load и кратко перескажи его назначение." @($skill.id)
        $skillTask = Wait-Task $skillSent.task.id @('completed', 'failed', 'cancelled')
        Require ($skillTask.status -eq 'completed') 'task with selected Skill completed'
        $skillExecutions = Api 'GET' "/ai/tasks/$($skillSent.task.id)/executions"
        if (@($skillExecutions.items | Where-Object { $_.toolId -eq 'skill.load' }).Count -gt 0) {
            Pass 'skill.load was executed'
        } else {
            Warn 'model did not call skill.load (manual review).'
        }
    }
} else {
    Warn "skills: $($skills.items.Count) visible; pass -SkillSlug to exercise skill.load."
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '=== Two-machine check summary ===' -ForegroundColor Cyan
Write-Host "PASS: $script:Passed"
Write-Host "WARN: $script:Warnings"
Write-Host "FAIL: $script:Failures"
Write-Host "Workspace: $WorkspaceRoot"
Write-Host "Runtime node: $RuntimeNodeName ($($node.id))"

if (-not $KeepArtifacts) {
    # Probe files are safe to keep; only the generated marker file is removed.
    $cleanup = Join-Path $WorkspaceRoot "summary-$runTag.txt"
    if (Test-Path $cleanup) { Remove-Item $cleanup -Force }
}

if ($script:Failures -gt 0) { exit 1 }
exit 0
