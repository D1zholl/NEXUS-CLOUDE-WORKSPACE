# I1 (часть 1): Playwright на Windows — вход и Cloud

**Дата:** 19.11.2026  
**Статус:** PASS — 10/10 E2E-тестов, 0 падений, 0 ошибок консоли/CSP  
**Итоговый прогон:** 3.0 минуты, release backend + production frontend

---

## 1. Что проверено

| # | Сценарий | Результат |
|---|---|---|
| 1 | Вход через UI, переход в Cloud, выход, защита маршрута после выхода | PASS |
| 2 | Неверный пароль: ошибка, затем успешный вход | PASS |
| 3 | Блокировка после серии неверных попыток (`ACCOUNT_LOCKED`) | PASS |
| 4 | Загрузка файла и предпросмотр текста с подсветкой (`hljs-*`) | PASS |
| 5 | Предпросмотр PDF: `<canvas>` реально отрисован | PASS |
| 6 | Загрузка папки из 2000 файлов, счётчик и пагинация | PASS |
| 7 | Пауза/продолжение загрузки папки (файл 70 МБ, 2 чанка) | PASS |
| 8 | Замена файла при конфликте имени | PASS |
| 9 | Публичная ссылка: анонимный контекст, скачивание | PASS |
| 10 | Корзина: перемещение и восстановление | PASS |

В каждом тесте автофикстура проверяет отсутствие `console.error`,
необработанных `pageerror` и CSP-нарушений (`securitypolicyviolation`).

Ожидаемыми считаются только штатные HTTP-ответы, которые UI обрабатывает:
`401/423/429` у `/auth/me` и `/auth/login`; `409` у `POST /cloud/uploads/{id}/complete`
(конфликт имени) и `409` у `POST /cloud/entries` (идемпотентное переиспользование
папки). Любой другой `500` или CSP-инцидент валит тест.

---

## 2. Окружение

| Параметр | Значение |
|---|---|
| ОС | Windows 11 Pro 10.0.26300, NTFS |
| Node.js / npm | v26.13.1 / 11.20.0 |
| Playwright | 1.65.1 |
| Chromium | Chrome for Testing 159.0.8154.6 (revision 1255) |
| Backend | release `backend/target/release/nexus.exe`, 17 761 280 байт, сборка 19.11.2026 21:47:16 |
| SHA-256 backend | `CE9A50B4B35FA05586B7ADF365FED63B411B7370474849D0049F606A8183504A` |
| Frontend | `npm run build`, production `frontend/dist` от 19.11.2026 22:45:33 |
| Сервер E2E | `127.0.0.1:8091`, release-бинарник |
| Порты | 8080 занят старым debug-процессом; E2E использует 8091 и падает при занятом порте |

Запуск webServer — только через `npm.cmd run e2e:server` (внутри —
`node e2e/start-server.mjs`), без bash. Пути собираются через `path.join`,
`webServer` ждёт `/api/v1/health` до 180 с.

---

## 3. Фикстуры

`frontend/e2e/fixtures/global-setup.ts` идемпотентно создаёт:

* администратора `e2e-admin` (если отсутствует — через `nexus create-admin`);
* пользователя `e2e-user`;
* отдельного пользователя `e2e-lock` для проверки блокировки;
* том `E2E Fixtures` в `C:\nx-e2e\volume`;
* PDF, JS-файл с подсветкой, TXT для ссылки/замены/корзины;
* папку паузы с файлом ~70 МБ (2 чанка по 64 МБ).

Короткий корень `C:\nx-e2e` выбран специально: длинные деревья Cloud и
имена файлов на Windows упираются в MAX_PATH. В репозиторий ничего не пишется,
кроме `frontend/e2e/.auth/*.json` (gitignore).

### Фикстура 2000 файлов и Defender

| Параметр | Значение |
|---|---|
| Каталог | `C:\nx-e2e\fixtures\bulk-2000` |
| Файлов | 2000 (+ служебный `.complete`) |
| Вложенность | 20 подпапок `sub-*` |
| Первая генерация | **251 мс** |
| Повторные прогоны | каталог переиспользуется; на прогон создаётся hardlink-копия |
| Hardlink-копия (финальный прогон) | 186 мс |
| Defender | `RealTimeProtectionEnabled=False`, `AntivirusEnabled=False`, `AMServiceEnabled=False` |
| Исключение Defender | **не создавалось и не требовалось** |

Важно: на этой машине защита Defender отключена, поэтому 251 мс — время без
сканирования в реальном времени. При включённом Defender на медленном диске
следует повторить замер; при необходимости добавить `C:\nx-e2e` в
`Add-MpPreference -ExclusionPath` и зафиксировать это в отчёте.

Сырой замер и состояние Defender: [`i1-playwright-fixtures.log`](i1-playwright-fixtures.log).

---

## 4. Команды

```powershell
cd backend
cargo build --release

cd ..\frontend
npm ci
npm run build
npx playwright install chromium
npx playwright test
```

`webServer` поднимается автоматически на 8091; отдельно запускать сервер не нужно.

---

## 5. Вывод `npx playwright test`

Полный лог: [`i1-playwright-run.log`](i1-playwright-run.log).

```text
[WebServer]
> nexus-frontend@0.1.0 e2e:server
> node e2e/start-server.mjs

[WebServer] [e2e] старт release backend: C:\Users\max\Desktop\MWS-NEXUS\backend\target\release\nexus.exe
[WebServer] [e2e] 127.0.0.1:8091, static=C:\Users\max\Desktop\MWS-NEXUS\frontend\dist
[e2e] run=mwnzayr5-c94k; fixtures: 2000 files, generate=0 ms, hardlink=227 ms; volume=C:\nx-e2e\volume

Running 10 tests using 1 worker

  ok  1 [chromium] › e2e\auth.spec.ts:64:3 › I1: вход и сессии › вход через UI, переход в Cloud и выход (17.3s)
  ok  2 [chromium] › e2e\auth.spec.ts:86:3 › I1: вход и сессии › неверный пароль: понятная ошибка и успешный вход после неё (11.2s)
  ok  3 [chromium] › e2e\auth.spec.ts:96:3 › I1: вход и сессии › блокировка после серии неверных попыток (14.6s)
  ok  4 [chromium] › e2e\cloud.spec.ts:38:3 › I1: Cloud Storage › загрузка файла и предпросмотр текста с подсветкой (5.4s)
  ok  5 [chromium] › e2e\cloud.spec.ts:59:3 › I1: Cloud Storage › предпросмотр PDF в canvas (5.1s)
  ok  6 [chromium] › e2e\cloud.spec.ts:83:3 › I1: Cloud Storage › папка: 2000 файлов и корректный счётчик (1.5m)
  ok  7 [chromium] › e2e\cloud.spec.ts:99:3 › I1: Cloud Storage › пауза и продолжение загрузки папки (12.9s)
  ok  8 [chromium] › e2e\cloud.spec.ts:139:3 › I1: Cloud Storage › замена файла при конфликте имени (4.3s)
  ok  9 [chromium] › e2e\cloud.spec.ts:165:3 › I1: Cloud Storage › публичная ссылка открывается без сессии и скачивает файл (10.7s)
  ok 10 [chromium] › e2e\cloud.spec.ts:203:3 › I1: Cloud Storage › корзина: перемещение и восстановление (6.1s)

  10 passed (3.0m)
```

---

## 6. Артефакты падения

В `playwright.config.ts`:

* `trace: retain-on-failure`;
* `screenshot: only-on-failure`;
* `outputDir: frontend/test-results`;
* HTML-отчёт: `frontend/playwright-report`.

При падении фикстура дополнительно прикладывает `console-csp.json` с
ошибками консоли/CSP. Механика проверена на промежуточных падениях: Playwright
сохранял `test-failed-1.png`, `trace.zip` и `console-csp.json`. Финальный
прогон зелёный, поэтому в `test-results` артефактов падения нет — это
ожидаемое поведение.

---

## 7. Дефекты, найденные и исправленные при подготовке E2E

| Дефект | Исправление |
|---|---|
| Быстрый «Пауза → Продолжить» запускал второй `run` того же файла; два `complete` использовали общий `Idempotency-Key` → `422 IDEMPOTENCY_KEY_REUSED` | `UploadQueue.nextQueued` не выдаёт элемент, уже находящийся в `running`; ключ фиксации читается в локальную переменную |
| `ensureFolderPath` создавал локальный кэш на каждый файл → для 2000 файлов повторные `POST` + `GET` папки | Кэш папок вынесен в `folderCacheRef` на время жизни страницы |
| Очередь рендерила 2000 DOM-строк и инвалидировала Cloud-список на каждом прогрессе | Показываются активные + 40 ближайших + последние 10; публикация UI — не чаще 200 мс; список — не чаще 750 мс |
| Headless Chromium рисовал 3D-фон через SwiftShader и замедлял 2000-файловую загрузку в ~5 раз | Для выделенного E2E-пользователя фон отключён через `appearance.background` (unit-тесты фона не затронуты) |

Регрессия на гонку pause/resume добавлена в
`frontend/src/lib/uploadQueue.test.ts`; полный frontend-набор — **46/46**.

---

## 8. Проверки после изменений

| Проверка | Результат |
|---|---|
| `cargo build --release` | успешно |
| `npm run build` | успешно (production dist) |
| `npm run typecheck` | exit 0 |
| `npm run test` | 46 passed, 0 failed |
| `npx playwright test` | 10 passed, 0 failed |
| Очистка Cloud после прогона | 0 активных и 0 записей в корзине у E2E-пользователей |
| `security.login_max_failures` после теста блокировки | восстановлено в `10` |

---

## 9. Ограничения и следующий шаг

* Прогон использует БД из `.env` (`nexus`), но данные изолированы
  пользователями `e2e-*` и уникальным `runId`; после прогона Cloud пуст.
* 2000-файловая папка удаляется рекурсивным purge через API — это около
  минуты из 1.4 минуты теста.
* I1 в полном объёме (Minecraft, Launcher, System, права) — следующая сессия.
  Инфраструктура `frontend/e2e/` уже готова для новых spec-файлов.
* Команда запуска рассчитана на Windows (в `playwright.config.ts` выбирается
  `npm`/`npm.cmd`).

Документация запуска: [`frontend/e2e/README.md`](../../frontend/e2e/README.md).
