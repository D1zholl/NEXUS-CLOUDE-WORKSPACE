//! Реестр операций OpenAPI: storage.

use super::{Operation, op};

pub const OPERATIONS: &[Operation] = &[
    // --- Storage Engine ---
    op(
        "get",
        "/api/v1/storage/disks",
        "Физические диски",
        "Storage",
    ),
    op("get", "/api/v1/storage/volumes", "Тома хранения", "Storage"),
    op(
        "post",
        "/api/v1/storage/volumes",
        "Зарегистрировать том",
        "Storage",
    ),
    op(
        "post",
        "/api/v1/storage/volumes/scan",
        "Проверить каталог перед регистрацией тома",
        "Storage",
    ),
    op(
        "patch",
        "/api/v1/storage/volumes/{id}",
        "Изменить том",
        "Storage",
    ),
    op(
        "delete",
        "/api/v1/storage/volumes/{id}",
        "Удалить том",
        "Storage",
    ),
    op(
        "post",
        "/api/v1/storage/volumes/{id}/check",
        "Проверить том",
        "Storage",
    ),
    op(
        "get",
        "/api/v1/storage/summary",
        "Сводка хранилищ",
        "Storage",
    ),
    op(
        "get",
        "/api/v1/storage/owners",
        "Пользователи для привязки тома",
        "Storage",
    ),
    op(
        "get",
        "/api/v1/storage/browse",
        "Обзор каталогов сервера",
        "Storage",
    ),
];
