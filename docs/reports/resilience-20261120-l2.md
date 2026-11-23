# I5: устойчивость NEXUS (Windows)

**Результат:** `PASS`  
**Дата:** 2026-11-20T17:15:39.8148456+03:00  
**Команда:** `powershell -File tests/resilience.ps1 -DiskFailureMode Auto -TestService:False -ReportName "resilience-20261120-l2"`

## Окружение

| Параметр | Значение |
|---|---|
| ОС | Майкрософт Windows 11 Pro (build 26300, 10.0.26300) |
| CPU / RAM | Intel(R) Core(TM) 5 210H, 12 лог. ядер / 15,7 ГиБ |
| PowerShell / CLR | 5.1.26100.9444 / 4.0.30319.42000 |
| PostgreSQL | 17.12 |
| ФС тестового тома | NTFS (VHDX 2048 МБ) |
| Defender real-time | False |
| Сервер | `http://127.0.0.1:8080` (PID 2820) |

## 1. Отказ диска

| Проверка | Результат |
|---|---|
| Способ | VHDX + detach (снятие буквы и отключение диска) |
| VHDX / корень | `C:\Users\max\AppData\Local\Temp\nexus-resilience\resilience-7a0658d2.vhdx` |
| Том NEXUS | 1f6eacb6-5bfa-4703-97e7-2492b49dd572 (`Resilience 7a0658d2`) |
| До отказа: reachable / state | True / online |
| Отключение | diskpart detach vdisk |
| После отказа: reachable / state | False / offline |
| Загрузка на offline-том | HTTP 422 NO_VOLUME_AVAILABLE |
| Скачивание при отказе | HTTP 500, code STORAGE_ERROR: Файл отсутствует в хранилище. |
| Восстановление | True |
| SHA после восстановления | 52f24e3c61fd4d2e4c42e38a7b5225545652be049da182569863dea21f5541a8 |

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
| Задача | system.objects.verify (`7254b9c0-7b4e-4f2e-8128-f352039ff24c`) |
| Объектов подготовлено | 16 × 24 МиБ |
| running до Stop-Process | True |
| Stop-Process -Force | True |
| Статус в БД после kill | running |
| Переходы статусов | pending->running → running->pending → pending->running → running->completed |
| running → pending зафиксирован | True |
| Перезапуск | Start-Service (NexusWorkspace), 12,92 с |
| Задача снова взята | True |

## 4. Restart-Service

| Проверка | Результат |
|---|---|
| Проверялось | True |
| Служба | NexusWorkspace |
| Запуск / health | True / True |
| Restart-Service, с | 0,79 |
| Сессия пережила перезапуск | True |

