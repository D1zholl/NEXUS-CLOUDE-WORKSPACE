# NEXUS E2E (Playwright) — I1

End-to-end проверки release-сборки backend + production-сборки frontend.
Покрытие: вход/выход, неверный пароль, блокировка учётной записи; Cloud Storage
(файл, папка, пауза/продолжение, замена, ссылка, корзина, предпросмотр
PDF/текста); Minecraft (мастер из 5 шагов, файл, проверка, релиз, манифест);
Launcher (токен, каталог, манифест, Range-скачивание); System (14 маршрутов `/system` и
применение настройки); модули и права; Windows/NTFS (не-ASCII, регистр,
MAX_PATH, возобновление после обрыва).

Текущий сохранённый результат: **24 passed, 0 failed**, полный прогон release
backend + production frontend на Windows.

## Быстрый запуск (Windows)

```powershell
cd backend
cargo build --release

cd ..\frontend
npm ci
npm run build
npx playwright install chromium
npx playwright test
```

`playwright.config.ts` сам поднимает release-бинарник
`backend/target/release/nexus.exe` на `http://127.0.0.1:8091` через
`npm.cmd run e2e:server` (не bash). Порт 8091 выбран, чтобы не конфликтовать
с dev/release-сервером на 8080; при занятом порте webServer падает явно.

## Что создаётся автоматически

`globalSetup` (`e2e/fixtures/global-setup.ts`) идемпотентно готовит:

| Фикстура                    | Значение по умолчанию                                               |
| --------------------------- | ------------------------------------------------------------------- |
| Администратор               | `e2e-admin` / `Nexus!E2E-admin-2026` (создаётся CLI при отсутствии) |
| Пользователь                | `e2e-user` / `Nexus!E2E-user-2026`                                  |
| Пользователь для блокировки | `e2e-lock` / `Nexus!E2E-lock-2026`                                  |
| Том хранения                | `C:\nx-e2e\volume` (имя `E2E Fixtures`)                             |
| 2000 файлов                 | `C:\nx-e2e\fixtures\bulk-2000` + hardlink-копия на прогон           |
| PDF / JS / TXT              | `C:\nx-e2e\fixtures\run-<runId>`                                    |
| Папка для паузы             | `C:\nx-e2e\runs\<runId>\pause-<runId>` (файл ~70 МБ)                |

Короткий корень `C:\nx-e2e` обязателен: длинные имена файлов и вложенные
папки Cloud иначе упираются в MAX_PATH. Временные файлы и `.auth` не попадают
в репозиторий.

Для выделенного E2E-пользователя 3D-фон отключается через
`PUT /api/v1/me/settings/appearance.background` (`enabled: false`). В headless
Chromium SwiftShader делает 2000-файловую загрузку в разы медленнее; сам фон
не относится к I1 и покрыт unit-тестами.

## Переменные окружения

| Переменная                                  | Назначение                                                     |
| ------------------------------------------- | -------------------------------------------------------------- |
| `E2E_PORT` / `E2E_BASE_URL`                 | порт/URL release-сервера (по умолчанию 8091)                   |
| `E2E_ROOT`                                  | короткий корень фикстур (по умолчанию `C:\nx-e2e`)             |
| `E2E_ADMIN_USERNAME` / `E2E_ADMIN_PASSWORD` | E2E-администратор                                              |
| `E2E_USER_USERNAME` / `E2E_USER_PASSWORD`   | обычный пользователь                                           |
| `E2E_LOCK_USERNAME` / `E2E_LOCK_PASSWORD`   | пользователь для проверки блокировки                           |
| `DATABASE_URL`, `NEXUS_SECRET`              | передаются release-серверу (иначе берутся из корневого `.env`) |

Если `e2e-admin` уже существует с другим паролем, задайте `E2E_ADMIN_PASSWORD`
(CLI не сможет пересоздать пользователя).

## Очистка после упавшего прогона

Тесты удаляют созданные записи через API. Если прогон прервался, при
запущенном backend:

```powershell
cd frontend
npm.cmd run e2e:cleanup
```

Скрипт удаляет **все** Cloud-записи только у `e2e-user`.

## Консоль и CSP

Автофикстура `consoleGuard` падает на:

- `console.error` любой страницы (включая новые контексты, например
  анонимную ссылку);
- необработанные `pageerror`;
- события `securitypolicyviolation` (CSP).

Ожидаемыми считаются только HTTP-статусы, которые являются штатным
контрактом и обрабатываются UI:

- `401/423/429` у `/api/v1/auth/me` и `/api/v1/auth/login`
  (анонимный bootstrap, неверный пароль, блокировка);
- `409` у `POST /cloud/uploads/{id}/complete` (конфликт имени → «Заменить/Копия»);
- `409` у `POST /cloud/entries` (идемпотентное переиспользование папки).

Всё остальное, включая `500` и CSP-нарушения, — падение теста.

## Артефакты падения

`trace: retain-on-failure`, `screenshot: only-on-failure`; HTML-отчёт пишется
в `frontend/playwright-report`, сырые артефакты — в `frontend/test-results`.
При падении фикстура дополнительно прикрепляет `console-csp.json`.

## Windows/NTFS-сценарии

`e2e/windows.spec.ts` выполняется только на Windows (`test.skip` на других ОС):

- загрузка файла с не-ASCII именем (`Отчёт … (итог).txt`) и RFC 5987
  `filename*=UTF-8''…` при скачивании;
- имя, отличающееся только регистром, не создаёт второй файл и не перезаписывает
  байты (`FILE_NAME_CONFLICT`);
- путь длиннее бюджета Windows отклоняется `422 PATH_TOO_LONG` без обрезания;
- обрыв после части файла: новый клиентский контекст получает серверный offset
  и продолжает с него; UI-очередь переживает `connectionreset` на чанке и
  повторяет запрос с того же offset.

Остановка/запуск системной службы в Playwright не выполняется: сервер поднят
`webServer`, его остановка оборвала бы весь прогон. Обрыв эмулируется на
сетевом слое и через переподключение API-сессии; серверное смещение при этом
реальное.

## Запуск

E2E запускаются локально: Windows, PostgreSQL, release backend, production
frontend, Chromium; при падении сохраняются `test-results`,
`playwright-report` и логи `C:\nx-e2e\logs`.

## Известные ограничения

- Тесты работают против той же БД, что указана в `.env`, но данные
  изолированы уникальным `runId` и выделенными E2E-пользователями.
- Проверка блокировки временно ставит `security.login_max_failures = 3`
  (минимум каталога) и всегда возвращает 10 в `finally`.
- 2000-файловая папка удаляется через API (рекурсивный purge) — это заметная
  часть времени теста.
