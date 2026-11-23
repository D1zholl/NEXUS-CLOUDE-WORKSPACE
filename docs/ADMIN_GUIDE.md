# Руководство администратора NEXUS WORKSPACE

Руководство разделено на тематические части. Каждая часть самостоятельна, но
ссылается на общий индекс и соседние разделы.

## Части руководства

| Часть | Содержание |
|---|---|
| [Установка и первый запуск](admin/install.md) | Требования, установка, миграции, первый администратор, проверка готовности |
| [Пользователи и права](admin/users.md) | Пользователи, роли, модель прав, квоты |
| [Диски и тома](admin/storage.md) | Термины, добавление каталога, состояния, проверка, планирование места |
| [Мониторинг и эксплуатация](admin/monitoring.md) | Модули, фоновые задачи, мониторинг, журнал, системные настройки |
| [Подключение MyLauncher](admin/launcher.md) | Адрес репозитория, каналы, токены, проверка и типовые ошибки |
| [Резервное копирование](admin/backup.md) | Состав копии, Windows-команды, объекты томов, восстановление |
| [Обновление и регламент](admin/update.md) | Штатное и ручное обновление, типовые ошибки, регламент администратора |

## Быстрые ссылки

- [Руководство пользователя](USER_GUIDE.md)
- [Развёртывание](DEPLOYMENT.md)
- [Безопасность](SECURITY.md)
- [API](API.md)

---

## AI Management (System Management)

Раздел `/system/ai` требует `ai.admin.manage` и содержит:

- **Overview** — module state, online Runtime, PolyStrata, задачи, approvals,
  очередь, последние ошибки/heartbeat.
- **Provider** — Base URL, скрытый API key, модель, generation, Runtime,
  test connection/refresh models.
- **Runtime** — enrollment code, статусы, heartbeat, версии, PolyStrata,
  программы, rotate/disable/revoke.
- **Workspaces** — owner, Runtime, root (только admin), квоты, лимиты,
  allowed tools/programs, suspend/resume.
- **Skills** — ZIP import, checksum/version, enable/disable, назначения, удаление.
- **Tools & Programs** — risk/approval/timeout/output policy, локальные программы.
- **Quotas** — задачи/параллельность/tool calls/time/storage/files.
- **Security & Logs** — AI audit и Runtime log.

Все элементы связаны с реальными API; при отсутствии данных UI показывает
«нет данных», а не фиктивные нули. См. [`AI_WORKSPACE.md`](AI_WORKSPACE.md).
