# Smoke-проверка новых возможностей на запущенном сервере.
# Использование: powershell -File tests\smoke-new-features.ps1
# Требуется запущенный сервер и существующий администратор
# (SMOKE_USER / SMOKE_PASSWORD, по умолчанию smokeadmin / TestPass123!).
$ErrorActionPreference = 'Stop'
$base = 'http://127.0.0.1:8080/api/v1'
$user = if ($env:SMOKE_USER) { $env:SMOKE_USER } else { 'smokeadmin' }
$pass = if ($env:SMOKE_PASSWORD) { $env:SMOKE_PASSWORD } else { 'TestPass123!' }

$script:session = $null
$script:csrf = $null

function Call($method, $path, $body) {
    $headers = @{}
    if ($script:csrf) { $headers['X-CSRF-Token'] = $script:csrf }
    $params = @{
        Uri = "$base$path"
        Method = $method
        Headers = $headers
        WebSession = $script:session
        UseBasicParsing = $true
        TimeoutSec = 60
    }
    if ($null -ne $body) {
        $params['Body'] = ($body | ConvertTo-Json -Depth 8)
        $params['ContentType'] = 'application/json'
    }
    try {
        $response = Invoke-WebRequest @params
        $parsed = $null
        try { $parsed = $response.Content | ConvertFrom-Json } catch { }
        return @{ ok = $true; status = [int]$response.StatusCode; body = $parsed }
    } catch {
        $status = 0
        if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
        $payload = $null
        if ($_.ErrorDetails -and $_.ErrorDetails.Message) {
            try { $payload = $_.ErrorDetails.Message | ConvertFrom-Json } catch { }
        }
        return @{ ok = $false; status = $status; body = $payload }
    }
}

$script:failures = 0
function Expect($condition, $label) {
    if ($condition) { Write-Host ("  OK   " + $label) -ForegroundColor Green }
    else { Write-Host ("  FAIL " + $label) -ForegroundColor Red; $script:failures++ }
}

Write-Host "NEXUS smoke: new features"
Write-Host ""

# login
$loginParams = @{
    Uri = "$base/auth/login"
    Method = 'Post'
    ContentType = 'application/json'
    Body = (@{ username = $user; password = $pass } | ConvertTo-Json)
    SessionVariable = 'session'
    UseBasicParsing = $true
}
$login = Invoke-WebRequest @loginParams
$script:session = $session
$loginData = $login.Content | ConvertFrom-Json
$script:csrf = $loginData.data.csrfToken
Expect ($login.StatusCode -eq 200) "login"
Expect ($null -ne $script:csrf) "csrf token"

# OpenAPI
$spec = Call 'GET' '/openapi.json'
Expect ($spec.ok -and $spec.body.openapi -eq '3.1.0') "openapi 3.1"
$pathCount = ($spec.body.paths.PSObject.Properties | Measure-Object).Count
Expect ($pathCount -ge 90) ("paths: " + $pathCount)

$docs = Call 'GET' '/docs'
Expect ($docs.status -eq 404) "swagger disabled by default"

# settings catalog
$catalog = Call 'GET' '/system/settings/catalog'
Expect ($catalog.ok -and $catalog.body.data.Count -ge 10) ("setting keys: " + $catalog.body.data.Count)
$settings = Call 'GET' '/system/settings'
Expect ($settings.ok -and $null -ne $settings.body.data.catalog) "settings values/meta/catalog"

# settings validation
$bad = Call 'PUT' '/system/settings/unknown.key' 42
Expect ($bad.status -eq 422 -and $bad.body.error.code -eq 'UNKNOWN_SETTING') "unknown key -> 422 UNKNOWN_SETTING"
$good = Call 'PUT' '/system/settings/limits.max_upload_mb' 2048
Expect ($good.ok) "update max upload limit"
$restore = Call 'PUT' '/system/settings/limits.max_upload_mb' 10240
Expect ($restore.ok) "restore max upload limit"

# quota
$usage = Call 'GET' '/me/usage'
Expect ($usage.ok -and $usage.body.data.quotaBytes -gt 0) "quota + usage"

# module gate
$off = Call 'PUT' '/system/modules/cloud' @{ enabled = $false }
Expect ($off.ok) "disable cloud"
Start-Sleep -Milliseconds 700
$blocked = Call 'GET' '/cloud/entries'
Expect ($blocked.status -eq 403 -and $blocked.body.error.code -eq 'MODULE_DISABLED') "cloud blocked 403 MODULE_DISABLED"
$systemOk = Call 'GET' '/system/overview'
Expect ($systemOk.ok) "system works while module disabled"
$minecraftOk = Call 'GET' '/minecraft/stats'
Expect ($minecraftOk.ok) "other module unaffected"
$on = Call 'PUT' '/system/modules/cloud' @{ enabled = $true }
Start-Sleep -Milliseconds 400
$restored = Call 'GET' '/cloud/entries'
Expect ($on.ok -and $restored.ok) "cloud re-enabled without restart"

# monitoring
$current = Call 'GET' '/monitoring/current'
Expect ($current.ok) "current metrics"
Expect ($null -ne $current.body.data.app.activeDownloads) "metric activeDownloads"
Expect ($null -ne $current.body.data.app.activeWebSockets) "metric activeWebSockets"
Expect ($null -ne $current.body.data.network.activeConnections) "metric activeConnections"
$modules = Call 'GET' '/monitoring/modules'
Expect ($modules.ok -and $null -ne $modules.body.data.connections) "connections block in modules"

# cloud views
$entries = Call 'GET' '/cloud/entries'
Expect ($entries.ok) "cloud list"
$viewAll = Call 'GET' '/cloud/entries?view=all'
Expect ($viewAll.ok) "view=all"

# minecraft summary
$stats = Call 'GET' '/minecraft/stats'
Expect ($stats.ok) "build stats"
Expect ($null -ne $stats.body.data.recentUpdated) "recentUpdated present"
Expect ($null -ne $stats.body.data.recentPublished) "recentPublished present"

Write-Host ""
if ($script:failures -eq 0) { Write-Host "smoke passed" -ForegroundColor Green; exit 0 }
Write-Host ("failed checks: " + $script:failures) -ForegroundColor Red
exit 1
