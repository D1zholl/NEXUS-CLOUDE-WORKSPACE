//! OpenAPI 3.1 спецификация (PLAN §11).
//!
//! Спецификация строится из явного реестра маршрутов: каждый маршрут описан
//! методом, путём и кратким описанием. Тест `openapi_tests` сверяет число
//! описанных операций с числом зарегистрированных маршрутов в коде, поэтому
//! расхождение спецификации и реализации обнаруживается сразу.

use std::sync::LazyLock;

/// Одна операция API.
#[derive(Clone, Copy)]
pub struct Operation {
    pub method: &'static str,
    pub path: &'static str,
    pub summary: &'static str,
    pub tag: &'static str,
    /// Требуется ли cookie-сессия браузера.
    pub session: bool,
    /// Требуется ли токен лаунчера.
    pub launcher: bool,
}

pub(crate) const fn op(
    method: &'static str,
    path: &'static str,
    summary: &'static str,
    tag: &'static str,
) -> Operation {
    Operation {
        method,
        path,
        summary,
        tag,
        session: true,
        launcher: false,
    }
}

pub(crate) const fn launcher_op(
    method: &'static str,
    path: &'static str,
    summary: &'static str,
) -> Operation {
    Operation {
        method,
        path,
        summary,
        tag: "Launcher",
        session: false,
        launcher: true,
    }
}

pub(crate) const fn public_op(
    method: &'static str,
    path: &'static str,
    summary: &'static str,
    tag: &'static str,
) -> Operation {
    Operation {
        method,
        path,
        summary,
        tag,
        session: false,
        launcher: false,
    }
}

mod access;
mod ai;
mod auth;
mod cloud;
mod launcher;
mod me;
mod minecraft;
mod storage;
mod system;
mod users;

/// Полный реестр маршрутов. Порядок сохраняет исходную спецификацию.
pub static OPERATIONS: LazyLock<Vec<Operation>> = LazyLock::new(|| {
    let mut operations = Vec::new();
    for block in [
        system::HEALTH_OPERATIONS,
        auth::OPERATIONS,
        users::OPERATIONS,
        access::OPERATIONS,
        me::OPERATIONS,
        system::SYSTEM_OPERATIONS,
        storage::OPERATIONS,
        cloud::OPERATIONS,
        minecraft::OPERATIONS,
        launcher::OPERATIONS,
        system::MONITORING_OPERATIONS,
        ai::OPERATIONS,
    ] {
        operations.extend_from_slice(block);
    }
    operations
});

mod document;

pub use document::{operation_count, specification};
