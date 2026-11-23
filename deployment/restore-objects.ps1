# NEXUS WORKSPACE - восстановление бинарных объектов (Windows).
#
# Принимает один набор backup-objects.ps1, каталог с наборами или цепочку
# full + incremental. Копирование идёт от старых наборов к новым, затем
# проверяются SHA-256, состав storage_objects и права службы (icacls).
#
# Примеры:
#   # Восстановить полный набор в исходный путь тома:
#   powershell -ExecutionPolicy Bypass -File deployment\restore-objects.ps1 `
#       -BackupPath C:\NexusObjectBackups\objects-20261120-135207 -Force
#
#   # Восстановить цепочку incremental:
#   ... -BackupPath C:\NexusObjectBackups\objects-20261120-135207, `
#                    C:\NexusObjectBackups\objects-20261120-140000 -Force
#
#   # В тестовый каталог (только для одного тома):
#   ... -BackupPath ... -TargetRoot C:\NexusRestoreTest -Force
#
# Служба останавливается на время записи и запускается в finally. Права
# выдаются учётной записи службы (по умолчанию StartName из Win32_Service,
# иначе NT AUTHORITY\SYSTEM) через icacls с наследованием и /T.
#
# Удаление файлов, которых нет в наборе, не выполняется: restore только
# добавляет/перезаписывает. Это защищает от случайной потери данных.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string[]]$BackupPath,
    [string]$InstallDir = 'C:\NexusWorkspace',
    [string]$ServiceName = 'NexusWorkspace',
    [string]$PgBin = '',
    [string]$TargetRoot = '',
    [string]$AclAccount = '',
    [switch]$SkipAcl,
    [switch]$SkipVerify,
    [switch]$SkipDatabaseCheck,
    [switch]$RequireDatabase,
    [switch]$SkipServiceStop,
    [switch]$SkipStart,
    [switch]$SkipHealthCheck,
    [switch]$AllowIncompleteChain,
    [switch]$Force,
    [int]$Port = 8080,
    [string]$ResultPath = ''
)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptDir 'nexus-common.ps1')
. (Join-Path $scriptDir 'objects-common.ps1')

function Resolve-NexusRestoreSets {
    param([string[]]$Paths)
    $sets = @()
    foreach ($inputPath in $Paths) {
        if (-not (Test-Path -LiteralPath $inputPath)) {
            throw "Путь резервной копии не найден: $inputPath"
        }
        $full = [System.IO.Path]::GetFullPath($inputPath)
        if (Test-Path -LiteralPath (Join-Path $full 'manifest.json')) {
            $sets += [pscustomobject]@{
                Dir      = $full
                Manifest = Get-NexusManifest -Path $full
            }
            continue
        }
        $children = @(Get-ChildItem -LiteralPath $full -Directory -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like 'objects-*' -and
                (Test-Path -LiteralPath (Join-Path $_.FullName 'manifest.json')) } |
            Sort-Object Name)
        if ($children.Count -eq 0) {
            throw "В $full нет manifest.json и нет наборов objects-*."
        }
        foreach ($child in $children) {
            $sets += [pscustomobject]@{
                Dir      = $child.FullName
                Manifest = Get-NexusManifest -Path $child.FullName
            }
        }
    }
    $unique = @{}
    foreach ($set in $sets) { $unique[$set.Dir.ToLowerInvariant()] = $set }
    return @($unique.Values | Sort-Object @{ Expression = { $_.Manifest.createdAt } }, @{ Expression = { $_.Dir } })
}

$sets = @(Resolve-NexusRestoreSets -Paths $BackupPath)
if ($sets.Count -eq 0) { throw 'Не найдено ни одного набора для восстановления.' }

$result = [ordered]@{
    startedAt   = (Get-Date).ToString('o')
    backupPaths = @($sets | ForEach-Object { $_.Dir })
    targetRoot  = $TargetRoot
    service     = $ServiceName
    serviceStopped = $false
    pass        = $false
}

$serviceStopped = $false
$pgPassFile = $null
$db = $null
$failureMessages = @()
$targets = @{}
$expected = @{}
$hashByPath = @{}
$logRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('nexus-restore-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$currentStage = 'init'

try {
    Assert-NexusAdmin

    # 1. Проверка цепочки: incremental без полного набора бесполезен.
    $setDirs = @($sets | ForEach-Object { [System.IO.Path]::GetFullPath($_.Dir) })
    $hasFull = @($sets | Where-Object { $_.Manifest.mode -eq 'Full' }).Count -gt 0
    if (-not $hasFull -and -not $AllowIncompleteChain) {
        throw 'В выбранных наборах нет полного (Full). Укажите цепочку с базовым набором или -AllowIncompleteChain.'
    }
    $setLeaves = @($sets | ForEach-Object { Split-Path -Leaf $_.Dir })
    foreach ($set in $sets) {
        if ($set.Manifest.mode -ne 'Incremental') { continue }
        $base = [string]$set.Manifest.base
        if ($base) {
            $baseFull = [System.IO.Path]::GetFullPath($base)
            # Наборы могли перенести на другой носитель: тогда полный путь в
            # base не совпадёт, но имя каталога objects-* остаётся тем же.
            $baseByLeaf = $setLeaves -contains (Split-Path -Leaf $baseFull)
            if ($setDirs -notcontains $baseFull -and -not $baseByLeaf) {
                Write-Host "    ВНИМАНИЕ: базовый набор $base не входит в восстановление" -ForegroundColor Yellow
                if (-not $AllowIncompleteChain) {
                    throw "Цепочка неполная: для $($set.Dir) нужен базовый набор $base."
                }
            }
        }
    }

    # 2. Целевые каталоги.
    $volumeEntries = [ordered]@{}
    foreach ($set in $sets) {
        foreach ($manifestVolume in $set.Manifest.volumes) {
            $id = [string]$manifestVolume.id
            $rootPath = [string]$manifestVolume.rootPath
            if (-not $volumeEntries.Contains($id)) {
                $volumeEntries[$id] = [pscustomobject]@{
                    Id       = $id
                    Name     = [string]$manifestVolume.name
                    RootPath = $rootPath
                }
            } elseif ($rootPath) {
                $volumeEntries[$id].RootPath = $rootPath
            }
        }
    }
    if ($volumeEntries.Count -eq 0) { throw 'В манифесте нет томов.' }

    if ($TargetRoot) {
        if ($volumeEntries.Count -ne 1) {
            throw "-TargetRoot допустим только для одного тома; в наборе их $($volumeEntries.Count)."
        }
        $target = [System.IO.Path]::GetFullPath($TargetRoot).TrimEnd('\')
        $driveRoot = [System.IO.Path]::GetPathRoot($target).TrimEnd('\')
        if ($target -ieq $driveRoot) { throw 'Нельзя восстанавливать в корень диска.' }
        foreach ($entry in $volumeEntries.Values) { $targets[$entry.Id] = $target }
    } else {
        foreach ($entry in $volumeEntries.Values) {
            if (-not $entry.RootPath) { throw "Для тома $($entry.Id) не указан исходный путь." }
            $target = [System.IO.Path]::GetFullPath($entry.RootPath).TrimEnd('\')
            $driveRoot = [System.IO.Path]::GetPathRoot($target).TrimEnd('\')
            if ($target -ieq $driveRoot) { throw "Нельзя восстанавливать в корень диска: $target" }
            $targets[$entry.Id] = $target
        }
    }

    Write-NexusStep 'Восстановление бинарных объектов'
    foreach ($entry in $volumeEntries.Values) {
        Write-Host ("    том {0}: {1} -> {2}" -f $entry.Name, $entry.RootPath, $targets[$entry.Id])
    }

    if (-not $Force) {
        if (-not [Environment]::UserInteractive) {
            throw 'Неинтерактивный запуск: подтвердите восстановление флагом -Force.'
        }
        $answer = Read-Host 'Файлы будут перезаписаны. Продолжить? (yes/no)'
        if ($answer -ne 'yes') {
            Write-Host 'Отменено.'
            $result.cancelled = $true
            $result.pass = $true
            return
        }
    }

    # 3. Параметры БД для сверки.
    if (-not $SkipDatabaseCheck) {
        $envPath = Join-Path $InstallDir '.env'
        if (Test-Path -LiteralPath $envPath) {
            $config = Get-NexusEnvConfig -Path $envPath
            if ($config['DATABASE_URL']) {
                $db = ConvertFrom-NexusDatabaseUrl -DatabaseUrl $config['DATABASE_URL']
                $pgPassFile = New-NexusPgPassFile -Database $db
                $result.database = $db.Database
            } elseif ($RequireDatabase) {
                throw "В $envPath нет DATABASE_URL."
            }
        } elseif ($RequireDatabase) {
            throw "Не найден $envPath для сверки со storage_objects."
        } else {
            Write-Host '    .env не найден - сверка со storage_objects пропущена' -ForegroundColor Yellow
        }
    }

    # 4. Остановка службы: восстановление не должно конкурировать за файлы.
    if (-not $SkipServiceStop) {
        $service = Get-NexusServiceObject -Name $ServiceName
        if ($service -and $service.Status -eq 'Running') {
            Write-NexusStep "Остановка службы $ServiceName"
            $serviceStopped = Stop-NexusService -Name $ServiceName -TimeoutSeconds 60
            $result.serviceStopped = $serviceStopped
            Write-Host '    служба остановлена'
        }
    } else {
        Write-Host '    ВНИМАНИЕ: служба не останавливается (-SkipServiceStop)' -ForegroundColor Yellow
    }

    # Создаём целевые корни заранее.
    foreach ($target in $targets.Values) {
        [void][System.IO.Directory]::CreateDirectory((ConvertTo-NexusLongPath $target))
    }

    # 5. Копирование от старых наборов к новым.
    $currentStage = 'copy'
    foreach ($set in $sets) {
        Write-NexusStep ("Набор {0} ({1})" -f (Split-Path -Leaf $set.Dir), $set.Manifest.mode)
        foreach ($manifestVolume in $set.Manifest.volumes) {
            $id = [string]$manifestVolume.id
            if (-not $targets.ContainsKey($id)) {
                throw "Нет целевого каталога для тома $id."
            }
            $relativeDir = ([string]$manifestVolume.directory).Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $source = Join-Path $set.Dir $relativeDir
            if (-not (Test-Path -LiteralPath $source)) { continue }
            $target = $targets[$id]
            $logPath = Join-Path $logRoot (('restore-' + $id + '-' + (Split-Path -Leaf $set.Dir) + '.log'))
            $copy = Invoke-NexusRobocopy -Source $source -Destination $target -LogPath $logPath -Threads 8
            if ($copy.Code -ge 8) {
                throw "robocopy восстановления завершился с кодом $($copy.Code). Журнал: $logPath"
            }
            foreach ($file in $manifestVolume.files) {
                $destination = Join-Path $target ([string]$file.path)
                $key = $destination.ToLowerInvariant()
                $expected[$key] = [pscustomobject]@{
                    Path        = [string]$file.path
                    Destination = $destination
                    SizeBytes   = [int64]$file.sizeBytes
                    Sha256      = [string]$file.sha256
                    Set         = $set.Dir
                    VolumeId    = $id
                }
            }
        }
    }
    if ($expected.Count -eq 0) {
        throw 'В выбранных наборах нет файлов для восстановления (возможно, только пустые incremental).'
    }

    # 6. Права службы.
    $currentStage = 'acl'
    if (-not $SkipAcl) {
        $account = if ($AclAccount) { $AclAccount } else { Get-NexusServiceAccount -ServiceName $ServiceName }
        Write-NexusStep "Права icacls для $account"
        $applied = @{}
        foreach ($target in ($targets.Values | Select-Object -Unique)) {
            if ($applied.ContainsKey($target.ToLowerInvariant())) { continue }
            Set-NexusObjectAcl -Path $target -Account $account -Recurse
            $applied[$target.ToLowerInvariant()] = $true
            Write-Host "    $target"
        }
        $result.aclAccount = $account
    } else {
        Write-Host '    ВНИМАНИЕ: права не выдавались (-SkipAcl)' -ForegroundColor Yellow
        $result.aclAccount = ''
    }

    # 7. Проверка SHA-256.
    $currentStage = 'verify'
    $verified = 0
    $missing = 0
    $mismatched = 0
    $samples = @()
    if ($SkipVerify) {
        Write-Host '    ВНИМАНИЕ: проверка SHA-256 пропущена (-SkipVerify)' -ForegroundColor Yellow
    } else {
        Write-NexusStep "Проверка SHA-256: файлов $($expected.Count)"
        $index = 0
        foreach ($entry in $expected.Values) {
            $index++
            $longPath = ConvertTo-NexusLongPath $entry.Destination
            if (-not [System.IO.File]::Exists($longPath)) {
                $missing++
                if ($samples.Count -lt 20) {
                    $samples += [ordered]@{ path = $entry.Path; problem = 'файл отсутствует'; set = $entry.Set }
                }
                continue
            }
            $size = (New-Object System.IO.FileInfo($longPath)).Length
            $hash = Get-NexusFileSha256 $entry.Destination
            $hashByPath[$entry.Destination.ToLowerInvariant()] = $hash
            if ($size -ne $entry.SizeBytes -or $hash -ne $entry.Sha256) {
                $mismatched++
                if ($samples.Count -lt 20) {
                    $samples += [ordered]@{
                        path     = $entry.Path
                        problem  = 'размер или SHA-256 не совпадают'
                        expected = $entry.Sha256
                        actual   = $hash
                        set      = $entry.Set
                    }
                }
            } else {
                $verified++
            }
            if ($index % 1000 -eq 0) { Write-Host ("    проверено {0} из {1}" -f $index, $expected.Count) }
        }
        $result.verification = [ordered]@{
            files      = $expected.Count
            verified   = $verified
            missing    = $missing
            mismatched = $mismatched
            samples    = $samples
        }
        Write-Host ("    проверено {0}, отсутствует {1}, несовпадений {2}" -f $verified, $missing, $mismatched)
        if ($missing -gt 0 -or $mismatched -gt 0) {
            $failureMessages += "проверка SHA-256: отсутствует $missing, несовпадений $mismatched"
        }
    }

    # 8. Сверка с storage_objects: каждый committed-объект должен быть на месте.
    $currentStage = 'database'
    if ($db) {
        Write-NexusStep 'Сверка со storage_objects'
        $volumeIds = @($targets.Keys | Where-Object { $_ -notlike 'local-*' })
        $dbSummary = [ordered]@{
            checked         = $true
            objects         = 0
            verified        = 0
            missing         = 0
            mismatch        = 0
            untracked       = 0
            missingSamples  = @()
            mismatchSamples = @()
        }
        if ($volumeIds.Count -gt 0) {
            $dbObjects = Get-NexusStorageObjectRecords -Database $db -VolumeIds $volumeIds -PgBin $PgBin
            $byVolume = @{}
            foreach ($row in $dbObjects) {
                if (-not $byVolume.ContainsKey($row.VolumeId)) { $byVolume[$row.VolumeId] = @() }
                $byVolume[$row.VolumeId] += $row
            }
            foreach ($volumeId in $volumeIds) {
                if (-not $byVolume.ContainsKey($volumeId)) { continue }
                $target = $targets[$volumeId]
                $dbKeys = @{}
                foreach ($row in @($byVolume[$volumeId])) { $dbKeys[$row.ObjectKey] = $true }
                foreach ($row in @($byVolume[$volumeId])) {
                    if ($row.State -ne 'committed') { continue }
                    $dbSummary.objects++
                    $destination = Join-Path $target ('.nexus/objects/' + $row.ObjectKey)
                    $longPath = ConvertTo-NexusLongPath $destination
                    if (-not [System.IO.File]::Exists($longPath)) {
                        $dbSummary.missing++
                        if ($dbSummary.missingSamples.Count -lt 20) {
                            $dbSummary.missingSamples += [ordered]@{ volume = $volumeId; key = $row.ObjectKey }
                        }
                        continue
                    }
                    $size = (New-Object System.IO.FileInfo($longPath)).Length
                    $hash = $hashByPath[$destination.ToLowerInvariant()]
                    if (-not $hash) { $hash = Get-NexusFileSha256 $destination }
                    if ($size -ne $row.SizeBytes -or $hash -ne $row.Sha256) {
                        $dbSummary.mismatch++
                        if ($dbSummary.mismatchSamples.Count -lt 20) {
                            $dbSummary.mismatchSamples += [ordered]@{
                                volume   = $volumeId
                                key      = $row.ObjectKey
                                expected = $row.Sha256
                                actual   = $hash
                            }
                        }
                    } else {
                        $dbSummary.verified++
                    }
                }
                # Файлы objects, которых нет в storage_objects: quarantine и
                # остатки после сбоя. Это предупреждение, не ошибка.
                $targetFiles = @(Get-NexusVolumeFiles -VolumeRoot $target |
                    Where-Object { $_.Area -eq 'objects' })
                foreach ($file in $targetFiles) {
                    $key = $file.RelativePath.Substring('.nexus/objects/'.Length)
                    if (-not $dbKeys.ContainsKey($key)) { $dbSummary.untracked++ }
                }
            }
            $result.databaseCheck = $dbSummary
            Write-Host ("    объектов {0}: совпало {1}, отсутствует {2}, несовпадений {3}, вне БД {4}" -f `
                $dbSummary.objects, $dbSummary.verified, $dbSummary.missing, $dbSummary.mismatch, $dbSummary.untracked)
            if ($dbSummary.missing -gt 0 -or $dbSummary.mismatch -gt 0) {
                $failureMessages += "сверка storage_objects: отсутствует $($dbSummary.missing), несовпадений $($dbSummary.mismatch)"
            }
        } else {
            Write-Host '    том не зарегистрирован в БД - сверка пропущена' -ForegroundColor Yellow
        }
    } else {
        $result.databaseCheck = [ordered]@{ checked = $false }
    }

    if ($failureMessages.Count -gt 0) {
        throw ('Восстановление завершено с ошибками: ' + ($failureMessages -join '; '))
    }

    $result.pass = $true
    $currentStage = 'done'
    Write-NexusStep 'Восстановление объектов завершено'
    Write-Host ("  файлов: {0}, байт: {1:N0}" -f $expected.Count, (($expected.Values | Measure-Object SizeBytes -Sum).Sum))
    Write-Host ("  журналы robocopy: {0}" -f $logRoot)
    if ($result.aclAccount) { Write-Host ("  права: {0}" -f $result.aclAccount) }
} catch {
    $result.pass = $false
    $result.stage = $currentStage
    $result.error = $_.Exception.Message
    Write-Host ("ОШИБКА: " + $_.Exception.Message) -ForegroundColor Red
} finally {
    if ($serviceStopped -and -not $SkipStart) {
        Write-NexusStep "Запуск службы $ServiceName"
        try {
            $status = Start-NexusService -Name $ServiceName
            $result.serviceAfter = [string]$status
            Write-Host "    $status" -ForegroundColor Green
            if (-not $SkipHealthCheck) {
                $health = Wait-NexusHttp -Port $Port -TimeoutSeconds 40
                $result.healthStatus = $health
                Write-Host ("    health: {0}" -f $health)
                if ($health -ne 200) {
                    $result.pass = $false
                    $result.error = "Служба не отвечает health=200 (получено $health)."
                    Write-Host ("ОШИБКА: " + $result.error) -ForegroundColor Red
                }
            }
        } catch {
            $result.pass = $false
            $result.error = "Не удалось запустить службу: $($_.Exception.Message)"
            Write-Host ("ОШИБКА: " + $result.error) -ForegroundColor Red
        }
    }
    if ($pgPassFile) { Remove-NexusPgPassFile -Path $pgPassFile }
    $result.finishedAt = (Get-Date).ToString('o')
    if ($ResultPath) {
        $dir = Split-Path -Parent $ResultPath
        if ($dir -and -not (Test-Path -LiteralPath $dir)) {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
        }
        $result | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
    }
    if (-not $result.pass) {
        # Возвращаем код 1 после того, как результат и служба уже обработаны.
        exit 1
    }
}
