# L2. Финальный прогон 1.0.0 (Windows)

**Дата:** 20.11.2026, 16:55–17:40 MSK.
**Платформа:** Windows 11 Pro 10.0.26300 x64, NTFS, PostgreSQL 17.12,
PowerShell 5.1.26100.9444, Node v26.13.1, Rust/cargo 1.100.
**Сервер:** установленная служба `NexusWorkspace` (`C:\NexusWorkspace\nexus.exe`,
`NT SERVICE\NexusWorkspace`), HTTPS через Caddy (`https://localhost`).
**Версия:** `nexus 1.0.0`, health `{"database":"up","status":"ok","version":"1.0.0"}`.

## 1. L1. Версия и документы

| Файл | Изменение |
|---|---|
| `backend/Cargo.toml` | `version = "1.0.0"` |
| `frontend/package.json`, `frontend/package-lock.json` | `1.0.0` |
| `CHANGELOG.md` | новый: Added / Changed / Fixed / Security, включая Windows-инварианты |
| `README.md` | заголовок 1.0.0, ссылка на CHANGELOG, 14 миграций, фактические результаты |
| `COMPLETE.md` | готовность по факту прогонов, 14 пунктов §11 |
| `docs/openapi.json` | `info.version = 1.0.0` (перегенерирован) |

## 2. Backend

```powershell
cd backend
cargo fmt --all -- --check          # exit 0
cargo clippy -j 2 --all-targets -- -D warnings   # exit 0, 0 предупреждений
cargo test -j 2                     # exit 0
cargo run -q -- check --json        # exit 0
```

`cargo test -j 2`:

```text
26 suites, 332 passed, 0 failed
54 unit + 4 main + 274 integration
```

`nexus check --json` (рабочая копия, БД `nexus`):

```json
{"admin_present":true,"info":{"admins":"3","database":"PostgreSQL 17.12",
"launcher_limits":"каталог 120/мин, артефакты 600/мин, параллельно 8",
"migrations":"14","static_dir":"...frontend/dist","volumes":"3"},
"ok":true,"problems":[],"version":"1.0.0","warnings":[]}
```

## 3. Frontend

```powershell
cd frontend
npm ci            # 149 packages, 0 vulnerabilities, exit 0
npm run typecheck # exit 0
npm run test      # 13 files, 65 passed
npm run build     # exit 0, 1808 modules
npx playwright test  # 24 passed (3.2m), exit 0
```

Сборка: `index-*.js` 1 174,69 кБ (gzip 318,44 кБ), CSS 63,01 кБ
(gzip 12,25 кБ), отдельные `pdf` 449,69 кБ и `highlight` 56,71 кБ.

E2E: вход/выход, блокировка, Cloud (2000 файлов, пауза, замена, ссылка,
корзина, PDF, подсветка), Minecraft-мастер/релиз/манифест, Launcher
(токен/каталог/Range), 14 System-страниц, reduced-motion, 360 px,
Windows-инварианты (`filename*`, регистр, `PATH_TOO_LONG`, resume).

## 4. Живой прогон против службы

Перед прогоном release-бинарник 1.0.0 и `frontend/dist` установлены в
`C:\NexusWorkspace`; прежний бинарник сохранён как `nexus.previous.exe`.
Для loopback-проверок по HTTP в `.env` службы временно заданы
`NEXUS_COOKIE_SECURE=false` и `NEXUS_PUBLIC_URL=http://127.0.0.1:8080`
(скрипты используют `http://127.0.0.1:8080`). После прогона значения
возвращены: `NEXUS_COOKIE_SECURE=true`, `NEXUS_PUBLIC_URL=https://localhost`;
служба перезапущена, health через HTTPS — 200.

Создан администратор `smokeadmin` (пароль только через
`NEXUS_ADMIN_PASSWORD`, в отчёте не хранится).

| Скрипт | Результат |
|---|---|
| `tests/smoke-new-features.ps1` | **PASS**, 26 проверок, exit 0 |
| `tests/ws-smoke.ps1` | **PASS**: hello, `job.progress`, аноним отклонён, exit 0 |
| `tests/resilience.ps1` | **PASS**, exit 0 |
| `tests/load.ps1` | **PASS**, exit 0 |

Ключевые числа:

* smoke: OpenAPI 107 путей, 17 ключей настроек, `UNKNOWN_SETTING` → 422,
  гейт модулей без рестарта, метрики `activeDownloads/WebSockets/Connections`.
* ws: `{"kind":"connected","system":true,...}` и
  `job.progress {done:1,total:1,progress:100}`.
* load: 200 списков (avg 1,5 мс, p95 2,2 мс, ошибок 0), 16 файлов × 4 МиБ
  (246,9 МБ/с), 32/32 скачивания с SHA, 24/24 Range; private
  11,1 МБ → peak/final 14,6 МБ (**+3,5 МБ** при лимите +20).
  Отчёт: [`load-20261120-l2.md`](load-20261120-l2.md).
* resilience: VHDX offline → `422 NO_VOLUME_AVAILABLE`, скачивание `500
  STORAGE_ERROR`, после подключения SHA совпал; обрыв на 25 % → offset
  `0/1048576`, продолжение и SHA OK; `Restart-Service` **0,79 с**, сессия
  жива; C2 `running→pending→running→completed`, рестарт службы **12,92 с**.
  Отчёт: [`resilience-20261120-l2.md`](resilience-20261120-l2.md).

`nexus.exe check --json` установленной службы (после возврата HTTPS):

```json
{"admin_present":true,"info":{"admins":"5","database":"PostgreSQL 17.12",
"log_dir":"C:/NexusWorkspace/logs","migrations":"14",
"static_dir":"C:/NexusWorkspace/frontend","volumes":"4"},
"ok":true,"problems":[],"version":"1.0.0","warnings":[]}
```

### 4.1. Изменения в `tests/resilience.ps1`

Скрипт предполагал, что сервер запущен вручную из `backend\target\release`,
и падал на установленной службе:

* `Win32_Service.StartService` на этом стенде возвращает
  `WBEM_E_INVALID_PARAMETER`; добавлен fallback `sc.exe start` и ожидание
  `Running`;
* `WorkingDirectory` вычислялся тремя уровнями выше exe и для
  `C:\NexusWorkspace\nexus.exe` давал NULL — теперь для службы берётся
  каталог exe;
* после `Restart-Service` PID не обновлялся, и C2 убивал уже несуществующий
  процесс — PID перечитывается по слушающему порту;
* C2 для службы поднимает её через SCM (`Start-Service`), а не второй
  экземпляр на занятом порту.

После правок скрипт по-прежнему работает и с ручным `serve`.

## 5. 25-ГБ сценарий

Повторный прогон не выполнялся: на `C:` оставалось ~15,5 ГиБ свободного
места, а сценарию нужно ≥ 2×25 ГиБ. Использован принятый отчёт G2:

* [`large-build-20261119.md`](large-build-20261119.md) — **PASS**,
  25 ГиБ / 500 файлов, 26 843 545 600 байт, 500 уникальных SHA,
  500 объектов, 38,37 МБ/с, private +2,0 МБ.

## 6. Платформа

Поставка и приёмка выполняются только на Windows; Linux исключён из проекта.

## 7. Release-артефакт

Артефакт J5 пересобран после смены версии:

```text
deployment\package-release.ps1 -SkipBuild -Version 1.0.0
Файлов: 32, размер 7 503 423 байт
SHA-256: 24f3a615771d2fe2cee9a4dc49b82ffff92016e1f3c5270a8fb957d376544147
nexus.exe: SHA-256 217157B404D57C6F1A130984243E7E5E5756C84FD71E95BBF122C688DE9A5444
nexus.exe --version → nexus 1.0.0
```

Контрольная сумма в [`release-20261120-j5.md`](release-20261120-j5.md)
относится к архиву, собранному до L1 (`f142f5ae…`, внутренняя версия
0.1.0); актуальная — `24f3a615…`.

## 8. Definition of Done §11

| № | Пункт | Статус | Доказательство |
|---:|---|---|---|
| 1 | P0/P1 закрыты, P2 закрыты или исключены | ✅ | COMPLETE.md, фазы A–K |
| 2 | Документы A1–A12, ссылки рабочие | ✅ | 40 `.md`, локальные ссылки: 0 битых |
| 3 | fmt/clippy/test | ✅ | 0 / 0 / 332 passed |
| 4 | typecheck/test/build | ✅ | 0 / 65 / успешно |
| 5 | Playwright ≥ 8 сценариев | ✅ | 24 passed |
| 6 | 25 ГиБ, отчёт, RAM | ✅ | G2, +2,0 МБ |
| 7 | Windows: установка, служба, HTTPS, WS, Range, backup/restore/update | ✅ | J1–J5 + L2 |
| 8 | Настройки применяются или «после перезапуска» | ✅ | 17 ключей, `settings_tests` |
| 9 | События §11 публикуются и покрыты | ✅ | `realtime_tests` 21 |
| 10 | Приватные сборки изолированы, лимиты и заголовки | ✅ | `launcher_tests` 18, `launcher_rate_limit_tests` 14 |
| 11 | Корзина по расписанию, PDF и подсветка | ✅ | `cloud_tests`, E2E PDF/highlight |
| 12 | COMPLETE.md: дата, результаты | ✅ | `COMPLETE.md` |
| 13 | Release-артефакты 1.0.0 + checksums | ✅ | `24f3a615…`, `SHA256SUMS` |
| 14 | Приёмочный чек-лист §17 подписан | ❌ **BLOCKED** | L3 в эту сессию не входил |

**Итог: 13/14.** Незакрыт только п. 14 — приёмочный прогон §17 (L3) и
подпись чек-листа. Причина — границы сессии (L1–L2), а не дефект сборки.

## 9. Замечания

* `cargo test` и `clippy` запускались с `-j 2` из-за `os error 1455`
  (файл подкачки) при неограниченном параллелизме.
* Во время resilience-прогонов остались два зарегистрированных тома
  `Resilience <tag>` с отсоединёнными VHDX; они отключены/удалены после
  прогона, VHDX отсоединены через `diskpart`.
* `npm ci` предупреждает об install-script `esbuild` (allowScripts), сборка
  и тесты при этом проходят.
