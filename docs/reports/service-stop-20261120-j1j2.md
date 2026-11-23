# Остановка службы Windows и восстановление C2

Дата: 20.11.2026. Стенд: Windows 11 Pro 10.0.26300, NTFS, PostgreSQL 17.12,
release-сборка `nexus.exe` с встроенным SCM dispatcher (`windows-service`),
тестовая служба `NexusStopTest111436` (LocalSystem, временный каталог
`%TEMP%\nexus-stop-test`).

## 1. Фактический сигнал и таймаут

| Параметр | Значение |
|---|---|
| binPath | `"...\nexus.exe" serve --service` |
| `WaitToKillServiceTimeout` (HKLM\SYSTEM\CurrentControlSet\Control) | **5000 мс** |
| Внутренний graceful shutdown (`shutdown_grace`) | **4000 мс** (таймаут SCM − 1 с) |
| `Stop-Service` → сигнал | **`SERVICE_CONTROL_STOP`**, не Ctrl+C |
| `StopPending` wait hint | 5000 мс |
| Фактическое время `Stop-Service` (idle/running-запись) | **270–274 мс** |
| Принудительное завершение процесса SCM | не потребовалось (270 ≪ 5000) |
| Верхняя граница ожидания активных задач | 4000 мс (если обработчик не успел — задача остаётся `running` для C2) |
| `Stop-Process -Force` (имитация убийства по таймауту) | служба переходит в `Stopped`, процесс завершён |

`Stop-Service` не убивает процесс, пока диспетчер отвечает: служба получает
`SERVICE_CONTROL_STOP`, сообщает `StopPending`, выполняет graceful shutdown и
сама завершается. Ctrl+C в службе не используется. Если graceful shutdown
превысит `WaitToKillServiceTimeout`, SCM завершит процесс принудительно — это
эквивалентно проверенному ниже `Stop-Process -Force`, после которого C2
восстанавливает задачи при следующем старте.

## 2. Graceful Stop-Service с running-задачей

В БД перед остановкой создана задача `system.metrics.prune` в состоянии
`running` (имитация незавершённой единицы работы).

| Проверка | Факт |
|---|---|
| Служба после `Stop-Service` | `Stopped` |
| Задача после остановки | `status=running`, `cancel_requested=true` |
| Причина в `error` после старта | содержит «прервана перезапуском сервера и возвращена в очередь» |
| Журнал после старта | `задачи, прерванные прошлым запуском, возвращены в очередь` |
| `orphan running` после восстановления | 0 |

То есть graceful stop не подменяет статус задачи и не теряет её: задача
помечается отменяемой, а следующий старт возвращает её в очередь (C2).

## 3. Жёсткое падение

| Проверка | Факт |
|---|---|
| `Stop-Process -Force` по PID службы | служба `Stopped` |
| Следующий `Start-Service` | `Running`, health 200 |
| Журнал | новая запись «возвращены в очередь» |
| `orphan running` | 0 |
| Статус задачи | не `running`; восстановление зафиксировано журналом |

## 4. Автотесты

```powershell
powershell -ExecutionPolicy Bypass -File tests\service-stop.ps1
# PASS: Stop-Service и восстановление C2 подтверждены.
```

Rust-тесты:

* `jobs_tests::shutdown_marks_running_job_and_recovery_requeues_it` —
  `cancel_requested=true`, затем `recover_interrupted` → `pending`;
* `jobs_tests::restart_returns_running_job_to_pending` — базовый C2;
* `windows_service::tests::graceful_shutdown_stays_below_scm_timeout` —
  graceful shutdown всегда меньше таймаута SCM.

## 5. Вывод

Для выбранной обёртки (встроенный `windows-service`) `Stop-Service` передаёт
`SERVICE_CONTROL_STOP`; Ctrl+C не приходит. При штатной остановке процесс
завершается сам за ~0,27 с. Если остановка не укладывается в
`WaitToKillServiceTimeout` (на стенде 5000 мс), SCM убивает процесс; в этом
случае и после любого жёсткого падения `running`-задачи восстанавливаются в
`pending` при следующем старте. Осиротевших `running` нет.
