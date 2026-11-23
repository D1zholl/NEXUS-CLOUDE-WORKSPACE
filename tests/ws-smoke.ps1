# Проверка WebSocket-канала: подключение по cookie-сессии и получение событий.
# Использование: powershell -File tests\ws-smoke.ps1
param(
    [string]$User = $(if ($env:SMOKE_USER) { $env:SMOKE_USER } else { 'smokeadmin' }),
    [string]$Password = $(if ($env:SMOKE_PASSWORD) { $env:SMOKE_PASSWORD } else { 'TestPass123!' })
)

$ErrorActionPreference = 'Stop'
$base = 'http://127.0.0.1:8080/api/v1'

# 1. Вход и получение cookie.
$loginParams = @{
    Uri = "$base/auth/login"
    Method = 'Post'
    ContentType = 'application/json'
    Body = (@{ username = $User; password = $Password } | ConvertTo-Json)
    SessionVariable = 'session'
    UseBasicParsing = $true
}
[void](Invoke-WebRequest @loginParams)
$cookie = ($session.Cookies.GetCookies('http://127.0.0.1:8080') | Where-Object { $_.Name -eq 'nexus_session' }).Value
Write-Host "login ok, cookie length: $($cookie.Length)"

# 2. Открываем WebSocket через ClientWebSocket (тип доступен без Add-Type).
$client = [System.Net.WebSockets.ClientWebSocket]::new()
$client.Options.SetRequestHeader('Cookie', "nexus_session=$cookie")
$uri = [Uri]'ws://127.0.0.1:8080/api/v1/ws'

$cts = [System.Threading.CancellationTokenSource]::new()
$cts.CancelAfter(15000)
$client.ConnectAsync($uri, $cts.Token).GetAwaiter().GetResult()
Write-Host "ws connected, state: $($client.State)"

# 3. Читаем первое сообщение (hello).
$buffer = New-Object byte[] 65536
$segment = [System.ArraySegment[byte]]::new($buffer)
$receiveCts = [System.Threading.CancellationTokenSource]::new()
$receiveCts.CancelAfter(10000)
$result = $client.ReceiveAsync($segment, $receiveCts.Token).GetAwaiter().GetResult()
$text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count)
Write-Host "first frame: $text"

$failed = 0
if ($text -notmatch '"kind":"connected"') { Write-Host "FAIL: no connected frame" -ForegroundColor Red; $failed++ }
else { Write-Host "  OK   hello frame" -ForegroundColor Green }

# 4. Триггерим событие: запускаем задачу обслуживания.
$me = Invoke-RestMethod -Uri "$base/auth/me" -WebSession $session
$csrf = $me.data.csrfToken
$job = Invoke-RestMethod -Uri "$base/jobs/maintenance" -Method Post -ContentType 'application/json' `
    -Body '{"kind":"metrics_prune"}' -Headers @{ 'X-CSRF-Token' = $csrf } -WebSession $session
Write-Host "job started: $($job.data.id)"

# 5. Ждём событие progress.
$sawProgress = $false
$deadline = (Get-Date).AddSeconds(12)
while ((Get-Date) -lt $deadline) {
    $receiveCts2 = [System.Threading.CancellationTokenSource]::new()
    $receiveCts2.CancelAfter(3000)
    try {
        $buffer2 = New-Object byte[] 65536
        $segment2 = [System.ArraySegment[byte]]::new($buffer2)
        $frame = $client.ReceiveAsync($segment2, $receiveCts2.Token).GetAwaiter().GetResult()
        $message = [System.Text.Encoding]::UTF8.GetString($buffer2, 0, $frame.Count)
        Write-Host "event: $message"
        if ($message -match 'job.progress' -or $message -match 'system.metrics.updated') {
            $sawProgress = $true
            break
        }
    } catch {
        continue
    }
}

if ($sawProgress) { Write-Host "  OK   realtime event received" -ForegroundColor Green }
else { Write-Host "  FAIL no realtime event" -ForegroundColor Red; $failed++ }

# 6. Проверяем, что без сессии соединение отклоняется.
$anon = [System.Net.WebSockets.ClientWebSocket]::new()
try {
    $anon.ConnectAsync($uri, [System.Threading.CancellationToken]::None).GetAwaiter().GetResult()
    Write-Host "  FAIL anonymous connection accepted" -ForegroundColor Red
    $failed++
} catch {
    Write-Host "  OK   anonymous connection rejected" -ForegroundColor Green
    $anon.Dispose()
}

$client.Dispose()
if ($failed -eq 0) { Write-Host "ws smoke passed" -ForegroundColor Green; exit 0 }
Write-Host "ws smoke failed: $failed" -ForegroundColor Red
exit 1
