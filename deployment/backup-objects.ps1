# NEXUS WORKSPACE - резервное копирование бинарных объектов (Windows).
#
# Копирует .nexus\objects, .nexus\staging и .nexus\quarantine каждого
# зарегистрированного тома в отдельный набор с манифестом SHA-256. Сверяет
# объекты со storage_objects и умеет строить цепочку incremental-наборов.
#
# Согласованность (открытые файлы Windows):
#   Auto  - VSS-снимок; если он недоступен, служба останавливается на время
#           копирования (согласованная пауза записи);
#   Vss   - только VSS, без остановки службы;
#   Pause - только остановка службы;
#   Live  - копирование на живую с повторами robocopy (без гарантии
#           согласованности; для стенда и диагностики).
#
# Примеры:
#   # Инкрементальный набор всех включённых томов, хранение 7 наборов:
#   powershell -ExecutionPolicy Bypass -File deployment\backup-objects.ps1 `
#       -BackupDir D:\NexusObjectBackups -Retain 7 -Prune -Force
#
#   # Только один том, полный набор:
#   ... -VolumeRoot C:\NexusStorage -Mode Full
#
#   # Ежедневное расписание в 03:30 (от SYSTEM, с правами администратора):
#   ... -RegisterTask -Schedule Daily -At 03:30
#
# Удаление старых наборов выполняется только при -Prune и подтверждении
# (-Force для неинтерактивного запуска).

[CmdletBinding()]
param(
    [string]$BackupDir = 'C:\NexusObjectBackups',
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$PgBin = '',
    [ValidateSet('Auto', 'Vss', 'Pause', 'Live')][string]$Consistency = 'Auto',
    [ValidateSet('Full', 'Incremental')][string]$Mode = 'Incremental',
    [string[]]$VolumeRoot = @(),
    [string]$BaseBackup = '',
    [int]$Retain = 7,
    [int]$FullEveryDays = 7,
    [switch]$Prune,
    [switch]$Force,
    [switch]$SkipDatabaseCheck,
    [switch]$StrictDatabase,
    [switch]$SkipStaging,
    [switch]$SkipQuarantine,
    [int]$Threads = 8,
    [string]$Label = '',
    [string]$Stamp = '',
    [string]$ResultPath = '',
    [ValidateSet('None', 'Daily', 'Weekly')][string]$Schedule = 'None',
    [string]$At = '03:30',
    [string]$TaskName = 'NexusObjectsBackup',
    [switch]$RegisterTask,
    [switch]$UnregisterTask
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptDir 'nexus-common.ps1')
. (Join-Path $scriptDir 'objects-common.ps1')

function Register-NexusObjectBackupTask {
    param(
        [string]$Name,
        [string]$Schedule,
        [string]$At,
        [string]$BackupDir,
        [string]$InstallDir,
        [string]$Mode,
        [string]$Consistency,
        [int]$Retain
    )
    Assert-NexusAdmin
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $scriptPath = Join-Path $scriptDir 'backup-objects.ps1'
    $arguments = @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', ('"' + $scriptPath + '"'),
        '-BackupDir', ('"' + $BackupDir + '"'),
        '-InstallDir', ('"' + $InstallDir + '"'),
        '-Mode', $Mode,
        '-Consistency', $Consistency,
        '-Retain', $Retain,
        '-Prune', '-Force'
    ) -join ' '

    $action = New-ScheduledTaskAction -Execute $powershell -Argument $arguments
    $trigger = if ($Schedule -eq 'Weekly') {
        New-ScheduledTaskTrigger -Weekly -DaysOfWeek Sunday -At $At
    } else {
        New-ScheduledTaskTrigger -Daily -At $At
    }
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -MultipleInstances IgnoreNew `
        -ExecutionTimeLimit (New-TimeSpan -Hours 6) `
        -StartWhenAvailable
    $description = 'NEXUS: резервное копирование бинарных объектов (.nexus objects/staging/quarantine)'
    Register-ScheduledTask -TaskName $Name -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings -Description $description -Force | Out-Null
    $task = Get-ScheduledTask -TaskName $Name
    Write-Host ("    задача {0}: {1}, следующее выполнение {2}" -f `
        $task.TaskName, $task.State, (($task.Triggers | Select-Object -First 1).StartBoundary))
}

if ($UnregisterTask) {
    Assert-NexusAdmin
    $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($existing) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-NexusStep "Задача $TaskName удалена"
    } else {
        Write-Host "Задача $TaskName не зарегистрирована." -ForegroundColor Yellow
    }
    return
}

if ($RegisterTask) {
    if ($Schedule -eq 'None') { throw 'Для -RegisterTask укажите -Schedule Daily или Weekly.' }
    if ($At -notmatch '^\d{2}:\d{2}$') { throw 'Время -At задаётся как ЧЧ:ММ.' }
    Register-NexusObjectBackupTask -Name $TaskName -Schedule $Schedule -At $At `
        -BackupDir $BackupDir -InstallDir $InstallDir -Mode $Mode `
        -Consistency $Consistency -Retain $Retain
    Write-Host 'Задача зарегистрирована. Первый набор запустите вручную или дождитесь расписания.'
    return
}

if ($Retain -lt 1) { throw 'Retain должен быть не меньше 1.' }
if ($Threads -lt 1 -or $Threads -gt 128) { throw 'Threads должен быть от 1 до 128.' }
if (-not $Stamp) { $Stamp = Get-Date -Format 'yyyyMMdd-HHmmss' }

$backupRoot = [System.IO.Path]::GetFullPath($BackupDir)
$setDir = Join-Path $backupRoot ('objects-' + $Stamp)
$setSuffix = 1
while (Test-Path -LiteralPath $setDir) {
    $setDir = Join-Path $backupRoot ('objects-' + $Stamp + '-' + $setSuffix)
    $setSuffix++
}

$result = [ordered]@{
    startedAt        = (Get-Date).ToString('o')
    backupPath       = $setDir
    backupDir        = $backupRoot
    installDir       = $InstallDir
    mode             = $Mode
    consistency      = $Consistency
    label            = $Label
    service          = $ServiceName
    serviceStopped   = $false
    pass             = $false
}

$serviceStopped = $false
$service = $null
$snapshots = @{}
$pgPassFile = $null
$db = $null
$volumes = @()
$previous = @{}
$currentStage = 'init'

try {
    if ($Consistency -ne 'Live') { Assert-NexusAdmin }

    if (-not $SkipDatabaseCheck) {
        $envPath = Join-Path $InstallDir '.env'
        $config = Get-NexusEnvConfig -Path $envPath
        if (-not $config['DATABASE_URL']) { throw "В .env нет DATABASE_URL." }
        $db = ConvertFrom-NexusDatabaseUrl -DatabaseUrl $config['DATABASE_URL']
        $pgPassFile = New-NexusPgPassFile -Database $db
        $result.database = $db.Database
    }

    Write-NexusStep "Подготовка резервной копии объектов в $setDir"
    New-Item -ItemType Directory -Force -Path $setDir | Out-Null

    # 1. Тома: из БД или явно указанные каталоги.
    if ($VolumeRoot -and $VolumeRoot.Count -gt 0) {
        foreach ($root in $VolumeRoot) {
            $fullRoot = [System.IO.Path]::GetFullPath($root).TrimEnd('\')
            if (-not (Test-Path -LiteralPath $fullRoot)) {
                Write-Host "    каталог не найден, пропускаю: $fullRoot" -ForegroundColor Yellow
                continue
            }
            $id = $null
            $name = Split-Path -Leaf $fullRoot
            if ($db) {
                $registered = Get-NexusObjectVolumeRecords -Database $db -PgBin $PgBin -IncludeDisabled |
                    Where-Object { [System.IO.Path]::GetFullPath($_.RootPath).TrimEnd('\') -ieq $fullRoot } |
                    Select-Object -First 1
                if ($registered) { $id = $registered.Id; $name = $registered.Name }
            }
            if (-not $id) {
                $digest = [System.Security.Cryptography.SHA256]::Create()
                $bytes = [System.Text.Encoding]::UTF8.GetBytes($fullRoot.ToLowerInvariant())
                $hash = $digest.ComputeHash($bytes)
                $id = 'local-' + (($hash[0..7] | ForEach-Object { $_.ToString('x2') }) -join '')
                Write-Host "    $fullRoot не зарегистрирован в БД - копирую как $id" -ForegroundColor Yellow
            }
            $volumes += [pscustomobject]@{ Id = $id; Name = $name; RootPath = $fullRoot }
        }
    } else {
        if (-not $db) {
            throw 'Без БД укажите -VolumeRoot. Иначе неоткуда взять список томов.'
        }
        $volumes = @(Get-NexusObjectVolumeRecords -Database $db -PgBin $PgBin)
    }

    if (-not $volumes -or $volumes.Count -eq 0) { throw 'Нет томов для резервного копирования.' }

    # Недоступный (отключённый) том не должен срывать копирование остальных:
    # фиксируем его в результате и продолжаем.
    $availableVolumes = @()
    $skippedVolumes = @()
    foreach ($volume in $volumes) {
        if (-not (Test-Path -LiteralPath $volume.RootPath)) {
            Write-Host "    том $($volume.Name) недоступен, пропускаю: $($volume.RootPath)" -ForegroundColor Yellow
            $skippedVolumes += [ordered]@{ id = $volume.Id; name = $volume.Name; rootPath = $volume.RootPath }
            continue
        }
        $setVolumeDir = Join-Path (Join-Path $setDir 'volumes') $volume.Id
        if ([System.IO.Path]::GetFullPath($setVolumeDir).StartsWith(
                [System.IO.Path]::GetFullPath($volume.RootPath).TrimEnd('\') + '\',
                [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Каталог резервной копии $setDir не должен находиться внутри тома $($volume.RootPath)."
        }
        $availableVolumes += $volume
    }
    if ($availableVolumes.Count -eq 0) {
        throw 'Все тома недоступны: копировать нечего.'
    }
    $volumes = $availableVolumes
    $result.volumes = @($volumes | ForEach-Object { $_.Name })
    $result.skippedVolumes = $skippedVolumes

    # 2. Индекс предыдущих наборов для incremental. Берём только текущую
    # цепочку: последний Full и все наборы после него. Иначе файл из старого
    # полного набора мог бы быть пропущен, хотя в актуальной цепочке его нет.
    $baseRef = ''
    $copyMode = $Mode
    if ($Mode -eq 'Incremental') {
        $manifests = @()
        if ($BaseBackup) {
            $manifests = @(Get-NexusManifest -Path ([System.IO.Path]::GetFullPath($BaseBackup)))
        } else {
            $candidateDirs = Get-ChildItem -LiteralPath $backupRoot -Directory -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like 'objects-*' -and $_.FullName -ne $setDir } |
                Sort-Object Name
            foreach ($candidate in $candidateDirs) {
                $manifestPath = Join-Path $candidate.FullName 'manifest.json'
                if (-not (Test-Path -LiteralPath $manifestPath)) { continue }
                try {
                    $candidateManifest = Get-NexusManifest -Path $candidate.FullName
                    if ($candidateManifest.status -eq 'complete') { $manifests += $candidateManifest }
                } catch {
                    Write-Host "    пропускаю повреждённый манифест $manifestPath" -ForegroundColor Yellow
                }
            }
        }
        if (-not $BaseBackup -and $manifests.Count -gt 1) {
            $lastFull = -1
            for ($i = 0; $i -lt $manifests.Count; $i++) {
                if ($manifests[$i].mode -eq 'Full') { $lastFull = $i }
            }
            if ($lastFull -gt 0) {
                $manifests = @($manifests[$lastFull..($manifests.Count - 1)])
            }
        }
        foreach ($manifest in $manifests) {
            foreach ($manifestVolume in $manifest.volumes) {
                foreach ($file in $manifestVolume.files) {
                    $key = ([string]$manifestVolume.id + '|' + [string]$file.path).ToLowerInvariant()
                    $previous[$key] = $file
                }
            }
        }
        if ($manifests.Count -eq 0) {
            $copyMode = 'Full'
            Write-Host '    предыдущих наборов нет - выполняю полный набор' -ForegroundColor Yellow
        } elseif (-not $BaseBackup -and $FullEveryDays -gt 0) {
            $latestFullManifest = $manifests | Where-Object { $_.mode -eq 'Full' } |
                Sort-Object { $_.createdAt } | Select-Object -Last 1
            if (-not $latestFullManifest) {
                $copyMode = 'Full'
                Write-Host '    в цепочке нет полного набора - выполняю полный набор' -ForegroundColor Yellow
            } else {
                $ageDays = ((Get-Date) - (Get-Date $latestFullManifest.createdAt)).TotalDays
                if ($ageDays -ge $FullEveryDays) {
                    $copyMode = 'Full'
                    Write-Host ("    полному набору {0:N1} дн. - начинаю новую цепочку" -f $ageDays) -ForegroundColor Yellow
                }
            }
        }
        if ($manifests.Count -gt 0) {
            $latestManifest = $manifests | Sort-Object { $_.createdAt } | Select-Object -Last 1
            $baseRef = if ($latestManifest.backupPath) { [string]$latestManifest.backupPath } else { [string]$latestManifest.createdAt }
        }
        if ($copyMode -eq 'Full') { $baseRef = '' }
        $result.previousSets = @($manifests | ForEach-Object { $_.createdAt })
    }

    # 3. Согласованность: VSS или пауза записи.
    $service = Get-NexusServiceObject -Name $ServiceName
    $serviceRunning = ($service -and $service.Status -eq 'Running')
    $consistencyUsed = 'live'
    $needPause = $false

    if ($Consistency -eq 'Vss' -or ($Consistency -eq 'Auto' -and $serviceRunning)) {
        try {
            $mounts = @()
            foreach ($volume in $volumes) {
                $mount = Get-NexusVolumeMountRoot $volume.RootPath
                if ($mounts -notcontains $mount) { $mounts += $mount }
            }
            foreach ($mount in $mounts) {
                $probeRoot = ($volumes | Where-Object { (Get-NexusVolumeMountRoot $_.RootPath) -eq $mount } |
                    Select-Object -First 1).RootPath
                $probe = $probeRoot.Substring($mount.Length).TrimStart('\')
                Write-Host "    VSS-снимок $mount"
                $snapshots[$mount] = New-NexusVssSnapshot -VolumeRoot $mount -ProbeRelative $probe
            }
            $consistencyUsed = 'vss'
        } catch {
            $message = $_.Exception.Message
            foreach ($snapshot in $snapshots.Values) { Remove-NexusVssSnapshot -Snapshot $snapshot }
            $snapshots = @{}
            if ($Consistency -eq 'Vss') { throw "VSS недоступен: $message" }
            Write-Host "    VSS недоступен: $message" -ForegroundColor Yellow
            Write-Host "    перехожу к согласованной паузе записи (остановке службы)" -ForegroundColor Yellow
            $needPause = $true
        }
    } elseif ($Consistency -eq 'Pause') {
        $needPause = $true
    } elseif ($Consistency -eq 'Live') {
        Write-Host "    ВНИМАНИЕ: Live-копирование не гарантирует согласованность; открытые файлы могут быть пропущены." -ForegroundColor Yellow
    }

    if ($needPause) {
        if (-not $serviceRunning) {
            Write-Host "    служба $ServiceName не запущена - пауза не нужна"
        } else {
            Write-NexusStep "Остановка службы $ServiceName на время копирования"
            $serviceStopped = Stop-NexusService -Name $ServiceName -TimeoutSeconds 60
            $result.serviceStopped = $serviceStopped
            Write-Host "    служба остановлена"
        }
        $consistencyUsed = 'pause'
    }
    $result.consistencyUsed = $consistencyUsed

    # 4. Копирование и манифест по томам.
    $currentStage = 'copy'
    $manifestVolumes = @()
    $totalFiles = 0
    $totalBytes = [int64]0
    $dbSummary = [ordered]@{
        checked                = $false
        objects                = 0
        verified               = 0
        missingOnDisk          = 0
        hashMismatch           = 0
        untracked              = 0
        missingSamples         = @()
        mismatchSamples        = @()
    }
    $dbObjectsByVolume = @{}

    if (-not $SkipDatabaseCheck -and $db) {
        $ids = @($volumes | ForEach-Object { $_.Id } | Where-Object { $_ -notlike 'local-*' })
        if ($ids.Count -gt 0) {
            $dbObjects = Get-NexusStorageObjectRecords -Database $db -VolumeIds $ids -PgBin $PgBin
            foreach ($row in $dbObjects) {
                if (-not $dbObjectsByVolume.ContainsKey($row.VolumeId)) {
                    $dbObjectsByVolume[$row.VolumeId] = @()
                }
                $dbObjectsByVolume[$row.VolumeId] += $row
            }
            $dbSummary.checked = $true
        }
    }

    $includedAreas = @('objects')
    if (-not $SkipStaging) { $includedAreas += 'staging' }
    if (-not $SkipQuarantine) { $includedAreas += 'quarantine' }

    foreach ($volume in $volumes) {
        Write-NexusStep ("Том {0}: {1}" -f $volume.Name, $volume.RootPath)
        $mount = Get-NexusVolumeMountRoot $volume.RootPath
        $sourceBase = if ($snapshots.ContainsKey($mount)) {
            Get-NexusVssSourcePath -Snapshot $snapshots[$mount] -VolumeRoot $volume.RootPath
        } else {
            $volume.RootPath
        }
        $sourceFiles = Get-NexusVolumeFiles -VolumeRoot $sourceBase
        $sourceFiles = @($sourceFiles | Where-Object { $includedAreas -contains $_.Area })
        $sourceObjectIndex = @{}
        foreach ($file in $sourceFiles) {
            if ($file.Area -eq 'objects') {
                $key = $file.RelativePath.Substring('.nexus/objects/'.Length)
                $sourceObjectIndex[$key] = $file
            }
        }

        $setVolumeDir = Join-Path (Join-Path $setDir 'volumes') $volume.Id
        New-Item -ItemType Directory -Force -Path $setVolumeDir | Out-Null
        $copyLog = Join-Path (Join-Path $setDir 'logs') (($volume.Id) + '-robocopy.log')

        $manifestFiles = @()
        $copiedCount = 0
        $reusedCount = 0
        $copiedBytes = [int64]0

        if ($copyMode -eq 'Full') {
            foreach ($area in $includedAreas) {
                $sourceArea = Join-Path $sourceBase (Join-Path '.nexus' $area)
                if (-not (Test-Path -LiteralPath $sourceArea)) { continue }
                $destinationArea = Join-Path $setVolumeDir (Join-Path '.nexus' $area)
                $copy = Invoke-NexusRobocopy -Source $sourceArea -Destination $destinationArea `
                    -LogPath $copyLog -Threads $Threads
                if ($copy.Code -ge 8) { throw "robocopy $area завершился с кодом $($copy.Code). Журнал: $copyLog" }
            }
            $destinationFiles = @(Get-NexusVolumeFiles -VolumeRoot $setVolumeDir |
                Where-Object { $includedAreas -contains $_.Area })
            foreach ($file in $destinationFiles) {
                Write-Host ("    SHA-256 {0}" -f $file.RelativePath)
                $hash = Get-NexusFileSha256 $file.FullName
                $manifestFiles += [ordered]@{
                    path      = $file.RelativePath
                    sizeBytes = $file.SizeBytes
                    mtimeUtc  = $file.LastWriteUtc.ToUniversalTime().ToString('o')
                    sha256    = $hash
                }
                $copiedCount++
                $copiedBytes += $file.SizeBytes
            }
        } else {
            foreach ($file in $sourceFiles) {
                $key = ([string]$volume.Id + '|' + [string]$file.RelativePath).ToLowerInvariant()
                $known = $previous[$key]
                $unchanged = $known -and
                    ([int64]$known.sizeBytes -eq [int64]$file.SizeBytes) -and
                    ($null -ne $known.mtimeUtc) -and
                    ([Math]::Abs(((Get-Date $file.LastWriteUtc.ToUniversalTime()) -
                        (Get-Date $known.mtimeUtc).ToUniversalTime()).TotalSeconds) -le 2)
                if ($unchanged) {
                    $reusedCount++
                    continue
                }
                $destination = Join-Path $setVolumeDir $file.RelativePath
                Copy-NexusFile -Source $file.FullName -Destination $destination -Retries 3 -WaitSeconds 5
                $hash = Get-NexusFileSha256 $destination
                $manifestFiles += [ordered]@{
                    path      = $file.RelativePath
                    sizeBytes = $file.SizeBytes
                    mtimeUtc  = $file.LastWriteUtc.ToUniversalTime().ToString('o')
                    sha256    = $hash
                }
                $copiedCount++
                $copiedBytes += $file.SizeBytes
            }
        }

        # 5. Сверка со storage_objects по этому тому.
        if ($dbSummary.checked -and $dbObjectsByVolume.ContainsKey($volume.Id)) {
            $manifestIndex = @{}
            foreach ($entry in $manifestFiles) {
                $manifestIndex[([string]$entry.path).ToLowerInvariant()] = $entry
            }
            foreach ($row in @($dbObjectsByVolume[$volume.Id])) {
                if ($row.State -ne 'committed') { continue }
                $dbSummary.objects++
                $relative = '.nexus/objects/' + $row.ObjectKey
                $relativeKey = $relative.ToLowerInvariant()
                if (-not $sourceObjectIndex.ContainsKey($row.ObjectKey)) {
                    $dbSummary.missingOnDisk++
                    if ($dbSummary.missingSamples.Count -lt 20) {
                        $dbSummary.missingSamples += [ordered]@{
                            volume = $volume.Name
                            key    = $row.ObjectKey
                        }
                    }
                    continue
                }
                $entry = $manifestIndex[$relativeKey]
                if ($entry) {
                    if ([int64]$entry.sizeBytes -ne $row.SizeBytes -or $entry.sha256 -ne $row.Sha256) {
                        $dbSummary.hashMismatch++
                        if ($dbSummary.mismatchSamples.Count -lt 20) {
                            $dbSummary.mismatchSamples += [ordered]@{
                                volume  = $volume.Name
                                key     = $row.ObjectKey
                                expected = $row.Sha256
                                actual   = $entry.sha256
                            }
                        }
                    } else {
                        $dbSummary.verified++
                    }
                } else {
                    # Incremental: файл не копировался в этом наборе. Проверяем
                    # хэш из предыдущего набора, если он там есть. В полном
                    # наборе каждый committed-объект обязан быть в manifest.
                    $known = if ($copyMode -eq 'Incremental') {
                        $previous[(([string]$volume.Id + '|' + $relative).ToLowerInvariant())]
                    } else {
                        $null
                    }
                    if ($known -and [int64]$known.sizeBytes -eq $row.SizeBytes -and $known.sha256 -eq $row.Sha256) {
                        $dbSummary.verified++
                    } else {
                        $dbSummary.hashMismatch++
                        if ($dbSummary.mismatchSamples.Count -lt 20) {
                            $dbSummary.mismatchSamples += [ordered]@{
                                volume  = $volume.Name
                                key     = $row.ObjectKey
                                expected = $row.Sha256
                                actual   = 'нет в наборе и в базе'
                            }
                        }
                    }
                }
            }
            foreach ($file in $sourceFiles) {
                if ($file.Area -ne 'objects') { continue }
                $key = $file.RelativePath.Substring('.nexus/objects/'.Length)
                $known = $false
                foreach ($row in @($dbObjectsByVolume[$volume.Id])) {
                    if ($row.ObjectKey -eq $key) { $known = $true; break }
                }
                if (-not $known) { $dbSummary.untracked++ }
            }
        }

        $manifestVolumes += [ordered]@{
            id        = $volume.Id
            name      = $volume.Name
            rootPath  = $volume.RootPath
            directory = 'volumes/' + $volume.Id
            files     = $manifestFiles
            fileCount = $manifestFiles.Count
            bytes     = $copiedBytes
        }
        $totalFiles += $manifestFiles.Count
        $totalBytes += $copiedBytes
        Write-Host ("    файлов в наборе: {0}, новых/изменённых: {1}, без изменений: {2}, байт: {3:N0}" -f `
            $manifestFiles.Count, $copiedCount, $reusedCount, $copiedBytes)
    }

    if ($dbSummary.checked) {
        $result.databaseCheck = $dbSummary
        if ($dbSummary.missingOnDisk -gt 0 -or $dbSummary.hashMismatch -gt 0) {
            $message = ("сверка со storage_objects: отсутствует {0}, несовпадений {1}" -f `
                $dbSummary.missingOnDisk, $dbSummary.hashMismatch)
            if ($StrictDatabase) { throw $message }
            Write-Host ("    ВНИМАНИЕ: " + $message) -ForegroundColor Yellow
        } else {
            Write-Host ("    сверка со storage_objects: проверено {0}, все совпадают" -f $dbSummary.verified)
        }
    } else {
        $result.databaseCheck = $dbSummary
    }

    # 6. Манифест и контрольные суммы.
    $currentStage = 'manifest'
    $appVersion = ''
    $exe = Join-Path $InstallDir 'nexus.exe'
    if (Test-Path -LiteralPath $exe) {
        try {
            $versionProcess = Invoke-NexusProcess -ExePath $exe -Arguments @('--version')
            $appVersion = ([string]$versionProcess.StdOut).Trim()
        } catch { }
    }
    $manifest = [ordered]@{
        format         = 'nexus-objects/1'
        status         = 'complete'
        createdAt      = (Get-Date).ToString('o')
        label          = $Label
        mode           = $copyMode
        requestedMode  = $Mode
        consistency    = $consistencyUsed
        backupPath     = $setDir
        base           = $baseRef
        sourceHost     = $env:COMPUTERNAME
        appVersion     = $appVersion
        backupDir      = $backupRoot
        volumes        = $manifestVolumes
        totals         = [ordered]@{
            volumes = $manifestVolumes.Count
            files   = $totalFiles
            bytes   = $totalBytes
        }
        database       = $dbSummary
    }
    Write-NexusObjectManifest -Path $setDir -Manifest $manifest
    Write-NexusChecksumFile -Path $setDir
    $manifestHash = Get-NexusFileSha256 (Join-Path $setDir 'manifest.json')
    $result.manifestSha256 = $manifestHash
    $result.files = $totalFiles
    $result.bytes = $totalBytes

    # 7. Retention: только по явному -Prune.
    $currentStage = 'retention'
    # Цепочка неприкосновенна: нельзя удалить последний Full или любой набор
    # после него. Иначе incremental-наборы останутся без базы.
    $existingSets = @()
    foreach ($candidate in @(Get-ChildItem -LiteralPath $backupRoot -Directory -Force |
            Where-Object { $_.Name -like 'objects-*' -and $_.FullName -ne $setDir } | Sort-Object Name)) {
        try {
            $existingSets += [pscustomobject]@{
                FullName = $candidate.FullName
                Name     = $candidate.Name
                Manifest = Get-NexusManifest -Path $candidate.FullName
            }
        } catch {
            Write-Host "    пропускаю набор без валидного манифеста: $($candidate.Name)" -ForegroundColor Yellow
        }
    }
    $allSets = @(@($existingSets + [pscustomobject]@{
        FullName = $setDir
        Name     = (Split-Path -Leaf $setDir)
        Manifest = $manifest
    }) | Sort-Object Name)

    $newestFullIndex = -1
    for ($i = 0; $i -lt $allSets.Count; $i++) {
        if ($allSets[$i].Manifest.mode -eq 'Full') { $newestFullIndex = $i }
    }
    $excess = $allSets.Count - $Retain
    $candidates = @()
    if ($excess -gt 0) {
        if ($newestFullIndex -lt 0) {
            Write-Host "    в каталоге нет полного набора - retention не удаляет наборы" -ForegroundColor Yellow
        } else {
            $deletable = @()
            if ($newestFullIndex -gt 0) {
                $deletable = @($allSets[0..($newestFullIndex - 1)])
            }
            # Удаляем старую цепочку с конца: сначала incremental, затем их
            # Full. Иначе можно удалить базу и оставить сиротские наборы.
            $candidates = @($deletable | Select-Object -Last $excess)
            if ($candidates.Count -lt $excess) {
                Write-Host ("    цепочка после последнего Full длиннее {0} наборов; " -f $Retain) -ForegroundColor Yellow
                Write-Host "    лишние наборы не удалены, чтобы не потерять базу incremental." -ForegroundColor Yellow
            }
            # Финальная страховка: после удаления у каждого оставшегося
            # incremental должен остаться его базовый Full.
            $candidateNames = @($candidates | ForEach-Object { $_.Name })
            $remaining = @($allSets | Where-Object { $candidateNames -notcontains $_.Name })
            $remainingNames = @($remaining | ForEach-Object { $_.Name })
            foreach ($kept in $remaining) {
                if ($kept.Manifest.mode -ne 'Incremental' -or -not $kept.Manifest.base) { continue }
                $baseLeaf = Split-Path -Leaf ([string]$kept.Manifest.base)
                if ($remainingNames -notcontains $baseLeaf) {
                    Write-Host "    retention отменён: у $($kept.Name) пропала бы база $baseLeaf" -ForegroundColor Yellow
                    $candidates = @()
                    break
                }
            }
        }
    }
    $pruned = @()
    if ($candidates.Count -gt 0) {
        if (-not $Prune) {
            Write-Host ("    наборов больше {0}; старые не удалены (нужен -Prune): {1}" -f `
                $Retain, (($candidates | ForEach-Object { $_.Name }) -join ', ')) -ForegroundColor Yellow
        } else {
            $confirmed = [bool]$Force
            if (-not $confirmed) {
                if ([Environment]::UserInteractive) {
                    $answer = Read-Host ("Удалить {0} старых набор(ов)? (yes/no)" -f $candidates.Count)
                    $confirmed = ($answer -eq 'yes')
                } else {
                    Write-Host "    неинтерактивный запуск без -Force: удаление пропущено" -ForegroundColor Yellow
                }
            }
            if ($confirmed) {
                foreach ($old in @($candidates | Sort-Object Name -Descending)) {
                    $parent = [System.IO.Path]::GetFullPath((Split-Path -Parent $old.FullName)).TrimEnd('\')
                    if ($parent -ne $backupRoot -or $old.Name -notlike 'objects-*' -or $old.FullName -eq $setDir) {
                        throw "Отказ от удаления подозрительного каталога: $($old.FullName)"
                    }
                    Remove-Item -LiteralPath $old.FullName -Recurse -Force
                    $pruned += $old.Name
                    Write-Host "    удалён старый набор $($old.Name)"
                }
            }
        }
    }
    $result.pruned = $pruned
    $result.effectiveMode = $copyMode
    $result.pass = $true
    $currentStage = 'done'
    Write-NexusStep "Готово: $setDir"
    Write-Host ("  файлов: {0}, байт: {1:N0}, согласованность: {2}" -f $totalFiles, $totalBytes, $consistencyUsed)
    Write-Host "  манифест: manifest.json, SHA256SUMS"
    if ($dbSummary.checked) {
        Write-Host ("  storage_objects: проверено {0}, отсутствует {1}, несовпадений {2}, вне БД {3}" -f `
            $dbSummary.verified, $dbSummary.missingOnDisk, $dbSummary.hashMismatch, $dbSummary.untracked)
    }
    Write-Host "  Восстановление: deployment\restore-objects.ps1 -BackupPath $setDir"
} catch {
    $result.pass = $false
    $result.stage = $currentStage
    $result.error = $_.Exception.Message
    Write-Host ("ОШИБКА: " + $_.Exception.Message) -ForegroundColor Red
    if ($currentStage -ne 'done') {
        $errorResult = [ordered]@{
            status    = 'failed'
            stage     = $currentStage
            createdAt = (Get-Date).ToString('o')
            error     = $_.Exception.Message
            mode      = $Mode
            consistency = $Consistency
        }
        try {
            $errorResult | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath `
                (Join-Path $setDir 'backup-result.json') -Encoding UTF8
        } catch { }
    }
    throw
} finally {
    foreach ($snapshot in $snapshots.Values) { Remove-NexusVssSnapshot -Snapshot $snapshot }
    if ($serviceStopped) {
        Write-NexusStep "Запуск службы $ServiceName"
        try {
            $status = Start-NexusService -Name $ServiceName
            $result.serviceAfter = [string]$status
            Write-Host "    $status" -ForegroundColor Green
        } catch {
            Write-Host ("    не удалось запустить службу: " + $_.Exception.Message) -ForegroundColor Red
        }
    }
    if ($pgPassFile) { Remove-NexusPgPassFile -Path $pgPassFile }
    $result.finishedAt = (Get-Date).ToString('o')
    if ($currentStage -eq 'done') {
        try {
            $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath `
                (Join-Path $setDir 'backup-result.json') -Encoding UTF8
        } catch { }
    }
    if ($ResultPath) {
        $dir = Split-Path -Parent $ResultPath
        if ($dir -and -not (Test-Path -LiteralPath $dir)) {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
        }
        $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
    }
}
