# I1 (часть 2): E2E Minecraft, Launcher, System, модули и Windows

**Дата:** 19.11.2026  
**Статус:** PASS — полный набор **21/21**, новых сценариев **11/11**, 0 падений  
**Прогон:** 4,6 минуты, release backend + production frontend, Windows 11 / NTFS

---

## 1. Новые сценарии

| # | Сценарий | Файл | Результат |
|---|---|---|---|
| 1 | Minecraft: мастер из 5 шагов, загрузка файла, проверка, релиз, публикация, манифест | `e2e/minecraft.spec.ts` | PASS |
| 2 | Launcher: выдача токена через UI, каталог, манифест, скачивание с Range | `e2e/launcher.spec.ts` | PASS |
| 3 | System: 13 страниц без `console.error`, `pageerror` и CSP-нарушений | `e2e/system.spec.ts` | PASS |
| 4 | System: настройка применяется и отражается в `effective` | `e2e/system.spec.ts` | PASS |
| 5 | Отключение модуля: навигация, прямой URL и API блокируются | `e2e/access.spec.ts` | PASS |
| 6 | Пользователь без `system.view` не видит системные разделы | `e2e/access.spec.ts` | PASS |
| 7 | Windows: загрузка файла с не-ASCII именем и RFC 5987 `filename*` | `e2e/windows.spec.ts` | PASS |
| 8 | Windows: имя, отличающееся только регистром, не перезаписывает файл | `e2e/windows.spec.ts` | PASS |
| 9 | Windows: слишком длинный путь → `422 PATH_TOO_LONG` без обрезания | `e2e/windows.spec.ts` | PASS |
| 10 | Windows: обрыв после части файла → продолжение с серверного offset | `e2e/windows.spec.ts` | PASS |
| 11 | Windows/UI: `connectionreset` на чанке → очередь повторяет с серверного offset | `e2e/windows.spec.ts` | PASS |

В каждом браузерном тесте автофикстура `consoleGuard` проверяет отсутствие
`console.error`, необработанных `pageerror` и `securitypolicyviolation`.
Ожидаемыми остаются только штатные `401/423/429` auth-эндпоинтов, `409`
конфликта имени/папки и намеренная сетевая ошибка на чанке в сценарии обрыва.

---

## 2. Полный прогон

```text
Running 21 tests using 1 worker

  ok   1 access.spec.ts    отключение модуля блокирует UI и API
  ok   2 access.spec.ts    пользователь без system.view не видит разделы
  ok   3 auth.spec.ts      вход, Cloud, выход
  ok   4 auth.spec.ts      неверный пароль и успешный вход
  ok   5 auth.spec.ts      блокировка после серии попыток
  ok   6 cloud.spec.ts     текст с подсветкой
  ok   7 cloud.spec.ts     PDF в canvas
  ok   8 cloud.spec.ts     2000 файлов
  ok   9 cloud.spec.ts     пауза/продолжение папки
  ok  10 cloud.spec.ts     замена при конфликте
  ok  11 cloud.spec.ts     публичная ссылка
  ok  12 cloud.spec.ts     корзина
  ok  13 launcher.spec.ts  токен, каталог, манифест, Range
  ok  14 minecraft.spec.ts мастер, файл, проверка, релиз, манифест
  ok  15 system.spec.ts    13 страниц без ошибок консоли
  ok  16 system.spec.ts    настройка применяется
  ok  17 windows.spec.ts   не-ASCII имя и filename*
  ok  18 windows.spec.ts   регистр-коллизия
  ok  19 windows.spec.ts   PATH_TOO_LONG
  ok  20 windows.spec.ts   resume после обрыва (API)
  ok  21 windows.spec.ts   resume после обрыва (UI-очередь)

  21 passed (4.6m)
```

Проверки после изменений:

| Проверка | Результат |
|---|---|
| `npm run typecheck` | exit 0 |
| `npm run test` | **46 passed** |
| `npm run build` | успешно, 1804 модуля, JS 1 163,70 кБ |
| `cargo test -j 2` | **269 passed, 0 failed** |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy -j 2 --all-targets -- -D warnings` | exit 0 |

---

## 3. Окружение

| Параметр | Значение |
|---|---|
| ОС | Windows 11 Pro 10.0.26300, NTFS |
| Node.js / npm | v26.13.1 / 11.20.0 |
| Playwright | 1.65.1, Chromium 159.0.8154.6 |
| Backend | `backend/target/release/nexus.exe`, 17 761 280 байт, 19.11.2026 23:21:39 |
| SHA-256 backend | `72DBE8DBAEAA2D3A46C20052DED11B34D8736917790C7CD1F5268D19A62BED09` |
| Frontend | production `frontend/dist`, сборка 19.11.2026 23:16:30 |
| Сервер E2E | `127.0.0.1:8091`, release-бинарник, `webServer` |

Для выделенных `e2e-admin` и `e2e-user` 3D-фон отключён через
`appearance.background`: System-тест открывает 13 страниц в headless Chromium,
а SwiftShader не должен влиять на время и стабильность.

---

## 4. Windows: что именно проверено

### Не-ASCII имя

Загружается `Отчёт <run> (итог).txt`. Сервер сохраняет имя дословно, скачивание
возвращает `Content-Disposition` с ASCII-fallback и RFC 5987 `filename*=UTF-8''…`;
декодированное имя совпадает с исходным, байты файла — с загруженными.

### Регистр

Сначала создаётся `Report-<run>.TXT`, затем `report-<run>.txt`. Вторая фиксация
получает `409 FILE_NAME_CONFLICT` с `existingFileId`; в списке одна запись,
исходные байты не перезаписаны.

### MAX_PATH

21 уровень по 10 UTF-16 единиц (236 единиц с коротким корнем), затем лист из
128 символов «я» → `422 PATH_TOO_LONG`, `actualLength > 260`, без обрезания.
Имя длиннее 128 символов отдельно отклоняется как `VALIDATION_ERROR`.

### Остановка/запуск и обрыв

Останавливать процесс `webServer` внутри Playwright нельзя: это оборвёт весь
прогон. Поэтому обрыв эмулируется двумя способами:

* API: после части файла клиентский контекст закрывается, новый контекст
  получает `offset` из `GET /cloud/uploads/{id}` и продолжает с него;
* UI: `route.abort('connectionreset')` на втором чанке; очередь делает retry,
  запрашивает серверный offset и продолжает с него. Проверяется, что повторный
  `PATCH` идёт с тем же смещением, а не с нуля, и что итоговый файл имеет
  исходный размер и корректный хвост.

Реальная остановка/запуск службы Windows остаётся в J4.

---

## 5. Дефекты, найденные и исправленные

| Дефект | Исправление |
|---|---|
| `module_guard` внутри `nest("/api/v1")` видел URI без префикса, поэтому отключённый Launcher отвечал `403 MODULE_DISABLED` вместо маскирующего `404` | Проверка обоих представлений пути: `/api/v1/launcher…` и `/launcher…` |
| `UploadQueue` после retry создавала новую сессию и отправляла файл с нуля; пауза/продолжение фактически не возобновляли передачу | Активная сессия переиспользуется, старт идёт с серверного offset; `UPLOAD_OFFSET_MISMATCH` обрабатывается как продолжение; unit-тест проверяет offset 4 и одну сессию |
| Прямой URL отключённого модуля показывал рабочий интерфейс, хотя API уже отвечал `MODULE_DISABLED` | `RequireModule` в `App.tsx` + экран `ModuleDisabledPage` |
| Администратор в headless-прогоне рисовал 3D-фон на 13 системных страницах | В `global-setup` фон отключён и для выделенного `e2e-admin` |

После исправления очереди: `npm run test` — **46 passed**; полный E2E — **21 passed**.

---

## 6. Запуск

E2E запускаются локально командой `npx playwright test`; внешний CI не используется.

* триггеры: `workflow_call` (для подключения из `ci.yml` на I6) и
  `workflow_dispatch`;
* `windows-latest`, PostgreSQL через `shogo82148/actions-setup-postgres`,
  release backend, production frontend, Chromium;
* при падении выгружаются `frontend/test-results`, `frontend/playwright-report`
  и `C:\nx-e2e\logs` (trace, screenshot, HTML-отчёт, console-csp).

Windows обязателен: сценарии регистра, не-ASCII и MAX_PATH проверяют реальную
семантику NTFS, а не эмуляцию.

---

## 7. Ограничения

* Тесты используют БД из `.env`, но изолированы `runId`, пользователями
  `e2e-*` и очисткой созданных записей.
* Windows-spec на не-Windows помечается `skipped`.
* Реальный stop/start службы Windows не выполняется внутри Playwright;
  используется эмуляция обрыва и переподключения (см. §4).
