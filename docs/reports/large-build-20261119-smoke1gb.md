# G2: нагрузочный прогон 1 ГиБ

**Результат:** `PASS`  
**Дата:** 2026-11-19T20:20:34.0311673+03:00  
**Команда:** `tests/large-build.ps1 -SizeGb 1 -FileCount 500 -VolumeRoot C:\Users\max\AppData\Local\Temp\nexus-resilience\disk -ReportDir docs\reports -ReportName large-build-20261119-smoke1gb -Password *** -Cleanup True`

## Окружение

| Параметр | Значение |
|---|---|
| ОС | Майкрософт Windows 11 Pro (build 26300, 10.0.26300) |
| PowerShell / .NET | 5.1.26100.9444 / 4.0.30319.42000 |
| Файловая система | NTFS |
| Диск | C:, Phison ESO512GHLCA1-E9C, SSD (NVMe) |
| Свободно до старта | 34,99 ГиБ |
| Свободно после загрузки (до очистки) | 33,99 ГиБ |
| Свободно после очистки | 34,99 ГиБ |
| Defender real-time | False |
| Исключения для staging/objects | unknown / unknown |
| Defender note | Get-MpPreference недоступен: Не удалось выполнить операцию. Ошибка: 0x800106ba |

## Том и размещение

| Параметр | Значение |
|---|---|
| Том | Resilience (`C:\Users\max\AppData\Local\Temp\nexus-resilience\disk`) |
| Staging | `C:\Users\max\AppData\Local\Temp\nexus-resilience\disk\.nexus\staging` |
| Objects | `C:\Users\max\AppData\Local\Temp\nexus-resilience\disk\.nexus\objects` |
| Один том | True |
| Стратегия фиксации | rename |
| Требовалось места | 1,52 ГиБ + резерв 5 ГиБ = 6,52 ГиБ |

## Нагрузка

| Параметр | Значение |
|---|---|
| Запрошено | 1 ГиБ, 500 файлов |
| Загружено | 1 ГиБ (1073741824 байт), 500 файлов |
| Уникальных SHA-256 | 500 |
| Дубликатов SHA-256 | 0 |
| Чанков отправлено | 500 |
| Объектов добавлено на диск | 500 (1 ГиБ) |
| Staging до / после / добавлено | 1 / 1 / 0 |

## Время и пропускная способность

| Этап | Секунды |
|---|---:|
| Предварительная проверка | 2,3 |
| Генерация + SHA + загрузка | 38,33 |
| Проверка сборки | 0,02 |
| Релиз и манифест | 0,26 |
| Сверка SHA и Range | 0,79 |
| **Всего** | **45,07** |

Throughput загрузки: **26,72 МБ/с**, end-to-end: **22,72 МБ/с**.

## Память процесса NEXUS

| Метрика | МБ |
|---|---:|
| Baseline working set | 41,4 |
| Baseline private | 21 |
| Peak working set | 41,4 |
| Peak private | 21,2 |
| Прирост private | 0,2 |
| Порог | 200 |
| Результат | PASS |

Замеров: 4, интервал 10 с. Порог считается по private bytes.

## Сверка SHA-256 и манифест

| Проверка | Результат |
|---|---|
| Клиент = сервер (все файлы) | True |
| Клиент = манифест (все файлы) | True |
| Уникальных SHA в манифесте | 500 |
| Манифест детерминирован (2 запроса) | True |
| Манифест не изменился после публикации | True |
| Порядок файлов (ordinal) | True |
| Файлов в манифесте | 500 |
| Размер в манифесте | 1 ГиБ |

## Выборочное скачивание и Range

| Файл | Проверка | Статус | Байт | OK |
|---|---|---:|---:|---|
| `mods/g2-0000.jar` | full download + SHA | 200 | 2147483 | True |
| `mods/g2-0000.jar` | range 0..1MiB-1 | 206 | 1048576 | True |
| `mods/g2-0000.jar` | range middle 1MiB | 206 | 1048576 | True |
| `mods/g2-0000.jar` | range suffix 4096 | 206 | 4096 | True |
| `mods/g2-0249.jar` | full download + SHA | 200 | 2147483 | True |
| `mods/g2-0249.jar` | range 0..1MiB-1 | 206 | 1048576 | True |
| `mods/g2-0249.jar` | range middle 1MiB | 206 | 1048576 | True |
| `mods/g2-0249.jar` | range suffix 4096 | 206 | 4096 | True |
| `mods/g2-0499.jar` | full download + SHA | 200 | 2147807 | True |
| `mods/g2-0499.jar` | range 0..1MiB-1 | 206 | 1048576 | True |
| `mods/g2-0499.jar` | range middle 1MiB | 206 | 1048576 | True |
| `mods/g2-0499.jar` | range suffix 4096 | 206 | 4096 | True |

## Проверки

| Проверка | Результат |
|---|---|
| Все файлы загружены | True |
| Контент уникален (SHA) | True |
| Клиент = сервер = манифест | True |
| Фактический объём на диске совпадает | True |
| Staging не содержит остатков | True |
| Манифест детерминирован | True |
| Лаунчер видит тот же манифест | True |
| Range и полное скачивание | True |
| Память в пределах порога | True |
| Сборка проверена и опубликована | True |

## Очистка

Запрошена: да. Выполнена: True. Освобождено: 1 ГиБ.

## Артефакты прогона

* buildId: `df24b07c-51f2-4ea1-872f-8b2b3348a6c0`
* slug: `g2-large-build-20261119-202036`
* releaseId: `177702cc-2c41-4d93-87d6-8c7a0e58c4a4`
* releaseVersion: `1.0.202114`
* JSON: `docs\reports\large-build-20261119-smoke1gb.json`
* лог: `docs\reports\large-build-20261119-smoke1gb.log`

Полная временная серия памяти и машинные поля — в JSON рядом с этим отчётом.
