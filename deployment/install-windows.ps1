# NEXUS WORKSPACE - установка на Windows 11 / Windows Server.
#
# Запуск от имени администратора:
#   powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1
#
# Скрипт создаёт каталог установки, копирует сборку, готовит .env и nexus.toml,
# регистрирует службу и запускает её. Повторный запуск обновляет файлы.

[CmdletBinding()]
param(
    [string]$InstallDir = "C:\NexusWorkspace",
    [string]$ServiceName = "NexusWorkspace",
    # Virtual — виртуальная учётная запись NT SERVICE\<ServiceName>: без пароля,
    # без интерактивного входа и с доступом только к каталогам установки.
    [ValidateSet('Virtual', 'LocalSystem', 'LocalService', 'NetworkService', 'Custom')]
    [string]$ServiceAccountType = "Virtual",
    # Для -ServiceAccountType Custom: DOMAIN\user или .\user.
    [string]$ServiceAccount = "",
    [Security.SecureString]$ServicePassword,
    # Дополнительные каталоги хранения вне InstallDir (например, D:\NexusStorage).
    [string[]]$StoragePaths = @(),
    # Проверить Stop/Start/Restart-Service после установки (приёмка J1).
    [switch]$TestServiceControl,
    # Проверка портов: 8080 должен быть свободен, 80/443 — не в исключённых
    # диапазонах Hyper-V/WSL (занятость Caddy — норма).
    [switch]$SkipPortCheck,
    [int]$HttpPort = 80,
    [int]$HttpsPort = 443,
    # Открыть 80/443 в Windows Firewall (для Caddy на этой машине).
    [switch]$ConfigureFirewall,
    # Явная зависимость службы от PostgreSQL. Пусто — определить локальный
    # postgresql* автоматически по DATABASE_URL (127.0.0.1/localhost).
    [string]$PostgresService = "",
    [string]$Domain = "",
    [string]$DatabaseUrl = "",
    [string]$Secret = "",
    [string]$AdminPassword = "",
    [switch]$SkipAdmin,
    [switch]$SkipService,
    [switch]$SkipBuild,
    # Считать включённые длинные пути обязательным требованием хоста:
    # при LongPathsEnabled=0 установка завершится ошибкой, а не предупреждением.
    [switch]$RequireLongPaths
)

$ErrorActionPreference = 'Stop'

function Write-Step($text) {
    Write-Host ""
    Write-Host "==> $text" -ForegroundColor Cyan
}

function New-SecretHex {
    $bytes = New-Object byte[] 32
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    return (($bytes | ForEach-Object { $_.ToString('x2') }) -join '')
}

# Загружает .env в переменные текущего процесса. dotenvy не переопределяет уже
# заданные переменные, поэтому для установки значения задаём явно.
function Import-EnvFile {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return }
    Get-Content $Path | Where-Object { $_ -match '^\s*[A-Za-z_][A-Za-z0-9_]*=' } | ForEach-Object {
        $parts = $_ -split '=', 2
        $name = $parts[0].Trim()
        $value = $parts[1].Trim()
        Set-Item -Path "Env:$name" -Value $value
    }
}

# Запускает nexus.exe из каталога установки: dotenvy ищет .env в рабочем каталоге.
# Вывод перенаправляется в файлы: иначе PowerShell превращает stderr программы
# (а nexus пишет туда сообщения о проблемах) в NativeCommandError и, при
# ErrorActionPreference=Stop, обрывает установку.
function Invoke-Nexus {
    param([string[]]$NexusArgs, [switch]$AllowFailure)
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $process = Start-Process -FilePath (Join-Path $InstallDir 'nexus.exe') `
            -ArgumentList $NexusArgs `
            -WorkingDirectory $InstallDir `
            -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput $stdoutFile `
            -RedirectStandardError $stderrFile
        $code = $process.ExitCode
        Get-Content $stdoutFile -ErrorAction SilentlyContinue | Out-Host
        Get-Content $stderrFile -ErrorAction SilentlyContinue | Out-Host
    } finally {
        Remove-Item $stdoutFile, $stderrFile -ErrorAction SilentlyContinue
    }
    if ($code -ne 0 -and -not $AllowFailure) {
        throw "nexus $($NexusArgs -join ' ') завершился с кодом $code"
    }
    return $code
}

# Выполняет nexus.exe и возвращает (stdout, stderr) без вывода в консоль.
function Get-NexusOutput {
    param([string[]]$NexusArgs)
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $process = Start-Process -FilePath (Join-Path $InstallDir 'nexus.exe') `
            -ArgumentList $NexusArgs `
            -WorkingDirectory $InstallDir `
            -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput $stdoutFile `
            -RedirectStandardError $stderrFile
        return [pscustomobject]@{
            Code   = $process.ExitCode
            StdOut = (Get-Content $stdoutFile -Raw -ErrorAction SilentlyContinue)
        }
    } finally {
        Remove-Item $stdoutFile, $stderrFile -ErrorAction SilentlyContinue
    }
}

# Возвращает имя учётной записи службы в формате, который понимает sc.exe.
function Resolve-NexusServiceAccount {
    param([string]$Type, [string]$Name, [string]$Service)
    switch ($Type) {
        'Virtual' { return "NT SERVICE\$Service" }
        'LocalSystem' { return "LocalSystem" }
        'LocalService' { return "NT AUTHORITY\LocalService" }
        'NetworkService' { return "NT AUTHORITY\NetworkService" }
        'Custom' {
            if ([string]::IsNullOrWhiteSpace($Name)) {
                throw "Для -ServiceAccountType Custom укажите -ServiceAccount DOMAIN\user."
            }
            return $Name
        }
        default { throw "Неизвестный тип учётной записи службы: $Type" }
    }
}

# Выдаёт учётной записи службы доступ к каталогу/файлу. LocalSystem и
# встроенные службы уже имеют нужные права, поэтому вызывается только для
# виртуальной или выделенной учётной записи.
function Grant-NexusPathAccess {
    param(
        [string]$Path,
        [string]$Account,
        [string]$Rights,
        [switch]$File
    )
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $acl = if ($File) { "${Account}:$Rights" } else { "${Account}:(OI)(CI)$Rights" }
    if ($File) {
        & icacls $Path /grant $acl /C /Q | Out-Null
    } else {
        & icacls $Path /grant $acl /T /C /Q | Out-Null
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось выдать ${Account} права $Rights на $Path"
    }
}

# Пароль для sc.exe: SecureString нельзя передать напрямую. Строка живёт
# только в этом процессе и затирается в finally.
function ConvertTo-PlainText {
    param([Security.SecureString]$Value)
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($Value)
    try {
        return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
    }
}

# Останавливает службу и ждёт StopPending → Stopped. Ошибка 1053 здесь так же
# важна, как при Start-Service: она означает, что процесс не отвечает SCM.
function Stop-NexusService {
    param([string]$Name, [int]$TimeoutSeconds = 40)
    $service = Get-Service -Name $Name -ErrorAction SilentlyContinue
    if (-not $service -or $service.Status -eq 'Stopped') { return }
    Stop-Service -Name $Name -Force -ErrorAction Stop
    $service.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds($TimeoutSeconds))
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Запустите скрипт от имени администратора."
}

$repo = Split-Path -Parent $PSScriptRoot
if ($ServiceName -notmatch '^[A-Za-z0-9_.-]+$') {
    throw "Недопустимое имя службы: '$ServiceName'. Разрешены буквы, цифры, '_', '-' и '.'."
}

# K3: проверка требования хоста. NEXUS ограничивает виртуальные пути 240
# UTF-16 единицами, а физические объекты лежат по коротким ключам, поэтому
# установка работает и при LongPathsEnabled=0. Однако каталоги пользователя,
# backup-скрипты и восстановление длинных деревьев требуют поддержки длинных
# путей на уровне ОС. По умолчанию — предупреждение; -RequireLongPaths
# превращает его в ошибку приёмки.
$longPathsValue = $null
$fileSystemKey = 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem'
$fileSystem = Get-ItemProperty -Path $fileSystemKey -Name LongPathsEnabled -ErrorAction SilentlyContinue
if ($fileSystem) { $longPathsValue = $fileSystem.LongPathsEnabled }
if ($longPathsValue -ne 1) {
    $message = "LongPathsEnabled=$longPathsValue в $fileSystemKey. " +
        'Включите длинные пути и перезагрузите хост: ' +
        'Set-ItemProperty -Path ' + $fileSystemKey + ' -Name LongPathsEnabled -Value 1'
    if ($RequireLongPaths) { throw $message }
    Write-Host "    ВНИМАНИЕ: $message" -ForegroundColor Yellow
} else {
    Write-Host "    LongPathsEnabled=1" -ForegroundColor DarkGray
}

Write-Step "Каталог установки: $InstallDir"

# 1. Сборка
if (-not $SkipBuild) {
    Write-Step "Сборка backend (release)"
    $env:PATH = "C:\Program Files\nodejs;" + $env:PATH
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (-not ($env:PATH -like "*$cargoBin*")) {
        $env:PATH = "$cargoBin;" + $env:PATH
    }
    Push-Location (Join-Path $repo 'backend')
    try {
        cargo build --release
    } finally {
        Pop-Location
    }

    Write-Step "Сборка frontend"
    Push-Location (Join-Path $repo 'frontend')
    try {
        if (-not (Test-Path 'node_modules')) { npm install }
        npm run build
    } finally {
        Pop-Location
    }
}

# 2. Каталоги
Write-Step "Подготовка каталогов"
$logDir = Join-Path $InstallDir 'logs'
$dataDir = Join-Path $InstallDir 'data'
$frontendDir = Join-Path $InstallDir 'frontend'
foreach ($dir in @($InstallDir, $frontendDir, $logDir, $dataDir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

# 3. Файлы. Запущенная служба держит nexus.exe, поэтому останавливаем её до
# копирования; повторный запуск — в шаге 7.
if (-not $SkipService) {
    $running = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    if ($running -and $running.Status -ne 'Stopped') {
        Write-Step "Остановка службы для обновления файлов"
        Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
    }
}

Write-Step "Копирование файлов"
# Два варианта поставки:
#  * репозиторий: backend\target\release\nexus.exe + frontend\dist;
#  * release-архив: nexus.exe + frontend\ рядом с deployment\.
$exeSource = Join-Path $repo 'backend\target\release\nexus.exe'
if (-not (Test-Path -LiteralPath $exeSource)) {
    $exeSource = Join-Path $repo 'nexus.exe'
}
if (-not (Test-Path -LiteralPath $exeSource)) {
    throw "Не найден nexus.exe. Соберите проект (cargo build --release) или распакуйте release-архив и запустите с -SkipBuild."
}
Copy-Item -LiteralPath $exeSource -Destination (Join-Path $InstallDir 'nexus.exe') -Force
Copy-Item (Join-Path $repo 'deployment\nexus.production.toml') (Join-Path $InstallDir 'nexus.toml') -Force
Copy-Item (Join-Path $repo 'deployment\env.production.example') (Join-Path $InstallDir '.env.example') -Force
$builtFrontend = Join-Path $repo 'frontend\dist'
if (-not (Test-Path -LiteralPath $builtFrontend)) {
    $builtFrontend = Join-Path $repo 'frontend'
}
if (Test-Path -LiteralPath $builtFrontend) {
    Copy-Item (Join-Path $builtFrontend '*') $frontendDir -Recurse -Force
}

# 4. Конфигурация: .env переопределяет nexus.toml, поэтому пути задаются здесь.
$envPath = Join-Path $InstallDir '.env'
if (-not (Test-Path $envPath)) {
    Write-Step "Создание .env"
    $publicUrl = "http://localhost:8080"
    $cookieSecure = "false"
    if ($Domain) {
        $publicUrl = "https://" + $Domain
        $cookieSecure = "true"
    }
    $dbUrl = "postgres://nexus:CHANGE_ME@127.0.0.1:5432/nexus"
    if ($DatabaseUrl) { $dbUrl = $DatabaseUrl }
    $secretValue = $Secret
    if (-not $secretValue) { $secretValue = New-SecretHex }

    # dotenvy не разбирает обратные слэши в значениях, поэтому пути записываем
    # в прямых слэшах: Windows их принимает.
    $frontendDirEnv = $frontendDir -replace '\\', '/'
    $logDirEnv = $logDir -replace '\\', '/'

    $lines = @()
    $lines += "# Создано install-windows.ps1"
    $lines += "DATABASE_URL=$dbUrl"
    $lines += "NEXUS_SECRET=$secretValue"
    $lines += "NEXUS_HOST=127.0.0.1"
    $lines += "NEXUS_PORT=8080"
    $lines += "NEXUS_PUBLIC_URL=$publicUrl"
    $lines += "NEXUS_COOKIE_SECURE=$cookieSecure"
    $lines += "NEXUS_STATIC_DIR=$frontendDirEnv"
    $lines += "NEXUS_TRUST_PROXY=true"
    $lines += "NEXUS_LOG_DIR=$logDirEnv"
    $lines += "NEXUS_LOG_FILE=nexus.log"
    $lines += "NEXUS_LOG_KEEP=7"
    $lines += "RUST_LOG=nexus=info,tower_http=warn,sqlx=warn"
    [System.IO.File]::WriteAllLines($envPath, $lines, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "    .env создан. Укажите реальный пароль PostgreSQL в DATABASE_URL." -ForegroundColor Yellow
} else {
    Write-Host "    .env уже существует - не изменяю." -ForegroundColor Yellow
}

# В шаблоне nexus.toml остаётся относительный static_dir; .env переопределяет его.
$tomlPath = Join-Path $InstallDir 'nexus.toml'
$tomlText = Get-Content $tomlPath -Raw
$tomlText = $tomlText -replace 'static_dir\s*=\s*"[^"]*"', ('static_dir = "' + ($frontendDir -replace '\\', '\\') + '"')
[System.IO.File]::WriteAllText($tomlPath, $tomlText, (New-Object System.Text.UTF8Encoding($false)))

# 5. Миграции и проверка. Значения из .env загружаем явно: dotenvy не
# переопределяет переменные, оставшиеся в текущем процессе от прошлых запусков.
Import-EnvFile -Path $envPath

# 4.1. Порты. 8080 должен быть свободен (служба уже остановлена). Для 80/443
# занятость Caddy — норма, критичны только исключённые диапазоны Hyper-V/WSL
# и отказ bind по AccessDenied (HTTP.sys/URL ACL).
if (-not $SkipPortCheck) {
    Write-Step "Проверка портов"
    $backendPort = 8080
    if ($env:NEXUS_PORT) {
        $backendPort = [int]($env:NEXUS_PORT.Trim('"', "'"))
    }
    & (Join-Path $PSScriptRoot 'check-ports.ps1') -Ports @($backendPort) -RequireFree
    if ($LASTEXITCODE -ne 0) {
        throw "Порт $backendPort занят или зарезервирован. Освободите его (IIS/HTTP.sys/Hyper-V/WSL) и повторите установку."
    }
    & (Join-Path $PSScriptRoot 'check-ports.ps1') -Ports @($HttpPort, $HttpsPort) -NoBindTest
    if ($LASTEXITCODE -ne 0) {
        throw "Порт $HttpPort/$HttpsPort входит в исключённый диапазон. Проверьте netsh interface ipv4 show excludedportrange protocol=tcp."
    }
}

Write-Step "Применение миграций"
$null = Invoke-Nexus -NexusArgs @('migrate')
$checkCode = Invoke-Nexus -NexusArgs @('check') -AllowFailure
if ($checkCode -ne 0) {
    Write-Host "    Проверка нашла проблемы - устраните их перед запуском службы." -ForegroundColor Yellow
}

# 6. Администратор (по машиночитаемому выводу check --json)
$statusText = (Get-NexusOutput -NexusArgs @('check', '--json')).StdOut
$status = $null
if ($statusText) {
    try {
        $status = $statusText | ConvertFrom-Json
    } catch {
        $status = $null
    }
}
if ($SkipAdmin) {
    Write-Host "    Создание администратора пропущено (-SkipAdmin)." -ForegroundColor Yellow
} elseif ($null -ne $status -and -not $status.admin_present) {
    Write-Step "Создание администратора"
    $adminArgs = @('create-admin', '--username', 'admin')
    if ($AdminPassword) {
        # Пароль передаётся через переменную окружения, а не аргументом:
        # аргументы видны в списке процессов.
        $env:NEXUS_ADMIN_PASSWORD = $AdminPassword
        $adminArgs += @('--password-env', 'NEXUS_ADMIN_PASSWORD')
    } else {
        Write-Host "    Введите пароль администратора (не отображается)." -ForegroundColor Yellow
    }
    Push-Location $InstallDir
    try {
        $null = Invoke-Nexus -NexusArgs $adminArgs
    } finally {
        Pop-Location
        Remove-Item Env:NEXUS_ADMIN_PASSWORD -ErrorAction SilentlyContinue
    }
} elseif ($null -eq $status) {
    Write-Host "    Не удалось получить статус проверки - создайте администратора вручную:" -ForegroundColor Yellow
    Write-Host "    cd $InstallDir; .\nexus.exe create-admin --username admin" -ForegroundColor Yellow
}

# 7. Служба. nexus.exe реализует SCM dispatcher сам (windows-service):
# binPath содержит `serve --service`, поэтому Start/Stop/Restart-Service
# работают без NSSM/WinSW и без ошибки 1053.
if (-not $SkipService) {
    Write-Step "Регистрация службы Windows"
    $exePath = Join-Path $InstallDir 'nexus.exe'
    $account = Resolve-NexusServiceAccount -Type $ServiceAccountType -Name $ServiceAccount -Service $ServiceName
    $isCustom = $ServiceAccountType -eq 'Custom'
    if ($isCustom -and -not $ServicePassword) {
        throw "Для -ServiceAccountType Custom передайте -ServicePassword (Read-Host -AsSecureString)."
    }

    $binPath = "`"$exePath`" serve --service"
    $existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    $created = $false
    if ($existing) {
        Write-Host "    Служба уже зарегистрирована - останавливаю для обновления."
        Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
        & sc.exe config $ServiceName binPath= $binPath start= auto | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "sc.exe config binPath завершился с кодом $LASTEXITCODE" }
    } elseif ($isCustom) {
        $credential = New-Object System.Management.Automation.PSCredential($account, $ServicePassword)
        New-Service -Name $ServiceName -BinaryPathName $binPath `
            -DisplayName "NEXUS WORKSPACE" `
            -Description "NEXUS WORKSPACE - сборки Minecraft и облачное хранилище" `
            -StartupType Automatic -Credential $credential | Out-Null
        $created = $true
    } else {
        New-Service -Name $ServiceName -BinaryPathName $binPath `
            -DisplayName "NEXUS WORKSPACE" `
            -Description "NEXUS WORKSPACE - сборки Minecraft и облачное хранилище" `
            -StartupType Automatic | Out-Null
        $created = $true
    }

    # Учётная запись. Для LocalSystem пароль не нужен; виртуальная учётная
    # запись NT SERVICE\<имя> создаётся SCM и тоже работает без пароля.
    $registered = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
    $currentAccount = $registered.StartName
    $accountChanged = -not $currentAccount -or ($currentAccount -ine $account)
    if ($accountChanged) {
        if ($isCustom) {
            if ($created) {
                # New-Service -Credential уже задал учётные данные.
            } else {
                if (-not $ServicePassword) {
                    throw "Смена учётной записи службы на $account требует -ServicePassword."
                }
                $plain = ConvertTo-PlainText $ServicePassword
                try {
                    & sc.exe config $ServiceName obj= $account password= $plain | Out-Null
                } finally {
                    $plain = $null
                    [GC]::Collect()
                }
                if ($LASTEXITCODE -ne 0) { throw "sc.exe config obj/password завершился с кодом $LASTEXITCODE" }
            }
        } else {
            & sc.exe config $ServiceName obj= $account | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "sc.exe config obj завершился с кодом $LASTEXITCODE" }
        }
    }

    Set-Service -Name $ServiceName -StartupType Automatic
    $regPath = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
    # NEXUS_SERVICE_NAME должен совпадать с именем службы: SCM сопоставляет
    # зарегистрированное имя с SERVICE_TABLE_ENTRY в диспетчере.
    # NEXUS_SERVICE_STOP_TIMEOUT_MS — фактический WaitToKillServiceTimeout:
    # после него SCM убивает процесс, поэтому graceful shutdown короче.
    $scmTimeoutMs = 5000
    $control = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control' `
        -Name WaitToKillServiceTimeout -ErrorAction SilentlyContinue
    if ($control -and $control.WaitToKillServiceTimeout) {
        $scmTimeoutMs = [int]$control.WaitToKillServiceTimeout
    }
    $serviceEnv = @(
        "NEXUS_SERVICE_NAME=$ServiceName",
        "NEXUS_SERVICE_STOP_TIMEOUT_MS=$scmTimeoutMs"
    )
    New-ItemProperty -Path $regPath -Name "Environment" -Value $serviceEnv `
        -PropertyType MultiString -Force | Out-Null
    Write-Host "    Таймаут остановки SCM: $scmTimeoutMs мс" -ForegroundColor DarkGray
    New-ItemProperty -Path $regPath -Name "AppDirectory" -Value $InstallDir `
        -PropertyType String -Force | Out-Null

    # Права: read/execute на установку, modify на данные и журналы.
    if ($ServiceAccountType -ne 'LocalSystem') {
        Grant-NexusPathAccess -Path $InstallDir -Account $account -Rights 'RX'
        Grant-NexusPathAccess -Path $dataDir -Account $account -Rights 'M'
        Grant-NexusPathAccess -Path $logDir -Account $account -Rights 'M'
        Grant-NexusPathAccess -Path $envPath -Account $account -Rights 'R' -File
        foreach ($storage in $StoragePaths) {
            if (-not (Test-Path -LiteralPath $storage)) {
                throw "Каталог хранения не найден: $storage"
            }
            Grant-NexusPathAccess -Path $storage -Account $account -Rights 'M'
        }
    }

    # Recovery: три попытки с нарастающей паузой, счётчик сбрасывается раз в
    # сутки. failureflag=1 учитывает и аварийный выход без «краша».
    & sc.exe failure $ServiceName reset= 86400 `
        actions= restart/5000/restart/15000/restart/60000 | Out-Null
    & sc.exe failureflag $ServiceName 1 | Out-Null

    # Автозапуск после перезагрузки: если PostgreSQL локальный, служба должна
    # стартовать после него, иначе первый connect_and_migrate упадёт.
    $dependency = $PostgresService
    if (-not $dependency -and $env:DATABASE_URL -match '@(127\.0\.0\.1|localhost)([:/]|$)') {
        $localPg = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue |
            Sort-Object Status -Descending | Select-Object -First 1
        if ($localPg) { $dependency = $localPg.Name }
    }
    if ($dependency) {
        $pg = Get-Service -Name $dependency -ErrorAction SilentlyContinue
        if (-not $pg) { throw "Служба PostgreSQL не найдена: $dependency" }
        & sc.exe config $ServiceName depend= $dependency | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "sc.exe config depend завершился с кодом $LASTEXITCODE" }
        Write-Host "    Зависимость: $dependency" -ForegroundColor DarkGray
    }

    Write-Step "Запуск службы"
    try {
        Start-Service -Name $ServiceName
        (Get-Service -Name $ServiceName).WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Running,
            [TimeSpan]::FromSeconds(30))
    } catch {
        Write-Host "    Start-Service: $($_.Exception.Message)" -ForegroundColor Red
        Write-Host "    Последние события Service Control Manager:" -ForegroundColor Yellow
        Get-WinEvent -FilterHashtable @{
            LogName = 'System'; ProviderName = 'Service Control Manager'
            StartTime = (Get-Date).AddMinutes(-5)
        } -MaxEvents 10 -ErrorAction SilentlyContinue |
            Select-Object TimeCreated, Id, LevelDisplayName, Message | Format-List | Out-Host
        throw "Служба $ServiceName не запустилась."
    }
    $status = (Get-Service -Name $ServiceName).Status
    Write-Host "    Служба ${ServiceName}: $status ($account)" -ForegroundColor Green

    if ($ConfigureFirewall) {
        Write-Step "Правила Windows Firewall"
        & (Join-Path $PSScriptRoot 'firewall-windows.ps1') -HttpPort $HttpPort -HttpsPort $HttpsPort
    }

    # J1: подтверждаем, что SCM dispatcher отвечает на все три команды.
    if ($TestServiceControl) {
        Write-Step "Проверка Start/Stop/Restart-Service"
        $watch = [Diagnostics.Stopwatch]::StartNew()
        Restart-Service -Name $ServiceName -Force
        (Get-Service -Name $ServiceName).WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Running,
            [TimeSpan]::FromSeconds(30))
        $restartMs = $watch.ElapsedMilliseconds
        Write-Host "    Restart-Service: $restartMs мс"

        $watch.Restart()
        Stop-NexusService -Name $ServiceName -TimeoutSeconds 40
        $stopMs = $watch.ElapsedMilliseconds
        Write-Host "    Stop-Service:    $stopMs мс (ожидание до 40 с)"

        $watch.Restart()
        Start-Service -Name $ServiceName
        (Get-Service -Name $ServiceName).WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Running,
            [TimeSpan]::FromSeconds(30))
        Write-Host "    Start-Service:   $($watch.ElapsedMilliseconds) мс"
    }
}

Write-Step "Готово"
Write-Host "  Каталог:   $InstallDir"
Write-Host "  Журналы:   $logDir (файлы nexus.log.<дата>, хранится NEXUS_LOG_KEEP)"
Write-Host "  Проверка:  cd $InstallDir; .\nexus.exe check"
Write-Host "  Служба:    Get-Service $ServiceName"
Write-Host ""
Write-Host "  Дальше: настройте обратный прокси и HTTPS - см. docs/DEPLOYMENT.md." -ForegroundColor Yellow
Write-Host "  Затем войдите в панель и зарегистрируйте каталог хранения в /system/storage." -ForegroundColor Yellow
