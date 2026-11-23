# I5: устойчивость NEXUS (Windows)

**Результат:** `PASS`  
**Дата:** 2026-11-20T10:27:27.5212374+03:00  
**Команда:** `powershell -File tests/resilience.ps1 -DiskFailureMode Vhdx -TestService:True -ReportName "resilience-20261120-i5"`

## Окружение

| Параметр | Значение |
|---|---|
| ОС | Майкрософт Windows 11 Pro (build 26300, 10.0.26300) |
| CPU / RAM | Intel(R) Core(TM) 5 210H, 12 лог. ядер / 15,7 ГиБ |
| PowerShell / CLR | 5.1.26100.9444 / 4.0.30319.42000 |
| PostgreSQL | 17.12 |
| ФС тестового тома | NTFS (VHDX 2048 МБ) |
| Defender real-time | False |
| Сервер | `http://127.0.0.1:8080` (PID 14776) |

## 1. Отказ диска

| Проверка | Результат |
|---|---|
| Способ | VHDX + detach (снятие буквы и отключение диска) |
| VHDX / корень | `C:\Users\max\AppData\Local\Temp\nexus-resilience\resilience-ec6e2e88.vhdx` |
| Том NEXUS | 4cbdba4f-e21f-44f6-859e-93a5d7ff113d (`Resilience ec6e2e88`) |
| До отказа: reachable / state | True / online |
| Отключение | diskpart detach vdisk |
| После отказа: reachable / state | False / offline |
| Загрузка на offline-том | HTTP 422 NO_VOLUME_AVAILABLE |
| Скачивание при отказе | HTTP 500, code STORAGE_ERROR: Файл отсутствует в хранилище. |
| Восстановление | True |
| SHA после восстановления | d3f1277360e9968d1c97e21db3fa67bbdd6c8a15f093a76534940849e10544c4 |

## 2. Обрыв клиента

| Проверка | Результат |
|---|---|
| Обрыв на 25% тела | True |
| Offset после обрыва | 0 из 1048576 |
| Статус сессии | active |
| Продолжение с серверного offset | True |
| SHA после завершения | True |

## 3. Жёсткое падение (C2)

| Проверка | Результат |
|---|---|
| Задача | system.objects.verify (`56fe001a-de11-4156-9e25-62afcdf34e78`) |
| Объектов подготовлено | 16 × 24 МиБ |
| running до Stop-Process | True |
| Stop-Process -Force | True |
| Статус в БД после kill | running |
| Переходы статусов | pending->running → running->pending → pending->running → running->completed |
| running → pending зафиксирован | True |
| Перезапуск | Start-Process (служба не использовалась для C2), 0,52 с |
| Задача снова взята | True |

## 4. Restart-Service

> **Обновление 20.11.2026 (J1):** ограничение снято. В бинарник добавлен
> встроенный SCM dispatcher (`serve --service`); `Restart-Service` — 266 мс,
> `Stop-Service` — 270 мс, сессия жива. Актуальные результаты:
> [`service-stop-20261120-j1j2.md`](service-stop-20261120-j1j2.md).

| Проверка | Результат |
|---|---|
| Проверялось | False |
| Служба | NexusResilienceec6e2e88 |
| Запуск / health | False / False |
| Restart-Service, с |  |
| Сессия пережила перезапуск | False |
| Причина / ограничение | nexus.exe не реализует StartServiceCtrlDispatcher: SCM не получает SERVICE_RUNNING, процесс остаётся StartPending. Start-Service/Restart-Service недоступны; нужен service-wrapper (NSSM/winsw) или windows-service в бинарнике. |

## Известные ограничения

* nexus.exe не реализует StartServiceCtrlDispatcher: SCM не получает SERVICE_RUNNING, процесс остаётся StartPending. Start-Service/Restart-Service недоступны; нужен service-wrapper (NSSM/winsw) или windows-service в бинарнике.
