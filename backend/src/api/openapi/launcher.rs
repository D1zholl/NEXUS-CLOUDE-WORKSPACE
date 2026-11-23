//! Реестр операций OpenAPI: launcher.

use super::{Operation, launcher_op, op, public_op};

pub const OPERATIONS: &[Operation] = &[
    // --- Launcher API ---
    public_op(
        "post",
        "/api/v1/launcher/auth/token",
        "Выдать токен лаунчера",
        "Launcher",
    ),
    launcher_op(
        "post",
        "/api/v1/launcher/auth/revoke",
        "Отозвать свой токен",
    ),
    launcher_op("get", "/api/v1/launcher/catalog", "Каталог сборок"),
    launcher_op("get", "/api/v1/launcher/builds", "Список сборок"),
    launcher_op("get", "/api/v1/launcher/builds/{slug}", "Сборка по slug"),
    launcher_op(
        "get",
        "/api/v1/launcher/builds/{slug}/icon",
        "Иконка сборки",
    ),
    launcher_op(
        "get",
        "/api/v1/launcher/builds/{slug}/releases",
        "Релизы сборки",
    ),
    launcher_op(
        "get",
        "/api/v1/launcher/builds/{slug}/latest/manifest",
        "Манифест последнего релиза",
    ),
    launcher_op(
        "get",
        "/api/v1/launcher/builds/{slug}/releases/{version}/manifest",
        "Манифест версии",
    ),
    launcher_op("get", "/api/v1/launcher/artifacts/{id}", "Скачать артефакт"),
    op(
        "get",
        "/api/v1/admin/launcher/tokens",
        "Токены лаунчера",
        "Launcher",
    ),
    op(
        "delete",
        "/api/v1/admin/launcher/tokens/{id}",
        "Отозвать токен",
        "Launcher",
    ),
    op(
        "get",
        "/api/v1/admin/launcher/repository",
        "Параметры репозитория MyLauncher",
        "Launcher",
    ),
    // --- Публичный репозиторий MyLauncher (без токена) ---
    public_op("get", "/api/packs", "Каталог сборок MyLauncher", "Launcher"),
    public_op(
        "get",
        "/api/packs/{pack_id}/versions",
        "Версии сборки MyLauncher",
        "Launcher",
    ),
    public_op(
        "get",
        "/api/packs/{pack_id}/versions/{version}/manifest",
        "Манифест MyLauncher",
        "Launcher",
    ),
    public_op(
        "get",
        "/api/packs/{pack_id}/icon",
        "Иконка сборки MyLauncher",
        "Launcher",
    ),
    public_op(
        "get",
        "/api/files/{hash}",
        "Файл сборки MyLauncher",
        "Launcher",
    ),
];
