# Развёртывание NEXUS WORKSPACE

Документ описывает production-развёртывание NEXUS WORKSPACE: требования,
PostgreSQL, конфигурацию, службу Windows, обратный прокси, HTTPS, WebSocket,
firewall, обновление с откатом, резервное копирование, проверку `nexus check`
и типовые проблемы.

Проверенные команды и фактические результаты прогонов фиксируются в
[`TESTING.md`](TESTING.md). **Целевая и единственная платформа — Windows 11
(x64)**: установка, служба, HTTPS, WebSocket, Range, backup/restore и update
выполняются на Windows. 25-ГБ сценарий — задача G2.

---

## 1. Требования

| Компонент | Минимум | Примечание |
|---|---|---|
| ОС | **Windows 11 / Windows Server 2019+** | единственная целевая платформа |
| Rust | 1.85+ | edition 2024; нужен для сборки из исходников |
| Node.js | 20+ | сборка frontend |
| PostgreSQL | 14+ | проверено на 17 |
| RAM | 1 ГБ + запас под PostgreSQL и кэш | служба ограничивает процесс 2 ГБ |
| Диск | под БД, журналы и тома хранения | резерв 10–15% |
| Утилиты | `pg_dump`, `psql`, `openssl` | `openssl` — для генерации секретов |
| Длинные пути | `LongPathsEnabled=1` (рекомендуется) | установщик проверяет реестр и предупреждает; `-RequireLongPaths` делает проверку обязательной |
| Порты | 80/443 на прокси; 8080 — только localhost | PostgreSQL 5432 — только localhost/приватная сеть |

Не запускайте две копии NEXUS на одном порту или против одной базы: миграции и
фоновые задачи не рассчитаны на несколько активных экземпляров.

### 1.1. Топология

```text
Интернет
   │  HTTPS 443
   ▼
Caddy  ──►  127.0.0.1:8080  (nexus serve)
                          │
                          ├── PostgreSQL 127.0.0.1:5432
                          └── тома хранения (локальные диски)
```

Наружу открываются только 80/443. Порт 8080 и PostgreSQL не должны быть
доступны из интернета. На целевой Windows 11 основной прокси — Caddy for Windows (§6.1).

### 1.2. Требования Windows 11: имена, длинные пути, backup и Defender

**Длинные пути.** Проверьте реестр до установки:

```powershell
(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
  -Name LongPathsEnabled -ErrorAction SilentlyContinue).LongPathsEnabled
```

Ожидаемо `1`. Если значение `0` или отсутствует:

```powershell
Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
  -Name LongPathsEnabled -Value 1
# Требуется перезагрузка хоста.
```

`install-windows.ps1` проверяет этот ключ: по умолчанию печатает
предупреждение, с `-RequireLongPaths` — завершает установку ошибкой.
NEXUS ограничивает виртуальные пути 240 UTF-16 единицами, поэтому приложение
работает и без `LongPathsEnabled`, но backup/restore и внешние инструменты
над длинными деревьями требуют поддержки ОС.

**Имена и длина путей.** Это политика приложения, а не настройка:

* Unicode NFC; сравнение в одной папке без учёта регистра → `409
  FILE_NAME_CONFLICT`;
* виртуальный путь ≤ 240 UTF-16, имя ≤ 128; длиннее → `422 PATH_TOO_LONG`
  без обрезания;
* запрещены `CON`/`NUL`/`COM1…`, ADS (`:`), `..`, абсолютные/UNC-пути,
  завершающие точка и пробел;
* физические объекты хранятся по коротким ключам, поэтому лимит не влияет на
  путь в `.nexus\objects`.

**Backup залоченных файлов.** `backup-objects.ps1` по умолчанию снимает
VSS-снимок тома, поэтому открытые staging-файлы и объекты копируются
согласованно; при недоступности VSS служба останавливается. Режим `Live`
(`robocopy`) допустим только как аварийный и не гарантирует согласованность —
подробности и таблица режимов в §9.3.

**Defender.** Исключение (`Add-MpPreference -ExclusionPath`) рекомендуется
только для выделенного тома хранения и только при доверенных пользователях и
внешней проверке на периметре; каталог установки, БД, журналы и системный
диск не исключаются. Правила и риск — §4.5, права после restore — §9.5.

---

## 2. PostgreSQL

Создайте отдельную роль и базу. Пользователь не должен быть суперпользователем.

```sql
create role nexus login password '<пароль>';
create database nexus owner nexus;
```

Проверка подключения:

```bash
psql "postgres://nexus:<пароль>@127.0.0.1:5432/nexus" -c "select version();"
```

Рекомендации:

* `listen_addresses = 'localhost'` (или приватный интерфейс) в
  `postgresql.conf`;
* доступ только с localhost/приватной сети в `pg_hba.conf`;
* отдельный пароль, не совпадающий с `NEXUS_SECRET`;
* регулярный `pg_dump` и проверка восстановления;
* запас соединений: `max_connections` в `nexus.toml` по умолчанию 20.

Миграции применяются командой `nexus migrate` и автоматически при старте
`nexus serve`.

---

## 3. Конфигурация

### 3.1. Приоритет источников

```text
переменные окружения  →  nexus.toml  →  значения по умолчанию
```

`nexus.toml` читается из текущего каталога или из пути `NEXUS_CONFIG`.
`.env` загружается из текущего каталога, а если его нет — из каталога рядом с
исполняемым файлом (важно для службы Windows, которая стартует из `System32`).
После загрузки `.env` из каталога установки рабочий каталог переключается туда,
поэтому относительные пути статики и журналов считаются от установки.

### 3.2. Обязательные значения

| Переменная | Назначение |
|---|---|
| `DATABASE_URL` | строка подключения PostgreSQL |
| `NEXUS_SECRET` | секрет подписи CSRF, **не менее 32 символов**; генерируется `openssl rand -hex 32` |

Без `NEXUS_SECRET` сервер создаст временный секрет: после перезапуска старые
CSRF-токены станут недействительны. `nexus check` считает короткий секрет
ошибкой.

### 3.3. Основные переменные

| Переменная | По умолчанию | Назначение |
|---|---|---|
| `NEXUS_HOST` | `127.0.0.1` | адрес прослушивания |
| `NEXUS_PORT` | `8080` | порт |
| `NEXUS_PUBLIC_URL` | `http://localhost:8080` | публичный URL для ссылок |
| `NEXUS_COOKIE_SECURE` | `false` | `true` при HTTPS |
| `NEXUS_STATIC_DIR` | `../frontend/dist` | собранный frontend |
| `NEXUS_TRUST_PROXY` | `false` | доверять `X-Forwarded-For` / `X-Real-IP` только за прокси |
| `NEXUS_CORS_ORIGINS` | пусто | список origin через запятую; пусто — same-origin |
| `NEXUS_SESSION_TTL_HOURS` | `24` | срок сессии |
| `NEXUS_PASSWORD_MIN_LENGTH` | `12` | минимальная длина пароля для CLI (8–64) |
| `NEXUS_MAX_UPLOAD_MB` | `10240` | максимальный размер файла |
| `NEXUS_UPLOAD_TTL_HOURS` | `24` | TTL незавершённой загрузки |
| `NEXUS_EXPORT_TTL_HOURS` | `6` | TTL ZIP-архива массового скачивания |
| `NEXUS_LAUNCHER_RPM` | `120` | запросов каталога лаунчера в минуту на токен |
| `NEXUS_LAUNCHER_ARTIFACTS_RPM` | `600` | скачиваний артефактов в минуту на токен |
| `NEXUS_LOG_DIR` | пусто | каталог файлового журнала; пусто — только stderr |
| `NEXUS_LOG_FILE` | `nexus.log` | имя журнала |
| `NEXUS_LOG_KEEP` | `7` | число архивов при ротации |
| `NEXUS_ENABLE_DOCS` | `false` | Swagger UI `/api/v1/docs` |
| `NEXUS_CONFIG` | `nexus.toml` | путь к TOML |
| `RUST_LOG` | `nexus=info,tower_http=info,sqlx=warn` | уровни `tracing` |

### 3.4. `nexus.toml`

Секции: `[server]`, `[database]`, `[security]`, `[storage]`, `[launcher]`,
`[logging]`.
Production-пример — [`deployment/nexus.production.toml`](../deployment/nexus.production.toml).

```toml
[server]
host = "127.0.0.1"
port = 8080
public_url = "https://nexus.example.com"
static_dir = "../frontend/dist"
trust_proxy = true

[database]
url = "postgres://nexus:CHANGE_ME@127.0.0.1:5432/nexus"
max_connections = 20

[security]
cookie_secure = true
session_ttl_hours = 24
password_min_length = 12
allowed_origins = []

[storage]
max_upload_bytes = 10737418240
chunk_max_bytes = 67108864
upload_ttl_hours = 24
share_default_hours = 168
export_ttl_hours = 6

[launcher]
requests_per_minute = 120
artifacts_per_minute = 600
max_parallel_downloads = 8

[logging]
dir = "./logs"
file = "nexus.log"
keep_files = 7
```

Проверки при загрузке: `max_upload_bytes` ≥ 1 МБ, `chunk_max_bytes` ≤ 256 МБ,
`upload_ttl_hours` и `session_ttl_hours` — 1…720, `password_min_length` — 8…64,
`requests_per_minute` и `artifacts_per_minute` — 1…1 000 000,
`max_parallel_downloads` — 1…1024, `NEXUS_SECRET` ≥ 32 символов.
`NEXUS_LAUNCHER_RPM` и `NEXUS_LAUNCHER_ARTIFACTS_RPM` переопределяют
`[launcher].requests_per_minute` и `artifacts_per_minute`.

### 3.4.1. Переменные окружения службы Windows

Служба Windows получает переменные среды **уровня машины** и загружает `.env`
из каталога установки (или рядом с `nexus.exe`). Переменные, заданные через
`setx` в интерактивной сессии пользователя, в службу **не попадают** — она
работает под системной учётной записью и не наследует профиль администратора.
Для лимитов лаунчера используйте один из вариантов:

* строки `NEXUS_LAUNCHER_RPM` / `NEXUS_LAUNCHER_ARTIFACTS_RPM` в
  `<InstallDir>\.env` (рекомендуется);
* переменные уровня машины:
  `[Environment]::SetEnvironmentVariable('NEXUS_LAUNCHER_RPM','120','Machine')`,
  затем перезапуск службы;
* секцию `[launcher]` в `<InstallDir>\nexus.toml`.

Эффективные значения видны в журнале при старте (`лимиты Launcher API`) и в
`nexus.exe check` (строка «лимиты лаунчера»).

### 3.5. Секреты

* `.env` и `nexus.toml` с паролями не коммитятся;
* права на `.env`: `0640`, владелец — учётная запись службы;
* не передавайте пароль PostgreSQL аргументом командной строки: он виден в
  списке процессов;
* после смены `NEXUS_SECRET` все сессии и CSRF-токены становятся
  недействительными — это ожидаемое поведение.

---

## 4. Windows

### 4.1. Установка

Запустите PowerShell **от имени администратора** из корня репозитория:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\install-windows.ps1 `
  -Domain nexus.example.com `
  -DatabaseUrl "postgres://nexus:<пароль>@127.0.0.1:5432/nexus"
```

Параметры:

| Параметр | Назначение |
|---|---|
| `-InstallDir` | каталог установки, по умолчанию `C:\NexusWorkspace` |
| `-ServiceName` | имя службы, по умолчанию `NexusWorkspace` |
| `-ServiceAccountType` | `Virtual` (по умолчанию, `NT SERVICE\<ServiceName>`), `LocalSystem`, `LocalService`, `NetworkService`, `Custom` |
| `-ServiceAccount` | учётная запись для `Custom` (`DOMAIN\user` или `.\user`) |
| `-ServicePassword` | пароль для `Custom`: `(Read-Host -AsSecureString)` |
| `-StoragePaths` | дополнительные каталоги хранения вне `InstallDir` (например, `D:\NexusStorage`) — выдаётся Modify |
| `-TestServiceControl` | после установки выполнить Restart/Stop/Start-Service и вывести время |
| `-SkipPortCheck` | не проверять порты 8080/80/443 перед установкой |
| `-HttpPort` / `-HttpsPort` | порты прокси для проверки и firewall (по умолчанию 80/443) |
| `-ConfigureFirewall` | открыть 80/443 в Windows Firewall |
| `-PostgresService` | служба PostgreSQL для зависимости автозапуска; пусто — локальный `postgresql*` по `DATABASE_URL` |
| `-Domain` | домен; включает `https://` и `NEXUS_COOKIE_SECURE=true` |
| `-DatabaseUrl` | строка подключения; без неё в `.env` попадает `CHANGE_ME` |
| `-Secret` | готовый секрет; иначе генерируется случайный |
| `-AdminPassword` | пароль первого администратора; иначе запрос интерактивно |
| `-SkipAdmin` | не создавать администратора |
| `-SkipService` | не регистрировать службу |
| `-SkipBuild` | использовать уже собранные артефакты |

Скрипт:

1. собирает backend (`cargo build --release`) и frontend (`npm run build`);
2. создаёт `C:\NexusWorkspace\{logs,data,frontend}`;
3. копирует `nexus.exe`, `nexus.toml`, `.env.example` и статику;
4. создаёт `.env` (существующий **не перезаписывает**), задаёт пути в прямых
   слэшах;
5. проверяет порты (`check-ports.ps1`): 8080 свободен, 80/443 не в
   исключённых диапазонах; применяет миграции и выполняет `nexus check`;
6. создаёт администратора `admin`, если администраторов ещё нет;
7. регистрирует службу `NexusWorkspace` с binPath
   `"...\nexus.exe" serve --service`, настраивает учётную запись, права на
   `data`/`logs` (и `-StoragePaths`), автозапуск и политику перезапуска,
   запускает её.

Служба работает через встроенный SCM dispatcher (`windows-service` в
`nexus.exe`), а не через NSSM/WinSW. Поэтому `Start-Service`,
`Stop-Service`, `Restart-Service` и `sc.exe query` не дают ошибку 1053.
`sc.exe qc` показывает `WIN32_OWN_PROCESS` и `AUTO_START`.

Учётная запись по умолчанию — виртуальная `NT SERVICE\NexusWorkspace`: без
пароля, без интерактивного входа, с доступом только к каталогам установки.
`LocalSystem` проще для пилота, но даёт службе полные права на машину.
Для `Custom` заранее выдайте учётной записи право «Вход в качестве службы»
(`secpol.msc` → Local Policies → User Rights Assignment) и передайте пароль
через `-ServicePassword`; при смене учётной записи существующей службы
пароль обязателен.

Скрипт останавливает уже зарегистрированную службу **до** копирования
`nexus.exe` (иначе файл заблокирован) и ждёт `Stopped` до 40 с.

Если `DATABASE_URL` указывает на `127.0.0.1`/`localhost`, установщик добавляет
зависимость службы от локальной `postgresql*`: после перезагрузки SCM сначала
запускает PostgreSQL, затем NEXUS. Для удалённой БД зависимость не создаётся;
явно задать её можно через `-PostgresService`.

Приёмка после реальной перезагрузки Windows 11 (служба `Automatic`, задача
`running`, публичная ссылка и resume-загрузка) — **PASS 20.11.2026**:
[`reports/reboot-20261120-j4.md`](reports/reboot-20261120-j4.md). На стенде
после холодного старта PostgreSQL был короткий период `database: down`,
который прошёл без ручного вмешательства; см. примечание в отчёте.

⚠ Скрипт записывает `NEXUS_TRUST_PROXY=true` независимо от наличия прокси.
Если вы открываете `:8080` напрямую, исправьте значение на `false` и
перезапустите службу, иначе клиент сможет подделать IP в заголовке
`X-Forwarded-For`.

⚠ Если не передан `-DatabaseUrl`, миграции с `CHANGE_ME` не пройдут. Исправьте
`C:\NexusWorkspace\.env` и запустите установку повторно (`.env` сохранится) или
выполните `nexus migrate` вручную.

### 4.2. Управление службой

```powershell
Get-Service NexusWorkspace
Start-Service NexusWorkspace
Stop-Service NexusWorkspace
Restart-Service NexusWorkspace
sc.exe qc NexusWorkspace
```

Служба запускает `"C:\NexusWorkspace\nexus.exe" serve --service`. SCM стартует
процесс из `System32`, поэтому `nexus.exe` сам ищет `.env` и `nexus.toml`
рядом с исполняемым файлом и переходит в каталог установки; `AppDirectory`
записан в реестр для совместимости. Имя службы передаётся процессу переменной
среды службы `NEXUS_SERVICE_NAME` (реестр, `REG_MULTI_SZ`): оно должно
совпадать с именем, которое зарегистрировано в SCM.

```powershell
sc.exe qc NexusWorkspace
sc.exe queryex NexusWorkspace
sc.exe qfailure NexusWorkspace
```

Политика восстановления (задаётся установщиком): три перезапуска через 5, 15 и
60 секунд, сброс счётчика раз в сутки; `failureflag=1` учитывает и аварийный
выход без «краша».

**Остановка.** `Stop-Service` отправляет `SERVICE_CONTROL_STOP`, а не Ctrl+C.
Диспетчер переводит его в тот же graceful shutdown, что и Ctrl+C: сервер
перестаёт принимать запросы, планировщик помечает текущие задачи
`cancel_requested`, пул БД закрывается. Служба сообщает SCM `StopPending` с
wait hint, равным `WaitToKillServiceTimeout`; скрипты ждут `Stopped` до 40 с.

Установщик читает `HKLM\SYSTEM\CurrentControlSet\Control\WaitToKillServiceTimeout`
(на стенде **5000 мс**) и передаёт его службе в
`NEXUS_SERVICE_STOP_TIMEOUT_MS`. Внутреннее ожидание задач — на 1 с меньше
(4000 мс), чтобы SCM не пришлось убивать процесс. Фактический `Stop-Service`
на стенде — **270–274 мс**. Если graceful shutdown всё же не уложился, SCM
завершает процесс принудительно; следующий старт возвращает `running`-задачи
в `pending` (C2). Это проверено и для `Stop-Service`, и для
`Stop-Process -Force`: отчёт
[`reports/service-stop-20261120-j1j2.md`](reports/service-stop-20261120-j1j2.md).

Журналы: `C:\NexusWorkspace\logs\nexus.log.<ГГГГ-ММ-ДД>` — файл на сутки,
хранится `NEXUS_LOG_KEEP` последних файлов (по умолчанию 7). Запись
неблокирующая; буфер сбрасывается при остановке службы. `stderr` службе не
подключён, поэтому файл — основной источник диагностики.

### 4.3. Проверка

```powershell
cd C:\NexusWorkspace
.\nexus.exe check
.\nexus.exe check --json
Invoke-WebRequest http://127.0.0.1:8080/api/v1/health
```

### 4.4. Порты, HTTP.sys и firewall

Перед установкой проверьте порты отдельным скриптом (установщик делает это
автоматически, `-SkipPortCheck` отключает):

```powershell
powershell -File deployment\check-ports.ps1
powershell -File deployment\check-ports.ps1 -Ports 80,443 -Json
powershell -File deployment\check-ports.ps1 -Ports 8080 -RequireFree
```

Скрипт показывает: кто слушает порт (PID/процесс, признак `System (HTTP.sys)`
и IIS), исключённые диапазоны TCP Hyper-V/WSL, URL ACL HTTP.sys и результат
реального `bind`. Критичные случаи — порт внутри исключённого диапазона или
`bind AccessDenied`; занятость 80/443 процессом Caddy — норма.

Проверить исключённые диапазоны вручную:

```powershell
netsh interface ipv4 show excludedportrange protocol=tcp
netsh interface ipv6 show excludedportrange protocol=tcp
```

На Windows 11 Hyper-V/WSL и другие компоненты резервируют диапазоны динамически.
Порт из такого диапазона не биндится даже администратором (часто
`WSAEACCES`/`AccessDenied`, реже — «адрес уже используется»). Если 80/443 попали
в диапазон, измените диапазон Hyper-V (`netsh int ipv4 set dynamicport tcp
start=... num=...`) или остановите компонент; простое освобождение порта не
помогает. Активные URL HTTP.sys:

```powershell
netsh http show servicestate view=session
netsh http show urlacl
```

Если 80/443 занял `System` (PID 4), это HTTP.sys/IIS/WinRM и подобные; сначала
остановите соответствующий сайт/службу. `netsh http show urlacl` сам по себе
не блокирует обычный TCP-bind, но для `HttpListener` нужен отдельный URL ACL.

Firewall открывается скриптом (установщик вызывает его с `-ConfigureFirewall`):

```powershell
# Caddy на этой же машине: входящие 80/443.
powershell -File deployment\firewall-windows.ps1

# Backend за удалённым прокси: 8080 только с адреса прокси.
powershell -File deployment\firewall-windows.ps1 -ExposeBackend `
  -ProxyAddress 10.0.0.10

# Удалить правила.
powershell -File deployment\firewall-windows.ps1 -Remove
```

Правила идемпотентны (создаются заново с актуальными портами). При локальном
прокси входящий 8080 не нужен. PostgreSQL 5432 не должен быть доступен снаружи.

Фактические результаты стенда (Windows 11: 80/443/8080 свободны, исключён
только managed-диапазон `13508` для NVIDIA HTTP.sys):
[`reports/ports-firewall-20261120-j3.md`](reports/ports-firewall-20261120-j3.md).

### 4.5. Windows Defender и каталоги хранения

Defender с защитой в реальном времени сканирует staging-файлы сразу после
записи. NEXUS повторяет чтение staging-файла и его перемещение в объектное
хранилище при `ERROR_SHARING_VIOLATION`/`ERROR_LOCK_VIOLATION` и
`PermissionDenied`, поэтому кратковременная блокировка не превращается в
ошибку завершения загрузки.

Если Defender **удалил** загруженный файл (ложное срабатывание), завершение
вернёт `UPLOAD_DATA_MISSING`; уже зафиксированный объект может стать
недоступен. Для выделенных томов хранения можно добавить исключение:

```powershell
# Только выделенный том NEXUS, не системный диск и не каталог установки.
Add-MpPreference -ExclusionPath 'D:\NexusStorage'
Get-MpPreference | Select-Object -ExpandProperty ExclusionPath
```

Снять исключение:

```powershell
Remove-MpPreference -ExclusionPath 'D:\NexusStorage'
```

⚠ **Риск:** исключённые каталоги не сканируются. Вредоносный файл,
загруженный пользователем, не будет обнаружен Defender. Добавляйте исключение
только если том выделен под NEXUS, пользователи доверенные, а на периметре
есть внешняя антивирусная проверка. Не исключайте `C:\`, каталог установки,
`frontend/dist`, БД и журналы. Не отключайте защиту в реальном времени
глобально (`Set-MpPreference -DisableRealtimeMonitoring $true`).

При массовых ложных срабатываниях предпочтительнее отправить файл в Microsoft
как false positive и/или подключить внешний сканер до фиксации объекта, а не
расширять исключения на весь диск.

Резервное копирование объектов обходит блокировки через VSS, поэтому
исключение тома для backup обычно не требуется; ограничения и исключения для
каталога копий описаны в §9.5.

---

## 5. Платформа поставки

Проект поставляется, устанавливается и принимается **только на Windows 11 /
Windows Server**. Поддержка Linux (systemd, shell-скрипты, отдельные unit-файлы)
в проект не входит; соответствующие файлы и инструкции удалены.

Все команды ниже приведены для Windows PowerShell и службы `NexusWorkspace`.

---

## 6. Обратный прокси и HTTPS

Сервер слушает `127.0.0.1:8080`; прокси терминирует TLS и передаёт реальный IP.

### 6.1. Caddy for Windows

Caddy запускается как служба через NSSM (или WinSW). Шаблон —
[`deployment/Caddyfile.windows`](../deployment/Caddyfile.windows): внутренний CA
для локального стенда, HTTP→HTTPS, HSTS, `request_body` 11 ГиБ, таймауты
транспорта 30 минут, перезапись `X-Forwarded-For`.

```powershell
nssm install Caddy C:\Caddy\caddy.exe
nssm set Caddy AppParameters "run --config C:\Caddy\Caddyfile --adapter caddyfile"
nssm set Caddy AppDirectory C:\Caddy
nssm set Caddy AppEnvironmentExtra "XDG_DATA_HOME=C:\Caddy\data" "XDG_CONFIG_HOME=C:\Caddy\config"
nssm set Caddy Start SERVICE_AUTO_START
nssm start Caddy
certutil -addstore -f Root C:\Caddy\data\pki\authorities\local\root.crt
```

Публичный домен — Let's Encrypt (нужны DNS и открытые 80/443), локальный
стенд — внутренний CA (`local_certs` + `certutil`; `caddy trust` из-под
LocalSystem на стенде вернул «request is not supported»). Проверка HTTPS,
WebSocket, Range и реального IP в аудите — отчёт
[`reports/proxy-https-20261120-j2.md`](reports/proxy-https-20261120-j2.md).
Для `NEXUS_TRUST_PROXY=true` Caddy должен быть единственным входом; при прямом
доступе к `:8080` оставьте `false`.

### 6.2. HTTPS и cookie

При HTTPS обязательно:

```dotenv
NEXUS_PUBLIC_URL=https://nexus.example.com
NEXUS_COOKIE_SECURE=true
NEXUS_TRUST_PROXY=true
```

`trust_proxy=true` допустим **только** за доверенным прокси. При прямом
доступе к `:8080` оставьте `false`.

### 6.3. WebSocket

WebSocket `/api/v1/ws` требует cookie-сессию и поддерживает `Upgrade`.
Проверьте, что прокси не буферизует соединение и не обрывает его по таймауту.
Caddy проксирует `Upgrade`/`Connection` автоматически.

---

## 7. Первый запуск и проверка

1. Примените миграции: `nexus migrate`.
2. Создайте администратора: `nexus create-admin --username admin`.
3. Выполните `nexus check`.
4. Запустите службу.
5. Откройте публичный URL и войдите.
6. Зарегистрируйте каталог хранения в **Система → Хранилища** (без тома
   загрузка вернёт `NO_VOLUME_AVAILABLE`).
7. Проверьте `/api/v1/health` и загрузите тестовый файл.

Пример `nexus check` (Windows-установка):

```text
NEXUS WORKSPACE 1.0.0 — проверка конфигурации
  база данных: подключение успешно (PostgreSQL 17.12)
  миграции: 14
  тома хранения: 1
  администраторы: 1
  статика: C:\NexusWorkspace\frontend
  каталог логов: C:\NexusWorkspace\logs
Проверка пройдена: сервер готов к запуску.
```

`nexus check` проверяет:

* длину и «заводское» значение `NEXUS_SECRET`;
* согласованность `https` в `public_url` и `NEXUS_COOKIE_SECURE`;
* подключение к PostgreSQL и наличие миграций;
* хотя бы одного администратора;
* наличие `frontend/dist/index.html`;
* доступность каталога журналов.

Код возврата `0` — проблем нет, `1` — есть. Отсутствие томов — предупреждение,
а не ошибка. JSON-вывод (`--json`) содержит `ok`, `admin_present`, `info`,
`warnings`, `problems`.

**`/api/v1/health`** возвращает `200` и `"status": "degraded"` при недоступной
БД. Внешний мониторинг должен проверять тело ответа, а не только код.

---

## 8. Обновление и откат

### 8.1. Штатные скрипты

Windows (администратор):

```powershell
powershell -ExecutionPolicy Bypass -File deployment\update-windows.ps1
```

Порядок:

1. резервная копия БД, `data` и конфигурации;
2. сборка backend и frontend;
3. остановка службы;
4. сохранение предыдущего бинарника (`nexus.previous.exe`);
5. замена бинарника, статики и `docs/`;
6. `nexus migrate` и `nexus check`;
7. при ошибке миграции — возврат предыдущего бинарника и запуск службы;
8. запуск службы и вывод статуса.

### 8.2. Ограничения отката

⚠ Автоматический откат возвращает только **бинарник**. Миграции forward-only и
не откатываются. Если новая версия изменила схему и её нужно отменить,
восстанавливайте БД из копии, снятой перед обновлением.

⚠ Скрипты не копируют тома хранения вне `InstallDir\data`. Их нужно сохранить
отдельно до обновления.

### 8.3. Ручное обновление

1. Снимите резервную копию.
2. Остановите службу.
3. Замените `nexus`/`nexus.exe` и содержимое `frontend`.
4. Выполните `nexus migrate`.
5. Выполните `nexus check`.
6. Запустите службу.
7. Проверьте вход, загрузку/скачивание, список сборок и журнал.

Держите предыдущий бинарник до подтверждения работоспособности.

---

## 9. Резервное копирование и восстановление

### 9.1. Что копировать

| Данные | Обязательно | Комментарий |
|---|---|---|
| Дамп PostgreSQL | да | пользователи, права, метаданные, релизы, настройки, журнал |
| Тома хранения | да | объекты `.nexus/objects` |
| `.env`, `nexus.toml` | да | секрет, БД, URL, лимиты |
| `data` | если используется | скрипты копируют автоматически |
| Логи | по желанию | для расследования |
| `nexus.exe`, `frontend/dist` | нет | восстанавливаются сборкой |

Дамп без объектов бесполезен, объекты без БД не видны приложению. Копируйте их
согласованно.

### 9.2. Скрипты

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\backup.ps1 `
  -BackupDir D:\Backups -InstallDir C:\NexusWorkspace
```

Результат: `database.dump` (custom format, `pg_restore`), `data`, `.env`,
`nexus.toml`, `manifest.json`, `SHA256SUMS`. Пароль PostgreSQL передаётся через
временный `PGPASSFILE`, служба на время дампа останавливается.

Бинарные объекты внешних томов копирует отдельный скрипт:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\backup-objects.ps1 `
  -BackupDir D:\NexusObjectBackups -Retain 7 -Prune -Force
```

Набор `objects-<stamp>` содержит `manifest.json` (SHA-256 каждого файла),
`SHA256SUMS`, `backup-result.json` и `volumes/<id>/.nexus/{objects,staging,quarantine}`.
`Full` копирует всё, `Incremental` — только новые/изменённые файлы по
размеру и времени; для restore нужна вся цепочка после полного набора.
Раз в `-FullEveryDays` дней (по умолчанию 7) автоматически создаётся новый
полный набор. Retention (`-Retain`, только с `-Prune`) не удаляет последний
`Full` и наборы после него, иначе incremental потерял бы базу. Сверка со
`storage_objects` входит в каждый запуск (`-SkipDatabaseCheck` — отключить,
`-StrictDatabase` — падать при расхождении). Тома берутся из
`storage_volumes` (только включённые) либо из `-VolumeRoot`.

### 9.3. Согласованность

Правильный порядок:

1. остановить службу;
2. снять дамп БД;
3. скопировать тома целиком, включая `.nexus`;
4. скопировать конфигурацию;
5. запустить службу.

Если остановка невозможна, используйте мгновенный снимок диска и БД;
копирование «на живую» может дать дамп, ссылающийся на ещё не записанный
объект. Скрипты этого не гарантируют.

`backup-objects.ps1` решает проблему открытых файлов так:

| `-Consistency` | Поведение | Ограничение |
|---|---|---|
| `Auto` (по умолчанию) | VSS-снимок тома; при ошибке — остановка службы | нужен администратор и NTFS; VSS crash-consistent, не application-consistent |
| `Vss` | только VSS | без VSS запуск падает, служба не останавливается |
| `Pause` | остановка службы на время копирования | простой сервиса |
| `Live` | robocopy `/R:3 /W:5` по живым файлам | открытые с несовместимым доступом файлы могут не скопироваться; согласованность не гарантирована |

VSS создаётся через `Win32_ShadowCopy.Create(..., 'ClientAccessible')`;
robocopy не читает `\\?\GLOBALROOT\Device\...` напрямую (ошибка 53), поэтому
скрипт делает временную символическую ссылку и удаляет её и снимок в
`finally`. Для staging-файлов, открытых на запись, снимок даёт согласованный
на момент создания срез; это проверено на стенде (файл был открыт с
`FileShare.None`, копия совпала по SHA-256). Junction/симлинки не обходятся:
Full — `robocopy /XJ`, Incremental — ручной обход с пропуском reparse points;
проверено на junction внутри `.nexus/objects` (внешний файл не попал в копию).

### 9.4. Восстановление

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\restore.ps1 `
  -BackupPath C:\NexusBackups\20261119-120000 `
  -TargetDatabase nexus_restore -SwitchInstall -TestUsername admin
```

`-TargetDatabase` создаёт новую БД и восстанавливает в неё; `-SwitchInstall`
переключает `.env`. Для замены активной БД добавьте `-DropExisting`. Пароль —
через `PGPASSFILE`, служба останавливается. Фактический прогон:
[`reports/backup-restore-20261120-j3.md`](reports/backup-restore-20261120-j3.md).

Объекты внешних томов восстанавливаются отдельно, до запуска службы:

```powershell
powershell -ExecutionPolicy Bypass -File deployment\restore-objects.ps1 `
  -BackupPath D:\NexusObjectBackups\objects-20261120-135451 -Force

# Цепочка full + incremental или каталог со всеми наборами
powershell -ExecutionPolicy Bypass -File deployment\restore-objects.ps1 `
  -BackupPath D:\NexusObjectBackups -Force
```

Скрипт останавливает службу, применяет наборы от старых к новым, проверяет
SHA-256 и каждый `storage_objects.state='committed'`, выдаёт права учётной
записи службы через `icacls`, запускает службу и ждёт `health=200`.
`-TargetRoot` предназначен для проверки копии на другом каталоге (только один
том). Файлы, которых нет в наборе, не удаляются.

Фактический прогон K2 (backup → удаление каталога → restore → скачивание):
[`reports/objects-backup-20261120-k2.md`](reports/objects-backup-20261120-k2.md).

Скрипты восстановления останавливают службу, восстанавливают БД и `data`,
копируют конфигурацию, выполняют `nexus check` и запускают службу. Внешние
тома они **не** восстанавливают: верните объекты в те же пути до запуска
службы.

После восстановления:

* войдите администратором и проверьте пользователей;
* откройте старый файл Cloud и скачайте релиз;
* запустите «Проверить целостность»;
* убедитесь, что в журнале нет повторяющихся `INTERNAL_ERROR`.

Путь тома после регистрации не редактируется. Если объекты восстановлены в
другой каталог, потребуется перерегистрация томов; без объектов удалить
регистрацию нельзя.

### 9.5. Права, Defender и ограничения объектов

**Права.** Копирование намеренно не переносит ACL из резервной копии:
`restore-objects.ps1` после копирования выполняет

```powershell
icacls <том> /inheritance:e /grant:r "<учётная запись службы>:(OI)(CI)M" /T
```

Учётная запись по умолчанию — `StartName` службы `NexusWorkspace`
(`NT SERVICE\NexusWorkspace`), иначе `NT AUTHORITY\SYSTEM`. `M` (Modify)
достаточно для записи, переименования и удаления объектов; наследование
распространяет права на `.nexus\objects`, `staging`, `quarantine`. Проверить:

```powershell
icacls D:\NexusStorage
Get-CimInstance Win32_Service -Filter "Name='NexusWorkspace'" |
  Select-Object Name, StartName
```

Не выдавайте `Everyone:F` и не снимайте наследование с тома. Если том
восстановлен в другой каталог, сначала перерегистрируйте его в
**Система → Хранилища**, затем проверьте права и целостность.

**Defender.** VSS-снимок обходит блокировки открытых файлов, но Defender
может удерживать staging-файл сразу после записи. Для `-Consistency Live`
скрипт повторяет копирование (`/R:3 /W:5`); для `Auto`/`Vss` снимок снимается
до чтения и блокировка не мешает. Если Defender **удалил** файл, backup
покажет `missingOnDisk`, а restore — отсутствующий объект; это реальная
потеря, а не ошибка скрипта.

Исключение для тома — только по правилам §4.5: выделенный том, доверенные
пользователи, внешняя проверка на периметре. Не исключайте каталог установки,
БД, журналы и системный диск. Каталог резервных копий можно исключить из
сканирования, если он находится на отдельном томе и доступ ограничен; это
ускоряет копирование, но снижает защиту самих копий. После изменения
исключений проверьте `Get-MpPreference`.

**Ограничения.**

* VSS требует NTFS и администратора; на FAT/exFAT и без прав автоматически
  используется пауза службы (или `Live`, если служба не запущена).
* Снимок VSS crash-consistent; БД и объекты всё равно снимайте в одном окне.
* Инкрементальный набор бесполезен без базового полного набора; restore
  проверяет `base` и отказывается восстанавливать неполную цепочку без
  `-AllowIncompleteChain`.
* Удаление старых наборов — только `-Prune` + подтверждение/`-Force`.
* `restore-objects.ps1` не удаляет лишние файлы и не перерегистрирует тома.

---

## 10. Диагностика

| Симптом | Вероятная причина | Действие |
|---|---|---|
| `check`: «NEXUS_SECRET оставлен значением по умолчанию» | секрет не заменён | задайте 32+ случайных символа |
| `check`: «не удалось подключиться к PostgreSQL» | неверный `DATABASE_URL`, БД не запущена, firewall | проверьте строку, `pg_isready`, `pg_hba.conf` |
| `check`: «администратор не создан» | нет суперпользователя | `nexus create-admin --username admin` |
| `check`: «каталог статики не найден» | frontend не собран или путь неверен | `npm run build`, проверьте `NEXUS_STATIC_DIR` |
| `check`: «тома хранения не зарегистрированы» | предупреждение | добавьте каталог в «Хранилищах» |
| 502/504 от прокси | служба не запущена, порт не совпадает | `Get-Service NexusWorkspace`, проверьте `NEXUS_PORT` и reverse_proxy |
| WebSocket не подключается | прокси не передаёт `Upgrade` | проверьте Caddyfile и `reverse_proxy` |
| `CSRF_INVALID` после перезапуска | изменился `NEXUS_SECRET` | задайте постоянный секрет; обновите страницу |
| `NO_VOLUME_AVAILABLE` | нет включённого тома `auto` с местом | добавьте/проверьте том, уменьшите резерв |
| `VOLUME_UNAVAILABLE` | диск отключён или нет прав | смонтируйте, проверьте права, «Проверить» |
| Том `offline` после перезагрузки | путь не смонтирован | смонтируйте диск и проверьте том |
| Загрузка падает на большом файле | лимит прокси или `max_upload_bytes` | увеличьте `client_max_body_size`/`request_body` и лимит сервера |
| Служба Windows не стартует | нет прав, занят порт, `.env` с `CHANGE_ME` | `Get-EventLog`, журнал в `logs`, `nexus check` |
| Загрузка: `UPLOAD_DATA_MISSING` | Defender удалил staging-файл или сессия истекла | повторите загрузку; проверьте карантин Defender; при необходимости исключите только том (§4.5) |
| Скачивание: `STORAGE_ERROR`, объект `missing` | файл объекта удалён/не восстановлен | восстановите объекты `restore-objects.ps1`, затем «Проверить целостность» |
| `backup-objects`: VSS недоступен | нет прав, FAT/exFAT, VSS-служба отключена | запустите от администратора, проверьте NTFS; используйте `-Consistency Pause` |
| `backup-objects`: robocopy код 8+ | открытый файл, права, Defender | проверьте журнал набора `logs/*-robocopy.log`; повторите с `Auto`/`Pause` |
| `restore-objects`: «цепочка неполная» | выбран только incremental | укажите полный набор и все последующие incremental или каталог целиком |
| Загрузка зависает/падает на завершении в Windows | антивирус/индексатор удерживает файл | повторите; проверьте журнал на `staging-файл занят`; рассмотрите исключение тома (§4.5) |
| Пустая история мониторинга | первый запуск или очистка метрик | подождите 10 секунд |
| Высокий RSS | большой объём метаданных/кэш | проверьте мониторинг и лимиты; не отключайте `MemoryMax` без причины |

Если проблема не описана:

1. зафиксируйте время, пользователя, действие и `request_id` из ответа API;
2. найдите событие в **Система → Журнал событий**;
3. найдите `request_id`/ошибку в журнале приложения;
4. выполните `nexus check --json`;
5. проверьте свободное место и права;
6. не удаляйте объекты и не пересоздавайте БД до снятия копии.

---

## 11. Чек-лист production

- [ ] HTTPS и `NEXUS_COOKIE_SECURE=true`;
- [ ] постоянный `NEXUS_SECRET` (32+ символа);
- [ ] PostgreSQL слушает только localhost/приватную сеть;
- [ ] порт 8080 недоступен из интернета;
- [ ] `NEXUS_TRUST_PROXY=true` только за доверенным прокси;
- [ ] зарегистрирован том хранения с резервом места;
- [ ] создан администратор и минимум одна резервная копия;
- [ ] настроен `backup-objects.ps1` (расписание, retention) и проверено
  восстановление объектов (`restore-objects.ps1`) на тестовом стенде;
- [ ] проверено восстановление БД на тестовом стенде;
- [ ] настроены журналы и контроль свободного места;
- [ ] `nexus check` возвращает 0;
- [ ] `/api/v1/health` отвечает `ok`;
- [ ] WebSocket и загрузка большого файла проверены через прокси;
- [ ] служба Windows: `Start/Stop/Restart-Service` без ошибки 1053,
  автозапуск и политика перезапуска включены (J1);
- [ ] Caddy for Windows: HTTPS, HSTS, WebSocket, Range проверены (J2);
- [ ] приёмка после перезагрузки хоста и обновления пройдена (J4, P0);
- [ ] Windows-инварианты подтверждены: регистр имён, длина пути, ADS,
  junction, sharing violation, остановка службы;
- [ ] на Windows принято решение по Defender: оставить сканирование или
  исключить только выделенные тома хранения с учётом риска (§4.5).

---

## 12. Связанные документы

* [`checklists/deploy-windows.md`](checklists/deploy-windows.md) — пошаговая
  приёмка Windows-развёртывания с ожидаемым выводом и полями «фактический
  результат».
* [`ADMIN_GUIDE.md`](ADMIN_GUIDE.md) — эксплуатация, пользователи, тома,
  резервное копирование.
* [`SECURITY.md`](SECURITY.md) — модель угроз и production-чек-лист.
* [`TESTING.md`](TESTING.md) — матрица тестов и фактические результаты.
* [`API.md`](API.md) — эндпоинты и WebSocket.
* [`../README.md`](../README.md) — быстрый старт для разработки.

---

## NEXUS AI Runtime (второй компьютер)

1. Соберите `runtime` (`cargo build --release`) и разложите файлы в
   `C:\NexusAI\`.
2. Настройте `runtime.toml`: WSS URL, `allowed_roots`, PolyStrata, программы.
3. Создайте enrollment code в **System Management → AI Management → Runtime**.
4. Выполните `nexus-ai-runtime enroll --code …`, затем
   `install-service.ps1`.
5. Проверьте `nexus-ai-runtime check --json` и heartbeat в AI Overview.

Backend при старте применяет миграцию `0019_ai.sql` и восстанавливает
прерванные AI-задачи в контролируемую ошибку. Runtime не требует входящих
портов; production — только `wss://`. См. [`AI_WORKSPACE.md`](AI_WORKSPACE.md)
и [`../runtime/README.md`](../runtime/README.md).
