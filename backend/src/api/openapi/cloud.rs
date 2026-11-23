//! Реестр операций OpenAPI: cloud.

use super::{Operation, op, public_op};

pub const OPERATIONS: &[Operation] = &[
    // --- Cloud Storage ---
    op("get", "/api/v1/cloud/config", "Лимиты загрузки", "Cloud"),
    op(
        "get",
        "/api/v1/cloud/entries",
        "Список файлов и папок",
        "Cloud",
    ),
    op("post", "/api/v1/cloud/entries", "Создать папку", "Cloud"),
    op(
        "post",
        "/api/v1/cloud/entries/batch",
        "Создать папки пакетно (до 200 имён)",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/entries/{id}",
        "Сведения о записи",
        "Cloud",
    ),
    op(
        "patch",
        "/api/v1/cloud/entries/{id}",
        "Переименовать или переместить запись (Idempotency-Key при strategy=replace)",
        "Cloud",
    ),
    op(
        "delete",
        "/api/v1/cloud/entries/{id}",
        "Переместить в корзину",
        "Cloud",
    ),
    op(
        "post",
        "/api/v1/cloud/entries/{id}/restore",
        "Восстановить из корзины",
        "Cloud",
    ),
    op(
        "delete",
        "/api/v1/cloud/entries/{id}/purge",
        "Удалить безвозвратно",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/entries/{id}/download",
        "Скачать файл (Range)",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/entries/{id}/preview",
        "Предпросмотр файла",
        "Cloud",
    ),
    op("delete", "/api/v1/cloud/trash", "Очистить корзину", "Cloud"),
    op(
        "post",
        "/api/v1/cloud/uploads",
        "Начать возобновляемую загрузку",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/uploads/{id}",
        "Смещение загрузки",
        "Cloud",
    ),
    op(
        "patch",
        "/api/v1/cloud/uploads/{id}",
        "Записать чанк",
        "Cloud",
    ),
    op(
        "delete",
        "/api/v1/cloud/uploads/{id}",
        "Отменить загрузку",
        "Cloud",
    ),
    op(
        "post",
        "/api/v1/cloud/uploads/{id}/complete",
        "Завершить загрузку (Idempotency-Key)",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/shares",
        "Список ссылок общего доступа",
        "Cloud",
    ),
    op(
        "post",
        "/api/v1/cloud/shares",
        "Создать ссылку (Idempotency-Key)",
        "Cloud",
    ),
    op(
        "delete",
        "/api/v1/cloud/shares/{id}",
        "Отозвать ссылку",
        "Cloud",
    ),
    op("get", "/api/v1/cloud/exports", "Список архивов", "Cloud"),
    op(
        "post",
        "/api/v1/cloud/exports",
        "Создать ZIP-архив (Idempotency-Key)",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/exports/{id}",
        "Состояние архива",
        "Cloud",
    ),
    op(
        "delete",
        "/api/v1/cloud/exports/{id}",
        "Удалить архив и запись задачи",
        "Cloud",
    ),
    op(
        "get",
        "/api/v1/cloud/exports/{id}/file",
        "Скачать архив",
        "Cloud",
    ),
    public_op(
        "get",
        "/api/v1/public/shares/{token}",
        "Сведения о публичной ссылке",
        "Public",
    ),
    public_op(
        "get",
        "/api/v1/public/shares/{token}/download",
        "Скачать по публичной ссылке",
        "Public",
    ),
    public_op(
        "post",
        "/api/v1/public/shares/{token}/unlock",
        "Проверить пароль и получить тикет скачивания",
        "Public",
    ),
];
