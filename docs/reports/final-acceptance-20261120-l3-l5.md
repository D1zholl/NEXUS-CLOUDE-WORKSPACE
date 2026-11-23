# Финальная приёмка L3–L5

Дата проверки: 20.11.2026. Сводка построена по сохранённым прогонам, скриншотам и read-only проверкам установленной службы `NexusWorkspace`.

Статусы: **PASS** — есть прямое доказательство; **PARTIAL** — подтверждена часть сценария или есть диагностическое предупреждение; **BLOCKED** — доказательство нельзя получить в этой сессии; **N/A** — функция не настроена.

## L3. Чек-лист §7 построчно

| № | Пункт §7 | Статус | Доказательство |
|---|---|---|---|
| 1 | Сервер запускается на Rust (release) | PASS | [J5 release-артефакт](release-20261120-j5.md), [L2 версия 1.0.0](release-1.0.0-l2.md) |
| 2 | Веб-интерфейс открывается из браузера | PASS | [I1 Playwright](i1-playwright-part2-20261119.md), [L2 E2E](release-1.0.0-l2.md) |
| 3 | Страница авторизации и состояния работают | PASS | [I1 auth E2E](i1-playwright-20261119.md), [L2 E2E](release-1.0.0-l2.md) |
| 4 | Пользователи, роли, права | PASS | [I3/I4 тесты](i3-i4-tests-20261120.md), `auth_tests`, `settings_tests`, [L2](release-1.0.0-l2.md) |
| 5 | Рабочие пространства изолированы и отключаемы | PASS | I1 `access.spec.ts`, backend `modules_tests`, [L2](release-1.0.0-l2.md) |
| 6 | Главная визуально соответствует исходным референсам | BLOCKED | Файл [l3-home-reference.png](screenshots/l3-home-reference.png) — скриншот приложения, не исходный дизайн-референс. В репозитории исходных `главная_страница.png` / `форма_входа.png` нет; сравнение невозможно. |
| 7 | 3D-фон работает и настраивается | PASS | K4 в [quality-20261120-k3k6.md](quality-20261120-k3k6.md), скриншоты [главной](screenshots/l3-home-app.png) и [reduced-motion](screenshots/l3-home-app-reduced-motion.png) |
| 8 | Чистая установка Windows 11 и управление службой | PARTIAL | [Инструкция из архива](../INSTALL-WINDOWS.md), [J5 распаковка и запуск установщика](release-20261120-j5.md), [J4 reboot](reboot-20261120-j4.md). Полная установка из ZIP на отдельной чистой Windows 11 не повторялась; J5 install был с `-SkipService`. |
| 9 | Caddy for Windows: HTTPS, WebSocket, Range, trust_proxy | PASS | [J2 proxy acceptance](proxy-https-20261120-j2.md), [L2 smoke/WS](release-1.0.0-l2.md) |
| 10 | Backup/restore/update, включая откат | PASS | [J3 backup/restore/update](backup-restore-20261120-j3.md), [K2 objects backup](objects-backup-20261120-k2.md) |
| 11 | Приёмка после reboot и update | PASS | [J4 фактический reboot](reboot-20261120-j4.md), [J3 update/rollback](backup-restore-20261120-j3.md) |
| 12 | Release-артефакты и checksums | PASS | [J5](release-20261120-j5.md), актуальная перепроверка архива в разделе L4 ниже |
| 13 | Платформа поставки | N/A | Проект Windows-only; Linux исключён, поставка — Windows 11 / Windows Server. |
| 14 | Windows: регистронезависимые конфликты | PASS | `windows_invariants_tests`, [лог теста](logs/l3-windows-invariants.log), [I1 Windows E2E](i1-playwright-part2-20261119.md) |
| 15 | Windows: PATH_TOO_LONG без обрезания | PASS | `windows_invariants_tests`, [лог](logs/l3-windows-invariants.log), I1 Windows E2E |
| 16 | Windows: ADS, зарезервированные имена, завершающие символы | PASS | `windows_invariants_tests`, [лог](logs/l3-windows-invariants.log) |
| 17 | Windows: junction/symlink за корень | PASS | `windows_invariants_tests` создаёт реальные junction/symlink; [лог](logs/l3-windows-invariants.log) |
| 18 | Windows: sharing violation обрабатывается retry; нет сирот | PASS | `replacement_tests`, `storage_tests` и H2 Windows retry; [I3/I4 отчёт](i3-i4-tests-20261120.md) |
| 19 | Остановка службы: running jobs и staging; восстановление очереди | PARTIAL | [J1/J2 Stop-Service и C2](service-stop-20261120-j1j2.md): задача возвращается в очередь, orphan running = 0. Именно остановка службы посреди активного HTTP PATCH не проверена; отдельный клиентский обрыв и resume подтверждены [L2 resilience](resilience-20261120-l2.md). |
| 20 | Minecraft: создание сборки и выбор MC/загрузчика | PASS | I1 `minecraft.spec.ts`, [L2](release-1.0.0-l2.md) |
| 21 | Minecraft: загрузка модов, конфигов, текстурпаков, шейдеров | PASS | I1 Minecraft E2E, `minecraft_wizard_tests`, [L2](release-1.0.0-l2.md) |
| 22 | Minecraft: структура файлов и категории | PASS | I1 Minecraft E2E, `minecraft_tests`, [L2](release-1.0.0-l2.md) |
| 23 | Minecraft: корректный манифест | PASS | `manifest_schema_tests`, `minecraft_tests`; [L2](release-1.0.0-l2.md) |
| 24 | Minecraft: опубликованный релиз | PASS | I1 `minecraft.spec.ts`, `minecraft_tests`; [L2](release-1.0.0-l2.md) |
| 25 | Launcher: каталог, манифест и файлы | PASS | I1 `launcher.spec.ts`, `launcher_tests`; [J2](proxy-https-20261120-j2.md) |
| 26 | Minecraft: SHA-256 сервера и клиента совпадает | PASS | [25 GiB / 500 файлов](large-build-20261119.md), [L2](release-1.0.0-l2.md) |
| 27 | Cloud: несколько дисков/каталогов | PASS | `storage_tests`, K1; [скрин мониторинга](screenshots/l5-monitoring.png) |
| 28 | Cloud: файлы физически находятся в зарегистрированных корнях | PASS | [K2 backup/delete/restore и SHA](objects-backup-20261120-k2.md), `storage_tests` |
| 29 | Cloud: папки и файловые операции | PASS | I1 Cloud E2E, `cloud_tests`; [L2](release-1.0.0-l2.md) |
| 30 | Cloud: большие загрузки возобновляются | PASS | I1 Windows resume, [L2 resilience](resilience-20261120-l2.md), `cloud_tests` |
| 31 | Cloud: Range | PASS | [J2 Range acceptance](proxy-https-20261120-j2.md), [L2 load](load-20261120-l2.md) |
| 32 | Cloud: предпросмотр форматов | PASS | E1/E2 frontend tests; [L2](release-1.0.0-l2.md) |
| 33 | Cloud: неудачная замена сохраняет старый файл | PASS | `replacement_tests`, `cloud_tests`; [I3/I4](i3-i4-tests-20261120.md) |
| 34 | Cloud: нет доступа к чужим файлам | PASS | `windows_invariants_tests`, `cloud_tests`, I1 access E2E |
| 35 | System: создание и управление пользователями | PASS | I1 System E2E, `auth_tests`, [L2](release-1.0.0-l2.md) |
| 36 | System: права по модулям | PASS | I1 `access.spec.ts`, backend `modules_tests` |
| 37 | System: реальные показатели сервера | PASS | [monitoring JSON](logs/l5-monitoring-current.json), [скриншот](screenshots/l5-monitoring.png) |
| 38 | System: графики из backend | PASS | [monitoring screenshot](screenshots/l5-monitoring.png), captured metrics JSON |
| 39 | System: журнал событий | PASS | [скриншот журнала](screenshots/l5-audit-journal.png), [сводка L5](logs/l5-db-checks.txt) |
| 40 | System: управление фоновыми задачами через UI | PASS | [скриншот задач](screenshots/l5-jobs.png), [jobs JSON](logs/l5-jobs.json), [DB checks](logs/l5-db-checks.txt) |
| 41 | Надёжность: RAM не растёт неконтролируемо | PASS | [25 GiB G2](large-build-20261119.md), [L2 load/private bytes](load-20261120-l2.md) |
| 42 | Надёжность: обрыв не приводит к потере данных | PASS | [L2 resilience](resilience-20261120-l2.md), regression `interrupted_patch_is_truncated_before_resume` |
| 43 | Надёжность: выход за каталог хранения невозможен | PASS | `windows_invariants_tests` (traversal/junction/ADS); [лог](logs/l3-windows-invariants.log) |
| 44 | Надёжность: reboot не повреждает сборки и загрузки | PASS | [J4 reboot: SHA, offset и 0 orphan running](reboot-20261120-j4.md) |
| 45 | Надёжность: после ошибки файлы согласованы | PASS | `replacement_tests`, `idempotency_tests`, `cloud_tests`; [L2](release-1.0.0-l2.md) |

### Отдельные Windows-доказательства

- **Служба после reboot:** [J4](reboot-20261120-j4.md): `Automatic`, запуск примерно через 20 секунд после загрузки Windows, health и PostgreSQL доступны, SHA завершённого файла совпал, upload offset восстановлен, orphan running = 0.
- **Служба остановлена во время загрузки:** **BLOCKED** для точного сценария active PATCH + `Stop-Service`. Доказаны отдельно `Stop-Service` с активной running-задачей и восстановлением C2 ([J1/J2](service-stop-20261120-j1j2.md)), а также обрыв клиента с resume ([L2](resilience-20261120-l2.md)); они не эквивалентны одновременному сценарию. Тестовые пароли в текущем окружении не заданы.
- **Junction / ADS / длинный путь:** `windows_invariants_tests`, [сохранённый лог](logs/l3-windows-invariants.log) и [I1 Windows E2E](i1-playwright-part2-20261119.md).
- **Backup залоченного файла:** [K2 §7](objects-backup-20261120-k2.md): `FileShare.None`, VSS backup, файл скопирован, SHA совпал; backup/restore проверен.
- **Референс главной:** **BLOCKED** — исходные референсы отсутствуют; приложенные `l3-home-reference.png`/`l3-home-app.png` являются снимками приложения.

## L4. Релиз 1.0.0

Фактический артефакт `dist/nexus-1.0.0-windows-x64.zip`: **7 503 507 байт**, 33 ZIP entries (32 файла из `SHA256SUMS` плюс сам `SHA256SUMS`). SHA-256 архива: `0d4875621772b54475cdfd008fbfff4848770024f601503f456ceb460f78c966`; sidecar `.zip.sha256` совпадает. Перепроверка всех 32 внутренних записей `SHA256SUMS`: **0 mismatches**. Версия архива — `1.0.0`; инструкция установки из распакованного архива находится в [INSTALL-WINDOWS.md](../INSTALL-WINDOWS.md).

В ранних J5/L2 текстах указаны другие размеры и SHA: это устаревшие результаты предыдущей упаковки, не checksum текущего файла. Установка из распакованного архива была проверена J5 с `-SkipService`; инструкция описывает полный сценарий Windows 11, но pristine-VM прогон полного install из этого ZIP не выполнялся.

Тег `v1.0.0` не используется: Git-инфраструктура исключена из проекта.

## L5. Пострелизный мониторинг

| Проверка | Результат | Доказательство |
|---|---|---|
| Метрики и графики мониторинга | PASS на сохранённом снимке: приложение, CPU/RAM, сеть, диски, активность и ошибки API | [JSON](logs/l5-monitoring-current.json), [скриншот](screenshots/l5-monitoring.png) |
| Журнал событий | PASS: записи входов и системных операций отображены | [скриншот](screenshots/l5-audit-journal.png) |
| Фоновые задачи по расписанию | PASS в зафиксированном прогоне: пять scheduled job types, по 11 completed each; failed = 0 | [DB checks](logs/l5-db-checks.txt), [jobs JSON](logs/l5-jobs.json), [скриншот](screenshots/l5-jobs.png) |
| Ошибки API / алерты | N/A: мониторинг показывает `errorsLastMinute`/`errorRate`; внешние уведомления/alert destinations не настроены и в коде не обнаружены | [monitoring JSON](logs/l5-monitoring-current.json), `backend/src/system/monitoring_routes.rs` |
| Recovery службы | PASS: `Automatic`, recovery restart 5/15/60 s; forced kill восстановлен, health восстановился через 60,488 ms | [service recovery log](logs/l5-service-recovery.log), [J4 reboot](reboot-20261120-j4.md) |
| PostgreSQL / health | PARTIAL: `nexus check --json` = `ok=true`, прямой PostgreSQL query проходит. В серии из пяти health-запросов первые 2 вернули `database: down`, следующие 3 — `up`; журнал показывает acquire/query задержки до 8,7 s при 2 s health timeout. | `C:\NexusWorkspace\nexus.exe check --json`, `docs/reports/logs/l5-service-recovery.log`; повторные health и psql проверки 20.11.2026 18:11–18:16 MSK |
| Свободное место и тома | PARTIAL: C: NTFS, 15,82 GB free на 18:11; все четыре тома службы `online`. В БД службы остаются 6 объектов `missing` (3 `committed`); требуется решение администратора restore/purge. | Read-only `Get-CimInstance Win32_LogicalDisk`, `pg_stat`/`storage_objects`/`storage_volumes`; [L5 DB snapshot](logs/l5-db-checks.txt) и [monitoring screenshot](screenshots/l5-monitoring.png) |

Health является неустойчивым на коротком интервале, хотя восстановился без перезапуска, а `nexus check` и PostgreSQL query зелёные. Это оставлено как операционное предупреждение, не как PASS без оговорок.

## L4/L5 и §11: сводка Definition of Done

| № §11 | Критерий | Статус | Доказательство |
|---|---|---|---|
| 1 | P0/P1 закрыты, P2 закрыты или обоснованно исключены | PARTIAL | Основные проверки зелёные; остаются L3 ручные доказательства и стабильность health. См. этот отчёт. |
| 2 | Документы A1–A12 и рабочие ссылки | PASS | [индекс docs](../README.md), [L2 release report](release-1.0.0-l2.md); A1–A12 и локальные ссылки проверены. |
| 3 | `cargo fmt`, `clippy -D warnings`, `cargo test` | PASS | [L2](release-1.0.0-l2.md): 332 backend tests passed. |
| 4 | Typecheck, frontend tests, production build | PASS | [L2](release-1.0.0-l2.md): 65 frontend tests, typecheck/build exit 0. |
| 5 | Playwright E2E | PASS | [L2](release-1.0.0-l2.md): 24 passed. |
| 6 | 25-GiB test, report, RAM | PASS | [G2 25 GiB](large-build-20261119.md): 500 SHA, private bytes +2.0 MB. |
| 7 | Windows deployment | PASS | Windows J1-J5 evidence; платформа поставки — Windows 11. |
| 8 | System settings applied or marked restart-required | PASS | `settings_tests`; [L2](release-1.0.0-l2.md). |
| 9 | §11 realtime events published and tested | PASS | `realtime_tests`; [L2](release-1.0.0-l2.md), I1 E2E. |
| 10 | Private launcher builds isolated; limits/headers work | PASS | `launcher_tests`, `launcher_rate_limit_tests`; [L2](release-1.0.0-l2.md). |
| 11 | Scheduled trash cleanup, PDF, code highlighting | PASS | [jobs DB evidence](logs/l5-db-checks.txt), E1/E2 tests, [L2](release-1.0.0-l2.md). |
| 12 | `COMPLETE.md` updated with status/date/results | PASS | [COMPLETE.md](../../COMPLETE.md), [L2](release-1.0.0-l2.md). |
| 13 | Release 1.0.0 artifacts/checksums | PASS | Current archive and 32/32 internal hashes verified above; [J5 report](release-20261120-j5.md). |
| 14 | §17 acceptance signed | BLOCKED | Owner sign-off is not present; this report is evidence, not an owner signature. |

## Что осталось человеку

1. Приложить исходные визуальные референсы главной страницы и подписать §17 после их сравнения.
3. Выполнить один live-тест `Stop-Service` во время незавершённого PATCH с последующим recovery/resume; не передавать пароль через чат, задавать `NEXUS_ADMIN_PASSWORD` локально в PowerShell.
4. Проверить причины шести `storage_objects.state='missing'` в базе службы и принять решение о restore/purge; отдельно разобрать перемежающиеся `health database=down` и acquire/query latency до 8,7 s.
5. При необходимости полной формальной Windows-приёмки запустить полный install из ZIP на чистой Windows 11 VM; текущий J5 archive-install использовал `-SkipService`.
6. Решить, нужно ли настраивать внешние алерты по ошибкам API; сейчас есть только метрики, канал уведомлений не настроен.
