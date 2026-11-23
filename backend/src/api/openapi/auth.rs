//! Реестр операций OpenAPI: auth.

use super::{Operation, op, public_op};

pub const OPERATIONS: &[Operation] = &[
    // --- Авторизация ---
    public_op(
        "post",
        "/api/v1/auth/login",
        "Вход по логину и паролю",
        "Auth",
    ),
    public_op("post", "/api/v1/auth/logout", "Выход", "Auth"),
    op(
        "get",
        "/api/v1/auth/me",
        "Текущий пользователь и права",
        "Auth",
    ),
    op("post", "/api/v1/auth/password", "Смена пароля", "Auth"),
    op(
        "post",
        "/api/v1/auth/sessions/revoke-all",
        "Завершить все сессии",
        "Auth",
    ),
    public_op(
        "get",
        "/api/v1/auth/setup-status",
        "Нужна ли первичная настройка",
        "Auth",
    ),
];
