//! Реестр операций OpenAPI: users.

use super::{Operation, op};

pub const OPERATIONS: &[Operation] = &[
    // --- Пользователи ---
    op("get", "/api/v1/users", "Список пользователей", "Users"),
    op("post", "/api/v1/users", "Создать пользователя", "Users"),
    op("get", "/api/v1/users/{id}", "Пользователь", "Users"),
    op(
        "patch",
        "/api/v1/users/{id}",
        "Изменить пользователя, квоту и права",
        "Users",
    ),
    op(
        "delete",
        "/api/v1/users/{id}",
        "Удалить пользователя",
        "Users",
    ),
    op(
        "post",
        "/api/v1/users/{id}/password",
        "Сбросить пароль",
        "Users",
    ),
    op(
        "post",
        "/api/v1/users/{id}/unlock",
        "Разблокировать аккаунт",
        "Users",
    ),
    op(
        "post",
        "/api/v1/users/{id}/sessions/revoke",
        "Завершить сессии пользователя",
        "Users",
    ),
];
