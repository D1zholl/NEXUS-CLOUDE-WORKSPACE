//! Реестр операций OpenAPI: access.

use super::{Operation, op};

pub const OPERATIONS: &[Operation] = &[
    // --- Роли и права ---
    op("get", "/api/v1/roles", "Список ролей", "Roles"),
    op("post", "/api/v1/roles", "Создать роль", "Roles"),
    op("patch", "/api/v1/roles/{id}", "Изменить роль", "Roles"),
    op("delete", "/api/v1/roles/{id}", "Удалить роль", "Roles"),
    op("get", "/api/v1/permissions", "Каталог прав", "Roles"),
];
