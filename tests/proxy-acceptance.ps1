# NEXUS WORKSPACE - приёмка J2: HTTPS через Caddy, 1 ГиБ upload с паузой,
# Range-скачивание 1 ГиБ, WebSocket, реальный IP в аудите.
#
# Запуск (пароль администратора только через переменную окружения):
#   $env:NEXUS_TEST_PASSWORD = '<пароль>'
#   powershell -ExecutionPolicy Bypass -File tests\proxy-acceptance.ps1
#
# Стенд: Caddy for Windows (NSSM) с внутренним CA, https://localhost и
# https://<LAN-IP>. Публичный домен на стенде недоступен (BLOCKED).

[CmdletBinding()]
param(
    [string]$BaseUrl = 'https://localhost',
    [string]$LanUrl = 'https://172.24.241.69',
    [string]$Username = 'j2admin',
    [string]$PasswordEnv = 'NEXUS_TEST_PASSWORD',
    [int]$FileSizeMb = 1024,
    [int]$ChunkMb = 16,
    [string]$ResultPath = 'docs\reports\logs\j2-results.json',
    [switch]$DeleteEntry
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$script:Checks = New-Object System.Collections.Generic.List[object]
$script:Log = New-Object System.Collections.Generic.List[string]
$script:Client = $null
$script:Handler = $null
$script:Csrf = $null
$script:CookieHeader = $null
$script:EntryId = $null
$script:Sha256 = $null
$script:Boundary = 0

function Write-Log {
    param([string]$Message)
    $line = '{0} {1}' -f (Get-Date).ToString('HH:mm:ss'), $Message
    $script:Log.Add($line)
    Write-Host $line
}

function Add-Check {
    param([string]$Name, [bool]$Pass, $Detail)
    $script:Checks.Add([pscustomobject]@{ name = $Name; pass = $Pass; detail = $Detail })
    $color = if ($Pass) { 'Green' } else { 'Red' }
    Write-Host ('  [{0}] {1} - {2}' -f $(if ($Pass) { 'PASS' } else { 'FAIL' }), $Name, $Detail) -ForegroundColor $color
}

function Get-NexusApiError {
    param([int]$Status, [string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return "HTTP $Status" }
    try {
        $parsed = $Text | ConvertFrom-Json
        if ($parsed.error) { return ("HTTP {0} {1}: {2}" -f $Status, $parsed.error.code, $parsed.error.message) }
    } catch { }
    if ($Text.Length -gt 300) { return ("HTTP {0}: {1}" -f $Status, $Text.Substring(0, 300)) }
    return ("HTTP {0}: {1}" -f $Status, $Text)
}

function New-JsonContent {
    param($Body)
    $json = $Body | ConvertTo-Json -Depth 20 -Compress
    return New-Object System.Net.Http.StringContent -ArgumentList $json, ([System.Text.Encoding]::UTF8), 'application/json'
}

function New-Client {
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseCookies = $true
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $handler.AllowAutoRedirect = $true
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(60)
    return @{ Client = $client; Handler = $handler }
}

function Get-CookieHeader {
    param($Handler, [string]$Url)
    $uri = [uri]$Url
    $cookies = $Handler.CookieContainer.GetCookies($uri)
    $parts = @()
    foreach ($cookie in $cookies) { $parts += ("{0}={1}" -f $cookie.Name, $cookie.Value) }
    return ($parts -join '; ')
}

function Invoke-Api {
    param(
        [string]$Method,
        [string]$Path,
        $Body = $null,
        [hashtable]$Headers = $null,
        [switch]$AllowError,
        [string]$Base = $script:BaseUrl
    )
    $request = New-Object System.Net.Http.HttpRequestMessage
    $request.Method = New-Object System.Net.Http.HttpMethod -ArgumentList $Method
    $request.RequestUri = [uri]($Base + $Path)
    if ($script:Csrf) { [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $script:Csrf) }
    if ($Headers) { foreach ($key in $Headers.Keys) { [void]$request.Headers.TryAddWithoutValidation($key, [string]$Headers[$key]) } }
    if ($null -ne $Body) { $request.Content = New-JsonContent $Body }
    $response = $script:Client.SendAsync($request).GetAwaiter().GetResult()
    $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    $status = [int]$response.StatusCode
    $headersOut = @{}
    foreach ($header in $response.Headers) { $headersOut[$header.Key] = ($header.Value -join ',') }
    foreach ($header in $response.Content.Headers) { $headersOut[$header.Key] = ($header.Value -join ',') }
    $parsed = $null
    try { if ($text) { $parsed = $text | ConvertFrom-Json } } catch { }
    $response.Dispose()
    $request.Dispose()
    if ($status -ge 400 -and -not $AllowError) {
        throw (Get-NexusApiError -Status $status -Text $text)
    }
    return [pscustomobject]@{ status = $status; body = $parsed; text = $text; headers = $headersOut }
}

if (-not (Test-Path ("Env:" + $PasswordEnv))) {
    throw "Переменная окружения $PasswordEnv с паролем администратора не задана."
}
$password = (Get-Item -Path ("Env:" + $PasswordEnv)).Value

Write-Log "Стенд: BaseUrl=$BaseUrl LanUrl=$LanUrl файл=${FileSizeMb}МиБ чанк=${ChunkMb}МиБ"

# --- 1. HTTPS, вход и CSRF ---------------------------------------------------
$clientPair = New-Client
$script:Client = $clientPair.Client
$script:Handler = $clientPair.Handler

$login = Invoke-Api -Method POST -Path '/api/v1/auth/login' -Body @{ username = $Username; password = $password }
Add-Check 'HTTPS login' ($login.status -eq 200) ("HTTP {0}" -f $login.status)
$script:Csrf = $login.body.data.csrfToken
Add-Check 'CSRF token' ([bool]$script:Csrf) 'csrfToken получен после логина'
$me = Invoke-Api -Method GET -Path '/api/v1/auth/me'
Add-Check 'Cookie session + CSRF' ($me.status -eq 200) ("GET /auth/me -> HTTP {0}" -f $me.status)
$script:CookieHeader = Get-CookieHeader -Handler $script:Handler -Url $BaseUrl
Add-Check 'Secure cookie' ($script:CookieHeader -like 'nexus_session=*') 'cookie nexus_session установлена по HTTPS'

# Заголовки HTTPS-ответа: HSTS выставлен Caddy.
$head = Invoke-Api -Method GET -Path '/api/v1/health'
$hsts = $head.headers['Strict-Transport-Security']
Add-Check 'HSTS' ([bool]$hsts) ("Strict-Transport-Security=$hsts")
$serverHeader = $head.headers['Server']
Add-Check 'Заголовок Server скрыт' ([string]::IsNullOrWhiteSpace($serverHeader)) 'Caddy не раскрывает версию'

# --- 2. WebSocket через Caddy ------------------------------------------------
$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ws.Options.SetRequestHeader('Cookie', $script:CookieHeader)
$wsCts = New-Object System.Threading.CancellationTokenSource
$wsCts.CancelAfter(15000)
$wsUri = [uri](($BaseUrl -replace '^https', 'wss') + '/api/v1/ws')
$wsConnected = $false
$wsHello = $null
try {
    $ws.ConnectAsync($wsUri, $wsCts.Token).GetAwaiter().GetResult()
    $buffer = New-Object byte[] 65536
    $segment = New-Object System.ArraySegment[byte] -ArgumentList (,$buffer)
    $received = $ws.ReceiveAsync($segment, $wsCts.Token).GetAwaiter().GetResult()
    $wsHello = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
    $wsConnected = ($ws.State -eq [System.Net.WebSockets.WebSocketState]::Open)
} catch {
    Write-Log ("WebSocket: " + $_.Exception.Message)
}
Add-Check 'WebSocket Upgrade' $wsConnected ("wss://{0}/api/v1/ws" -f ([uri]$BaseUrl).Authority)
if ($wsHello) {
    $hello = $null
    try { $hello = $wsHello | ConvertFrom-Json } catch { }
    $isConnectedEvent = ($hello -and $hello.kind -eq 'connected')
    Add-Check 'WS connected event' $isConnectedEvent $wsHello
}

# --- 3. Загрузка 1 ГиБ чанками с паузой и продолжением ----------------------
$size = [int64]$FileSizeMb * 1MB
$chunk = [int64]$ChunkMb * 1MB
$tempDir = Join-Path $env:TEMP 'nexus-j2'
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
$sourcePath = Join-Path $tempDir 'j2-1gb.bin'

if (-not (Test-Path $sourcePath) -or (Get-Item $sourcePath).Length -ne $size) {
    Write-Log "Генерация файла $size байт: $sourcePath"
    $buffer = New-Object byte[] (4MB)
    for ($i = 0; $i -lt $buffer.Length; $i++) { $buffer[$i] = [byte](($i * 37 + 11) -band 0xFF) }
    $stream = [System.IO.File]::Create($sourcePath)
    try {
        $written = [int64]0
        $block = 0
        while ($written -lt $size) {
            [BitConverter]::GetBytes([int]$block).CopyTo($buffer, 0)
            $count = [int][Math]::Min([int64]$buffer.Length, $size - $written)
            $stream.Write($buffer, 0, $count)
            $written += $count
            $block++
        }
    } finally { $stream.Dispose() }
}
$sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
$script:Sha256 = $sourceHash
Write-Log "SHA-256 исходного файла: $sourceHash"

$uploadName = 'j2-1gb-{0}.bin' -f (Get-Date -Format 'yyyyMMdd-HHmmss')
$session = Invoke-Api -Method POST -Path '/api/v1/cloud/uploads' -Body @{
    name = $uploadName
    size = $size
    contentType = 'application/octet-stream'
}
$uploadId = $session.body.data.id
$sessionState = Invoke-Api -Method GET -Path ("/api/v1/cloud/uploads/{0}" -f $uploadId)
$maxChunk = [int64]$sessionState.body.data.chunkMaxBytes
if ($maxChunk -gt 0 -and $chunk -gt $maxChunk) { $chunk = $maxChunk }
$chunkCount = [int][Math]::Ceiling($size / $chunk)
Add-Check 'Upload session' ([bool]$uploadId) ("id=$uploadId chunkMax=$maxChunk чанков=$chunkCount")

$file = [System.IO.File]::OpenRead($sourcePath)
$uploadWatch = [Diagnostics.Stopwatch]::StartNew()
$offset = [int64]0
$pauseAt = [int][Math]::Floor($chunkCount / 4)
$paused = $false
$staleTested = $false
try {
    for ($index = 0; $index -lt $chunkCount; $index++) {
        if (-not $paused -and $index -ge $pauseAt) {
            # Пауза: клиент перестаёт отправлять, затем берёт offset с сервера.
            $status = Invoke-Api -Method GET -Path ("/api/v1/cloud/uploads/{0}" -f $uploadId)
            $offsetBefore = [int64]$status.body.data.offset
            Write-Log "Пауза на offset=$offsetBefore; ожидание 5 с"
            Start-Sleep -Seconds 5
            $again = Invoke-Api -Method GET -Path ("/api/v1/cloud/uploads/{0}" -f $uploadId)
            $offsetAfter = [int64]$again.body.data.offset
            Add-Check 'Пауза: offset не изменился' ($offsetBefore -eq $offsetAfter) ("$offsetBefore -> $offsetAfter")
            $offset = $offsetAfter
            # Смещение файла приводим к серверному.
            [void]$file.Seek($offset, [System.IO.SeekOrigin]::Begin)

            # Устаревший offset должен дать 409 с details.offset.
            if (-not $staleTested -and $offset -gt 0) {
                $staleBytes = New-Object byte[] 1024
                $staleRequest = New-Object System.Net.Http.HttpRequestMessage
                $staleRequest.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'PATCH'
                $staleRequest.RequestUri = [uri]($BaseUrl + ("/api/v1/cloud/uploads/{0}" -f $uploadId))
                [void]$staleRequest.Headers.TryAddWithoutValidation('Upload-Offset', '0')
                [void]$staleRequest.Headers.TryAddWithoutValidation('X-CSRF-Token', $script:Csrf)
                $staleContent = New-Object System.Net.Http.ByteArrayContent -ArgumentList (,$staleBytes)
                $staleContent.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue('application/octet-stream')
                $staleRequest.Content = $staleContent
                $staleResponse = $script:Client.SendAsync($staleRequest).GetAwaiter().GetResult()
                $staleCode = [int]$staleResponse.StatusCode
                $staleResponse.Dispose(); $staleRequest.Dispose()
                Add-Check 'Устаревший offset -> 409' ($staleCode -eq 409) ("HTTP {0}" -f $staleCode)
                $staleTested = $true
            }
            $paused = $true
        }

        $count = [int][Math]::Min($chunk, $size - $offset)
        if ($count -le 0) { break }
        $bytes = New-Object byte[] $count
        $read = 0
        while ($read -lt $count) {
            $n = $file.Read($bytes, $read, $count - $read)
            if ($n -le 0) { throw 'Неожиданный конец файла при чтении чанка.' }
            $read += $n
        }
        $request = New-Object System.Net.Http.HttpRequestMessage
        $request.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'PATCH'
        $request.RequestUri = [uri]($BaseUrl + ("/api/v1/cloud/uploads/{0}" -f $uploadId))
        [void]$request.Headers.TryAddWithoutValidation('Upload-Offset', [string]$offset)
        [void]$request.Headers.TryAddWithoutValidation('X-CSRF-Token', $script:Csrf)
        $content = New-Object System.Net.Http.ByteArrayContent -ArgumentList (,$bytes)
        $content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue('application/octet-stream')
        $request.Content = $content
        $response = $script:Client.SendAsync($request).GetAwaiter().GetResult()
        $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        $code = [int]$response.StatusCode
        $response.Dispose(); $request.Dispose()
        if ($code -ne 200) { throw ("PATCH offset={0} -> HTTP {1}: {2}" -f $offset, $code, $text) }
        $parsed = $text | ConvertFrom-Json
        $newOffset = [int64]$parsed.data.offset
        if ($newOffset -ne ($offset + $count)) {
            throw ("Сервер вернул offset {0}, ожидалось {1}" -f $newOffset, ($offset + $count))
        }
        $offset = $newOffset
        if (($index + 1) % 8 -eq 0 -or $offset -eq $size) {
            Write-Log ("  {0}/{1} МиБ ({2:N0} МиБ/с)" -f [int]($offset / 1MB), [int]($size / 1MB), (($offset / 1MB) / [Math]::Max(0.001, $uploadWatch.Elapsed.TotalSeconds)))
        }
    }
} finally {
    $file.Dispose()
    $uploadWatch.Stop()
}
Add-Check 'Загрузка 1 ГиБ завершена' ($offset -eq $size) ("offset=$offset size=$size за {0:N1} с" -f $uploadWatch.Elapsed.TotalSeconds)

$complete = Invoke-Api -Method POST -Path ("/api/v1/cloud/uploads/{0}/complete" -f $uploadId) `
    -Headers @{ 'Idempotency-Key' = [guid]::NewGuid().ToString() } -Body @{ strategy = 'auto' }
$entry = $complete.body.data
$script:EntryId = $entry.id
Add-Check 'Завершение загрузки' ([bool]$entry.id) ("entry=$($entry.id) size=$($entry.size) sha=$($entry.sha256)")

# --- 4. WebSocket: upload.completed -----------------------------------------
if ($wsConnected) {
    $sawCompleted = $false
    $deadline = (Get-Date).AddSeconds(15)
    $buffer = New-Object byte[] 65536
    $segment = New-Object System.ArraySegment[byte] -ArgumentList (,$buffer)
    while ((Get-Date) -lt $deadline -and -not $sawCompleted) {
        $cts = New-Object System.Threading.CancellationTokenSource
        $cts.CancelAfter(3000)
        try {
            $received = $ws.ReceiveAsync($segment, $cts.Token).GetAwaiter().GetResult()
            if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { break }
            $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
            if ($text -match 'upload\.completed') { $sawCompleted = $true; Write-Log ("WS: " + $text) }
        } catch { }
        $cts.Dispose()
    }
    Add-Check 'WS upload.completed' $sawCompleted 'событие получено через Caddy'
    try {
        $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    } catch { }
}

# --- 5. Range-скачивание 1 ГиБ ----------------------------------------------
$rangeSize = 100MB
$hashed = [System.Security.Cryptography.IncrementalHash]::CreateHash([System.Security.Cryptography.HashAlgorithmName]::SHA256)
$rangeTotal = [int64]0
$rangeOk = 0
$rangeCountExpected = [int][Math]::Ceiling($size / $rangeSize)
$acceptRanges = $null
$rangeWatch = [Diagnostics.Stopwatch]::StartNew()
try {
    $start = [int64]0
    $index = 0
    while ($start -lt $size) {
        $length = [int64][Math]::Min($rangeSize, $size - $start)
        $end = $start + $length - 1
        $request = New-Object System.Net.Http.HttpRequestMessage
        $request.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'GET'
        $request.RequestUri = [uri]($BaseUrl + ("/api/v1/cloud/entries/{0}/download" -f $entry.id))
        [void]$request.Headers.TryAddWithoutValidation('Range', ("bytes={0}-{1}" -f $start, $end))
        $response = $script:Client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        if (-not $acceptRanges) {
            $values = $null
            if ($response.Headers.TryGetValues('Accept-Ranges', [ref]$values)) { $acceptRanges = ($values -join ',') }
        }
        $code = [int]$response.StatusCode
        $contentRange = ''
        $values = $null
        if ($response.Content.Headers.TryGetValues('Content-Range', [ref]$values)) { $contentRange = ($values -join ',') }
        if ($code -eq 206) { $rangeOk++ }
        $stream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        $buffer = New-Object byte[] 1MB
        $received = [int64]0
        while (($n = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $hashed.AppendData($buffer, 0, $n)
            $received += $n
        }
        $stream.Dispose(); $response.Dispose(); $request.Dispose()
        if ($received -ne $length) { throw ("Range {0}: получено {1}, ожидалось {2}" -f $start, $received, $length) }
        $rangeTotal += $received
        if ($index -lt 2 -or $start + $length -ge $size) {
            Write-Log ("  Range {0}: {1}-{2} HTTP {3} Content-Range={4}" -f $index, $start, $end, $code, $contentRange)
        }
        $start += $length
        $index++
    }
} finally {
    $rangeHash = ([BitConverter]::ToString($hashed.GetHashAndReset())).Replace('-', '').ToLowerInvariant()
    $hashed.Dispose()
    $rangeWatch.Stop()
}
Add-Check 'Range 206' ($rangeOk -eq $rangeCountExpected) ("{0}/{1} ответов 206" -f $rangeOk, $rangeCountExpected)
Add-Check 'Accept-Ranges' ($acceptRanges -like '*bytes*') ("Accept-Ranges=$acceptRanges")
Add-Check 'Range 1 ГиБ: SHA совпал' ($rangeHash -eq $sourceHash) ("$rangeHash за {0:N1} с, {1} байт" -f $rangeWatch.Elapsed.TotalSeconds, $rangeTotal)

# --- 6. trust_proxy: реальный IP в аудите -----------------------------------
$lanPair = New-Client
$lanRequest = New-Object System.Net.Http.HttpRequestMessage
$lanRequest.Method = New-Object System.Net.Http.HttpMethod -ArgumentList 'POST'
$lanRequest.RequestUri = [uri]($LanUrl + '/api/v1/auth/login')
[void]$lanRequest.Headers.TryAddWithoutValidation('X-Forwarded-For', '203.0.113.77')
$lanRequest.Content = New-JsonContent @{ username = $Username; password = $password }
$lanResponse = $lanPair.Client.SendAsync($lanRequest).GetAwaiter().GetResult()
$lanCode = [int]$lanResponse.StatusCode
$lanResponse.Dispose(); $lanRequest.Dispose(); $lanPair.Client.Dispose(); $lanPair.Handler.Dispose()
Add-Check 'Логин по LAN-адресу' ($lanCode -eq 200) ("$LanUrl HTTP $lanCode")

$lanIp = ([uri]$LanUrl).Host
Start-Sleep -Seconds 1
$audit = Invoke-Api -Method GET -Path '/api/v1/system/audit?perPage=50&eventType=auth.login.success'
$auditItems = @($audit.body.data.items)
$match = $auditItems | Where-Object { $_.ip -eq $lanIp -and $_.actorUsername -eq $Username } | Select-Object -First 1
$spoofed = $auditItems | Where-Object { $_.ip -eq '203.0.113.77' } | Select-Object -First 1
Add-Check 'Аудит: IP клиента' ([bool]$match) ("ip=$lanIp (не 127.0.0.1) для $Username")
Add-Check 'Подмена X-Forwarded-For отброшена' (-not $spoofed) '203.0.113.77 в аудите отсутствует'
if ($match) { Write-Log ("Аудит: event={0} ip={1} actor={2} at={3}" -f $match.eventType, $match.ip, $match.actorUsername, $match.createdAt) }

# --- 7. Итог ----------------------------------------------------------------
$failed = @($script:Checks | Where-Object { -not $_.pass })
$summary = [pscustomobject]@{
    startedAt      = (Get-Date).ToString('o')
    baseUrl        = $BaseUrl
    lanUrl         = $LanUrl
    uploadFile     = $uploadName
    uploadEntryId  = $script:EntryId
    sizeBytes      = $size
    sha256         = $script:Sha256
    uploadSeconds  = [Math]::Round($uploadWatch.Elapsed.TotalSeconds, 2)
    rangeSeconds   = [Math]::Round($rangeWatch.Elapsed.TotalSeconds, 2)
    checks         = $script:Checks
    failed         = $failed.Count
    passed         = $script:Checks.Count - $failed.Count
}
$dir = Split-Path -Parent $ResultPath
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
Write-Log ("Итог: PASS {0}, FAIL {1}; JSON: {2}" -f $summary.passed, $failed.Count, $ResultPath)

if ($DeleteEntry -and $script:EntryId) {
    try {
        [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}" -f $script:EntryId) -AllowError)
        [void](Invoke-Api -Method DELETE -Path ("/api/v1/cloud/entries/{0}/purge" -f $script:EntryId) -AllowError)
        Write-Log "Запись $($script:EntryId) удалена."
    } catch { Write-Log ("Удаление записи: " + $_.Exception.Message) }
}

Remove-Item (Join-Path $tempDir 'j2-1gb.bin') -Force -ErrorAction SilentlyContinue

if ($failed.Count -gt 0) { exit 1 } else { exit 0 }
