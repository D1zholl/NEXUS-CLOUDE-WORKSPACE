//! Реестр операций OpenAPI: system.

use super::{Operation, op, public_op};

pub const HEALTH_OPERATIONS: &[Operation] = &[
    // --- Health и realtime ---
    public_op("get", "/api/v1/health", "Состояние сервера", "System"),
    public_op(
        "get",
        "/api/v1/openapi.json",
        "Спецификация OpenAPI 3.1",
        "System",
    ),
    public_op(
        "get",
        "/api/v1/docs",
        "Swagger UI (NEXUS_ENABLE_DOCS)",
        "System",
    ),
    op("get", "/api/v1/ws", "WebSocket-канал прогресса", "Realtime"),
];

pub const SYSTEM_OPERATIONS: &[Operation] = &[
    // --- Публичное оформление ---
    public_op(
        "get",
        "/api/v1/public/appearance/background",
        "3D-фон для страниц без сессии",
        "Settings",
    ),
    // --- Здоровье системы и модули ---
    op("get", "/api/v1/system/overview", "Обзор системы", "System"),
    op(
        "get",
        "/api/v1/system/modules",
        "Состояние модулей",
        "System",
    ),
    op(
        "put",
        "/api/v1/system/modules/{key}",
        "Включить или отключить модуль",
        "System",
    ),
    op("get", "/api/v1/system/audit", "Журнал событий", "System"),
    op(
        "get",
        "/api/v1/system/settings",
        "Системные настройки",
        "Settings",
    ),
    op(
        "get",
        "/api/v1/system/settings/catalog",
        "Реестр системных настроек",
        "Settings",
    ),
    op(
        "put",
        "/api/v1/system/settings/{key}",
        "Сохранить системную настройку",
        "Settings",
    ),
];

pub const MONITORING_OPERATIONS: &[Operation] = &[
    // --- Мониторинг и задачи ---
    op(
        "get",
        "/api/v1/monitoring/current",
        "Текущие метрики",
        "Monitoring",
    ),
    op(
        "get",
        "/api/v1/monitoring/history",
        "История метрик",
        "Monitoring",
    ),
    op(
        "get",
        "/api/v1/monitoring/modules",
        "Метрики модулей",
        "Monitoring",
    ),
    op(
        "get",
        "/api/v1/monitoring/intervals",
        "Интервалы истории",
        "Monitoring",
    ),
    op("get", "/api/v1/jobs", "Фоновые задачи", "Jobs"),
    op("get", "/api/v1/jobs/{id}", "Задача", "Jobs"),
    op(
        "post",
        "/api/v1/jobs/{id}/pause",
        "Приостановить задачу",
        "Jobs",
    ),
    op(
        "post",
        "/api/v1/jobs/{id}/resume",
        "Возобновить задачу",
        "Jobs",
    ),
    op(
        "post",
        "/api/v1/jobs/{id}/cancel",
        "Отменить задачу",
        "Jobs",
    ),
    op(
        "post",
        "/api/v1/jobs/maintenance",
        "Запустить обслуживание",
        "Jobs",
    ),
];
