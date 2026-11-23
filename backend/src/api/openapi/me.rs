//! Реестр операций OpenAPI: me.

use super::{Operation, op};

pub const OPERATIONS: &[Operation] = &[
    // --- Настройки пользователя ---
    op(
        "get",
        "/api/v1/me/settings",
        "Настройки пользователя",
        "Settings",
    ),
    op(
        "put",
        "/api/v1/me/settings/{key}",
        "Сохранить настройку пользователя",
        "Settings",
    ),
    op("get", "/api/v1/me/usage", "Использование квоты", "Settings"),
];
