# NEXUS WORKSPACE - общие функции резервного копирования бинарных объектов.
#
# Подключается из backup-objects.ps1 и restore-objects.ps1:
#   . (Join-Path $PSScriptRoot 'objects-common.ps1')
#
# Здесь собрано то, что одинаково нужно обеим сторонам:
# * длинные пути (> 260 символов) через префикс \\?\;
# * потоковый SHA-256 (IncrementalHash), без чтения файла целиком в память;
# * копирование с повторами при sharing violation;
# * VSS-снимок тома через Win32_ShadowCopy и временную символическую ссылку
#   (robocopy не умеет читать \\?\GLOBALROOT напрямую);
# * запросы к storage_volumes/storage_objects через psql и PGPASSFILE;
# * выдача прав службе через icacls.

$script:NexusObjectAreas = @('objects', 'staging', 'quarantine')

# \\?\C:\очень\длинный\путь. UNC-пути в хранилище запрещены политикой NEXUS,
# но для полноты они тоже приводятся к extended-форме.
function ConvertTo-NexusLongPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ($Path.StartsWith('\\?\')) { return $Path }
    $full = [System.IO.Path]::GetFullPath($Path)
    if ($full.StartsWith('\\')) {
        return '\\?\UNC\' + $full.TrimStart('\')
    }
    return '\\?\' + $full
}

function Get-NexusComparablePath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    if ($full.StartsWith('\\?\UNC\')) { return '\\' + $full.Substring(8) }
    if ($full.StartsWith('\\?\')) { return $full.Substring(4) }
    return $full
}

# Путь относительно каталога, с прямыми слэшами (формат манифеста).
function Get-NexusRelativePath {
    param(
        [Parameter(Mandatory = $true)][string]$Base,
        [Parameter(Mandatory = $true)][string]$Full
    )
    $basePath = (Get-NexusComparablePath $Base).TrimEnd('\', '/')
    $fullPath = Get-NexusComparablePath $Full
    if ($fullPath.Equals($basePath, [System.StringComparison]::OrdinalIgnoreCase)) { return '' }
    $prefix = $basePath + '\'
    if (-not $fullPath.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Путь $Full находится вне $Base."
    }
    return $fullPath.Substring($prefix.Length).Replace('\', '/')
}

# Потоковый SHA-256. FileShare.ReadWrite позволяет считать файл, который ещё
# открыт на чтение (например, staging во время загрузки).
function Get-NexusFileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $longPath = ConvertTo-NexusLongPath $Path
    $stream = $null
    $hasher = $null
    try {
        $stream = [System.IO.File]::Open(
            $longPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite)
        $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
            [System.Security.Cryptography.HashAlgorithmName]::SHA256)
        $buffer = New-Object byte[] (1024 * 1024)
        while ($true) {
            $read = $stream.Read($buffer, 0, $buffer.Length)
            if ($read -le 0) { break }
            $hasher.AppendData($buffer, 0, $read)
        }
        $hash = $hasher.GetHashAndReset()
        return (-join ($hash | ForEach-Object { $_.ToString('x2') }))
    } finally {
        if ($hasher) { $hasher.Dispose() }
        if ($stream) { $stream.Dispose() }
    }
}

# Копирование файла с длинными путями и повторами при блокировке.
# Атрибуты и время изменения сохраняются; ACL не копируются (права выдаёт
# restore-objects.ps1 через icacls).
function Copy-NexusFile {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [int]$Retries = 3,
        [int]$WaitSeconds = 5
    )
    $sourcePath = ConvertTo-NexusLongPath $Source
    $destinationPath = ConvertTo-NexusLongPath $Destination
    $parent = Split-Path -Parent $destinationPath
    if ($parent) { [void][System.IO.Directory]::CreateDirectory($parent) }

    $attempt = 0
    while ($true) {
        $attempt++
        $inputStream = $null
        $output = $null
        try {
            $inputStream = [System.IO.File]::Open(
                $sourcePath,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::ReadWrite)
            $output = [System.IO.File]::Open(
                $destinationPath,
                [System.IO.FileMode]::Create,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::None)
            $inputStream.CopyTo($output, 1024 * 1024)
            $output.Flush()
            $output.Dispose()
            $output = $null
            $stamp = [System.IO.File]::GetLastWriteTimeUtc($sourcePath)
            [System.IO.File]::SetLastWriteTimeUtc($destinationPath, $stamp)
            return
        } catch [System.IO.IOException] {
            if ($attempt -gt $Retries) { throw }
            Start-Sleep -Seconds $WaitSeconds
        } catch [System.UnauthorizedAccessException] {
            if ($attempt -gt $Retries) { throw }
            Start-Sleep -Seconds $WaitSeconds
        } finally {
            if ($output) { $output.Dispose() }
            if ($inputStream) { $inputStream.Dispose() }
        }
    }
}

# Корень тома для каталога: C:\NexusStorage -> C:\
function Get-NexusVolumeMountRoot {
    param([Parameter(Mandatory = $true)][string]$Root)
    $full = [System.IO.Path]::GetFullPath($Root)
    $mount = [System.IO.Path]::GetPathRoot($full)
    if ([string]::IsNullOrWhiteSpace($mount)) { throw "Не удалось определить том для $Root." }
    return $mount
}

# Обход каталога без перехода в junction/симлинки: аналог robocopy /XJ для
# incremental, где файлы копируются поштучно. PowerShell 5.1 Get-ChildItem
# -Recurse не гарантирует такое поведение, поэтому обходим вручную.
function Get-NexusVolumeFiles {
    param([Parameter(Mandatory = $true)][string]$VolumeRoot)
    $result = New-Object System.Collections.ArrayList
    foreach ($area in $script:NexusObjectAreas) {
        $areaRoot = Join-Path $VolumeRoot (Join-Path '.nexus' $area)
        if (-not (Test-Path -LiteralPath $areaRoot)) { continue }

        $stack = New-Object System.Collections.Stack
        $stack.Push((ConvertTo-NexusLongPath $areaRoot))
        while ($stack.Count -gt 0) {
            $directory = [string]$stack.Pop()
            $entries = [System.IO.Directory]::EnumerateFileSystemEntries($directory)
            foreach ($entry in $entries) {
                $attributes = [System.IO.File]::GetAttributes($entry)
                if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    continue
                }
                if (($attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                    $stack.Push($entry)
                    continue
                }
                $info = New-Object System.IO.FileInfo($entry)
                $relative = Get-NexusRelativePath -Base $areaRoot -Full $entry
                [void]$result.Add([pscustomobject]@{
                    Area         = $area
                    RelativePath = ('.nexus/' + $area + '/' + $relative)
                    FullName     = $entry
                    SizeBytes    = [int64]$info.Length
                    LastWriteUtc = $info.LastWriteTimeUtc
                })
            }
        }
    }
    return @($result.ToArray() | Sort-Object RelativePath)
}

# Вызов robocopy с обязательными для NEXUS ключами:
# /XJ - не заходить в junction и симлинки;
# /R:3 /W:5 - три повтора с паузой 5 с (открытые файлы, Defender);
# /MT - многопоточное копирование; /COPY:DAT - данные, атрибуты, время.
function Invoke-NexusRobocopy {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [int]$Threads = 8
    )
    if (-not (Test-Path -LiteralPath $Destination)) {
        New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    }
    $logDir = Split-Path -Parent $LogPath
    if ($logDir -and -not (Test-Path -LiteralPath $logDir)) {
        New-Item -ItemType Directory -Force -Path $logDir | Out-Null
    }
    $arguments = @(
        $Source,
        $Destination,
        '/E', '/XJ',
        '/R:3', '/W:5',
        ('/MT:' + $Threads),
        '/COPY:DAT', '/DCOPY:DAT',
        '/NP', '/NFL', '/NDL', '/NJH', '/NJS',
        ('/UNILOG+:' + $LogPath)
    )
    & robocopy @arguments | Out-Null
    $code = $LASTEXITCODE
    return [pscustomobject]@{ Code = $code; LogPath = $LogPath }
}

# psql с PGPASSFILE из nexus-common.ps1. SQL передаётся только аргументом.
function Invoke-NexusPsql {
    param(
        [Parameter(Mandatory = $true)]$Database,
        [Parameter(Mandatory = $true)][string]$Sql,
        [string]$PgBin = ''
    )
    $psql = Join-Path (Resolve-NexusPgBin -PgBin $PgBin) 'psql.exe'
    $arguments = @(
        '--host', $Database.Host,
        '--port', $Database.Port,
        '--username', $Database.User,
        '--dbname', $Database.Database,
        '--no-psqlrc',
        '--set', 'ON_ERROR_STOP=on',
        '--tuples-only',
        '--no-align',
        '--field-separator', "`t",
        '--command', $Sql
    )
    $output = & $psql @arguments 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        $text = (($output | ForEach-Object { "$_" }) -join ' ').Trim()
        throw "psql завершился с ошибкой ($code): $text"
    }
    return @($output | ForEach-Object { "$_" } | Where-Object { $_.Trim() -ne '' })
}

# Зарегистрированные тома. По умолчанию берутся только включённые.
function Get-NexusObjectVolumeRecords {
    param(
        [Parameter(Mandatory = $true)]$Database,
        [string]$PgBin = '',
        [switch]$IncludeDisabled
    )
    $where = if ($IncludeDisabled) { '' } else { 'where enabled = true ' }
    $sql = "select id::text, name, root_path, enabled from storage_volumes $where order by priority desc, name asc;"
    $rows = Invoke-NexusPsql -Database $Database -Sql $sql -PgBin $PgBin
    $volumes = @()
    foreach ($line in $rows) {
        $parts = $line -split "`t"
        if ($parts.Count -lt 4) { continue }
        $volumes += [pscustomobject]@{
            Id      = $parts[0]
            Name    = $parts[1]
            RootPath = $parts[2]
            Enabled = ($parts[3] -eq 't')
        }
    }
    return $volumes
}

# Объекты storage_objects для указанных томов. Возвращает строки как есть;
# сверка выполняется в скриптах, чтобы не смешивать SQL и политику.
function Get-NexusStorageObjectRecords {
    param(
        [Parameter(Mandatory = $true)]$Database,
        [Parameter(Mandatory = $true)][string[]]$VolumeIds,
        [string]$PgBin = ''
    )
    if (-not $VolumeIds -or $VolumeIds.Count -eq 0) { return @() }
    $quoted = @()
    foreach ($id in $VolumeIds) {
        if ($id -notmatch '^[0-9a-fA-F-]{36}$') { continue }
        $quoted += ("'" + $id + "'::uuid")
    }
    if ($quoted.Count -eq 0) { return @() }
    $sql = @"
select v.id::text, v.root_path, o.object_key, o.size_bytes::text, o.sha256, o.state
from storage_objects o
join storage_volumes v on v.id = o.volume_id
where v.id in ($($quoted -join ', '))
order by v.id, o.object_key;
"@
    $rows = Invoke-NexusPsql -Database $Database -Sql $sql -PgBin $PgBin
    $objects = @()
    foreach ($line in $rows) {
        $parts = $line -split "`t"
        if ($parts.Count -lt 6) { continue }
        $objects += [pscustomobject]@{
            VolumeId  = $parts[0]
            RootPath  = $parts[1]
            ObjectKey = $parts[2]
            SizeBytes = [int64]$parts[3]
            Sha256    = $parts[4]
            State     = $parts[5]
        }
    }
    return $objects
}

# VSS-снимок тома. Требует администратора и NTFS.
#
# robocopy не открывает \\?\GLOBALROOT\Device\... напрямую (ошибка 53),
# поэтому создаём временную символическую ссылку и копируем через неё.
# Ссылка и снимок удаляются в finally обоих скриптов.
function New-NexusVssSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$VolumeRoot,
        [string]$ProbeRelative = ''
    )
    Assert-NexusAdmin
    $volume = $VolumeRoot
    if (-not $volume.EndsWith('\')) { $volume += '\' }

    $created = Invoke-CimMethod -ClassName Win32_ShadowCopy -MethodName Create `
        -Arguments @{ Volume = $volume; Context = 'ClientAccessible' } -ErrorAction Stop
    if ($null -eq $created -or [int]$created.ReturnValue -ne 0) {
        $code = if ($created) { [int]$created.ReturnValue } else { -1 }
        throw "Win32_ShadowCopy.Create вернул $code."
    }
    $shadowId = [string]$created.ShadowID
    $shadow = Get-CimInstance -ClassName Win32_ShadowCopy -ErrorAction Stop |
        Where-Object { $_.ID -eq $shadowId } | Select-Object -First 1
    if (-not $shadow) { throw "Снимок VSS $shadowId создан, но не найден в WMI." }

    $link = Join-Path ([System.IO.Path]::GetTempPath()) ('nexus-objects-vss-' + [guid]::NewGuid().ToString('N'))
    & cmd.exe /d /c "mklink /d `"$link`" `"$($shadow.DeviceObject)`"" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Remove-NexusVssSnapshot -Snapshot ([pscustomobject]@{ Id = $shadowId; LinkPath = $link })
        throw "Не удалось создать символическую ссылку на снимок VSS."
    }

    $snapshot = [pscustomobject]@{
        Id           = $shadowId
        DeviceObject = [string]$shadow.DeviceObject
        LinkPath     = $link
        VolumeRoot   = $volume
    }
    if ($ProbeRelative) {
        $probe = Join-Path $link $ProbeRelative
        if (-not (Test-Path -LiteralPath $probe)) {
            Remove-NexusVssSnapshot -Snapshot $snapshot
            throw "Снимок VSS создан, но $probe в нём не виден."
        }
    }
    return $snapshot
}

function Remove-NexusVssSnapshot {
    param($Snapshot)
    if (-not $Snapshot) { return }
    if ($Snapshot.LinkPath -and (Test-Path -LiteralPath $Snapshot.LinkPath)) {
        Remove-Item -LiteralPath $Snapshot.LinkPath -Force -Recurse -ErrorAction SilentlyContinue
    }
    if (-not $Snapshot.Id) { return }
    try {
        $shadow = Get-CimInstance -ClassName Win32_ShadowCopy -ErrorAction SilentlyContinue |
            Where-Object { $_.ID -eq $Snapshot.Id } | Select-Object -First 1
        if ($shadow) { Remove-CimInstance -InputObject $shadow -ErrorAction Stop }
    } catch {
        # Фолбэк для Windows PowerShell: у WMI-объекта есть метод Delete().
        try {
            $wmi = Get-WmiObject -Class Win32_ShadowCopy -ErrorAction SilentlyContinue |
                Where-Object { $_.ID -eq $Snapshot.Id } | Select-Object -First 1
            if ($wmi) { [void]$wmi.Delete() }
        } catch {
            Write-Warning "Не удалось удалить снимок VSS $($Snapshot.Id): $($_.Exception.Message)"
        }
    }
}

# Путь внутри снимка VSS для каталога тома или относительного пути в нём.
function Get-NexusVssSourcePath {
    param(
        [Parameter(Mandatory = $true)]$Snapshot,
        [Parameter(Mandatory = $true)][string]$VolumeRoot,
        [string]$RelativePath = ''
    )
    $mount = Get-NexusVolumeMountRoot $VolumeRoot
    $relativeFromMount = $VolumeRoot.Substring($mount.Length).TrimStart('\')
    $path = $Snapshot.LinkPath
    if ($relativeFromMount) { $path = Join-Path $path $relativeFromMount }
    if ($RelativePath) { $path = Join-Path $path $RelativePath }
    return $path
}

function Get-NexusServiceAccount {
    param([string]$ServiceName = 'NexusWorkspace')
    if ($ServiceName) {
        $service = Get-CimInstance Win32_Service -Filter ("Name='" + ($ServiceName -replace "'", "''") + "'") -ErrorAction SilentlyContinue
        if ($service -and $service.StartName) { return [string]$service.StartName }
    }
    return 'NT AUTHORITY\SYSTEM'
}

# Права на восстановленный том: наследование включается, служба получает
# Modify на каталог и всё содержимое. Это и есть «восстановление прав» после
# копирования, которое намеренно не переносит ACL из резервной копии.
function Set-NexusObjectAcl {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Account,
        [switch]$Recurse
    )
    if (-not (Test-Path -LiteralPath $Path)) { throw "Каталог для выдачи прав не найден: $Path" }
    $grant = $Account + ':(OI)(CI)M'
    $arguments = @($Path, '/inheritance:e', '/grant:r', $grant, '/C', '/Q')
    if ($Recurse) { $arguments += '/T' }
    & icacls @arguments | Out-Null
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        throw "icacls завершился с кодом $code для $Path."
    }
}

function Get-NexusManifest {
    param([Parameter(Mandatory = $true)][string]$Path)
    $manifestPath = Join-Path $Path 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        throw "В $Path нет manifest.json."
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($manifest.format -ne 'nexus-objects/1') {
        throw "Неизвестный формат манифеста в ${manifestPath}: $($manifest.format)"
    }
    if ($manifest.status -and $manifest.status -ne 'complete') {
        throw "Набор ${Path} не завершён (status=$($manifest.status))."
    }
    return $manifest
}

function Write-NexusObjectManifest {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Manifest
    )
    $json = $Manifest | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText(
        (Join-Path $Path 'manifest.json'),
        $json + [Environment]::NewLine,
        (New-Object System.Text.UTF8Encoding($false)))
}

# SHA256SUMS для переноса на другой носитель: hash  relative/path.
function Write-NexusChecksumFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$ManifestName = 'manifest.json'
    )
    $lines = @()
    $files = Get-ChildItem -LiteralPath (ConvertTo-NexusLongPath $Path) -Recurse -File -Force |
        Where-Object { $_.Name -ne 'SHA256SUMS' } |
        Sort-Object FullName
    foreach ($file in $files) {
        $relative = Get-NexusRelativePath -Base $Path -Full $file.FullName
        $hash = Get-NexusFileSha256 $file.FullName
        $lines += ('{0}  {1}' -f $hash, $relative)
    }
    $lines | Set-Content -LiteralPath (Join-Path $Path 'SHA256SUMS') -Encoding ASCII
}
