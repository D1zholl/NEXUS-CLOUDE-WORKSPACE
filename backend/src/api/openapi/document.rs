//! Сборка OpenAPI-документа, общие схемы и ответы.

use serde_json::{Value, json};

use super::{OPERATIONS, Operation};

fn path_parameters(path: &str) -> Vec<Value> {
    let mut params = Vec::new();
    for segment in path.split('/') {
        if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            params.push(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            }));
        }
    }
    params
}

fn query_parameter(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "in": "query",
        "required": false,
        "description": description,
        "schema": schema,
    })
}

fn is_mutating(method: &str) -> bool {
    matches!(method, "post" | "put" | "patch" | "delete")
}

fn is_launcher_rate_limited(operation: &Operation) -> bool {
    (operation.launcher && !operation.path.contains("/auth/"))
        || operation.path.starts_with("/api/packs")
        || operation.path.starts_with("/api/files/")
}

fn is_binary_download(method: &str, path: &str) -> bool {
    method == "get"
        && (path.ends_with("/download")
            || path.ends_with("/preview")
            || path.ends_with("/file")
            || path.ends_with("/icon")
            || path.starts_with("/api/v1/launcher/artifacts/")
            || path.starts_with("/api/files/"))
}

fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "headers": {
            "X-Request-Id": { "$ref": "#/components/headers/RequestId" }
        },
        "content": {
            "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
            }
        }
    })
}

fn response_data_schema(operation: &Operation) -> Value {
    match (operation.method, operation.path) {
        ("post", "/api/v1/cloud/uploads") => {
            json!({ "$ref": "#/components/schemas/UploadSession" })
        }
        ("get", "/api/v1/cloud/uploads/{id}") | ("get", "/api/v1/minecraft/uploads/{id}") => {
            json!({ "$ref": "#/components/schemas/UploadStatus" })
        }
        ("patch", "/api/v1/cloud/uploads/{id}") | ("patch", "/api/v1/minecraft/uploads/{id}") => {
            json!({ "$ref": "#/components/schemas/UploadChunkResponse" })
        }
        ("post", "/api/v1/minecraft/uploads") => {
            json!({ "$ref": "#/components/schemas/MinecraftUploadSession" })
        }
        ("get", "/api/v1/users")
        | ("get", "/api/v1/system/audit")
        | ("get", "/api/v1/cloud/entries")
        | ("get", "/api/v1/minecraft/builds") => {
            json!({ "$ref": "#/components/schemas/Page" })
        }
        ("get", "/api/v1/storage/volumes") => json!({
            "type": "array",
            "items": { "$ref": "#/components/schemas/StorageVolume" }
        }),
        ("post", "/api/v1/storage/volumes") | ("patch", "/api/v1/storage/volumes/{id}") => {
            json!({ "$ref": "#/components/schemas/StorageVolume" })
        }
        ("post", "/api/v1/storage/volumes/scan") => {
            json!({ "$ref": "#/components/schemas/VolumeScanReport" })
        }
        ("get", "/api/v1/storage/owners") => json!({
            "type": "array",
            "items": { "$ref": "#/components/schemas/VolumeOwner" }
        }),
        ("get", "/api/v1/storage/disks") => json!({
            "type": "array",
            "items": { "$ref": "#/components/schemas/DetectedDisk" }
        }),
        ("post", "/api/v1/launcher/auth/token") => {
            json!({ "$ref": "#/components/schemas/LauncherTokenResponse" })
        }
        ("post", path) if path.ends_with("/unlock") => json!({
            "type": "object",
            "required": ["ticket", "expiresAt"],
            "properties": {
                "ticket": { "type": "string", "description": "Короткоживущий HMAC-тикет скачивания." },
                "expiresAt": { "type": "string", "format": "date-time" }
            }
        }),
        ("get", "/api/v1/ws") => json!({ "$ref": "#/components/schemas/WebSocketConnected" }),
        _ => json!({}),
    }
}

fn success_response(operation: &Operation) -> Value {
    if operation.path == "/api/v1/openapi.json" {
        return json!({
            "description": "Спецификация OpenAPI 3.1",
            "content": { "application/json": { "schema": { "type": "object" } } }
        });
    }
    if operation.path == "/api/v1/docs" {
        return json!({
            "description": "Swagger UI (только при NEXUS_ENABLE_DOCS=true)",
            "content": { "text/html": { "schema": { "type": "string" } } }
        });
    }
    if is_binary_download(operation.method, operation.path) {
        let mut headers = json!({
            "X-Request-Id": { "$ref": "#/components/headers/RequestId" },
            "Content-Disposition": {
                "description": "Имя файла для скачивания.",
                "schema": { "type": "string" }
            },
            "Accept-Ranges": {
                "description": "Для файлов — `bytes`.",
                "schema": { "type": "string" }
            }
        });
        if is_launcher_rate_limited(operation) {
            headers["X-RateLimit-Limit"] = json!({ "$ref": "#/components/headers/RateLimitLimit" });
            headers["X-RateLimit-Remaining"] =
                json!({ "$ref": "#/components/headers/RateLimitRemaining" });
            headers["X-RateLimit-Reset"] = json!({ "$ref": "#/components/headers/RateLimitReset" });
        }
        return json!({
            "description": "Файл или фрагмент файла (поддерживается HTTP Range)",
            "headers": headers,
            "content": {
                "application/octet-stream": {
                    "schema": { "type": "string", "format": "binary" }
                },
                "image/png": { "schema": { "type": "string", "format": "binary" } },
                "image/jpeg": { "schema": { "type": "string", "format": "binary" } },
                "text/plain": { "schema": { "type": "string", "format": "binary" } }
            }
        });
    }
    let mut headers = json!({
        "X-Request-Id": { "$ref": "#/components/headers/RequestId" }
    });
    if is_launcher_rate_limited(operation) {
        headers["X-RateLimit-Limit"] = json!({ "$ref": "#/components/headers/RateLimitLimit" });
        headers["X-RateLimit-Remaining"] =
            json!({ "$ref": "#/components/headers/RateLimitRemaining" });
        headers["X-RateLimit-Reset"] = json!({ "$ref": "#/components/headers/RateLimitReset" });
    }
    json!({
        "description": "Успешный ответ",
        "headers": headers,
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "required": ["success", "data"],
                    "properties": {
                        "success": { "type": "boolean", "enum": [true] },
                        "data": response_data_schema(operation)
                    }
                }
            }
        }
    })
}

fn operation_responses(operation: &Operation) -> Value {
    let is_login = matches!(
        operation.path,
        "/api/v1/auth/login" | "/api/v1/launcher/auth/token"
    );
    let public_share_download = operation.path.starts_with("/api/v1/public/shares/")
        && operation.path.ends_with("/download");
    let public_share_unlock =
        operation.path.starts_with("/api/v1/public/shares/") && operation.path.ends_with("/unlock");
    let public_share = operation.path.starts_with("/api/v1/public/shares/");
    let mut responses = serde_json::Map::new();
    if operation.path == "/api/v1/ws" {
        responses.insert(
            "101".to_owned(),
            json!({
                "description": "Переключение протокола на WebSocket",
                "headers": {
                    "X-Request-Id": { "$ref": "#/components/headers/RequestId" }
                }
            }),
        );
    } else {
        responses.insert("200".to_owned(), success_response(operation));
    }

    if operation.session || operation.launcher || is_login {
        responses.insert(
            "401".to_owned(),
            json!({ "$ref": "#/components/responses/Unauthorized" }),
        );
        responses.insert(
            "403".to_owned(),
            json!({ "$ref": "#/components/responses/Forbidden" }),
        );
        responses.insert(
            "422".to_owned(),
            json!({ "$ref": "#/components/responses/ValidationError" }),
        );
    }
    if public_share {
        responses.insert(
            "401".to_owned(),
            json!({
                "description": "Ссылка создана с authOnly: требуется действующая cookie-сессия",
                "content": {
                    "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } }
                }
            }),
        );
    }
    if public_share_download || public_share_unlock {
        responses.insert(
            "403".to_owned(),
            json!({ "$ref": "#/components/responses/Forbidden" }),
        );
        responses.insert(
            "410".to_owned(),
            json!({
                "description": "Лимит скачиваний по ссылке исчерпан (DOWNLOAD_LIMIT_REACHED)",
                "content": {
                    "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } }
                }
            }),
        );
    }
    if public_share_unlock {
        responses.insert(
            "429".to_owned(),
            json!({ "$ref": "#/components/responses/TooManyRequests" }),
        );
    }
    if operation.path.contains('{') {
        responses.insert(
            "404".to_owned(),
            json!({ "$ref": "#/components/responses/NotFound" }),
        );
    }
    if is_mutating(operation.method) || operation.path.contains("/uploads") {
        responses.insert(
            "409".to_owned(),
            json!({ "$ref": "#/components/responses/Conflict" }),
        );
    }
    if operation.path.contains("/upload")
        || operation.path.ends_with("/icon")
        || operation.path.ends_with("/replace")
    {
        responses.insert(
            "413".to_owned(),
            json!({ "$ref": "#/components/responses/PayloadTooLarge" }),
        );
    }
    if is_login || is_launcher_rate_limited(operation) {
        responses.insert(
            "429".to_owned(),
            json!({ "$ref": "#/components/responses/TooManyRequests" }),
        );
    }
    if operation.method == "get" && operation.path.starts_with("/api/v1/launcher/artifacts/") {
        responses.insert(
            "503".to_owned(),
            json!({ "$ref": "#/components/responses/ServiceUnavailable" }),
        );
    }
    if operation.path.contains("/auth/") {
        responses.insert(
            "423".to_owned(),
            json!({ "$ref": "#/components/responses/Locked" }),
        );
    }
    responses.insert(
        "500".to_owned(),
        json!({ "$ref": "#/components/responses/InternalError" }),
    );
    responses.insert(
        "default".to_owned(),
        json!({ "$ref": "#/components/responses/Error" }),
    );
    Value::Object(responses)
}

/// E8: маршруты, которые принимают `Idempotency-Key`.
fn supports_idempotency(operation: &Operation) -> bool {
    matches!(
        (operation.method, operation.path),
        ("post", "/api/v1/cloud/uploads/{id}/complete")
            | ("post", "/api/v1/minecraft/uploads/{id}/complete")
            | ("post", "/api/v1/cloud/shares")
            | ("post", "/api/v1/cloud/exports")
            | ("patch", "/api/v1/cloud/entries/{id}")
    )
}

fn operation_parameters(operation: &Operation) -> Vec<Value> {
    let mut parameters = path_parameters(operation.path);

    if operation.session && is_mutating(operation.method) {
        parameters.push(json!({ "$ref": "#/components/parameters/CsrfToken" }));
    }
    if supports_idempotency(operation) {
        parameters.push(json!({ "$ref": "#/components/parameters/IdempotencyKey" }));
    }
    if operation.method == "patch" && operation.path.contains("/uploads/{id}") {
        parameters.push(json!({ "$ref": "#/components/parameters/UploadOffset" }));
    }

    let mut queries: Vec<Value> = Vec::new();
    match (operation.method, operation.path) {
        ("get", "/api/v1/cloud/entries") => {
            queries = vec![
                query_parameter(
                    "page",
                    "Страница, начиная с 1.",
                    json!({ "type": "integer", "minimum": 1, "default": 1 }),
                ),
                query_parameter(
                    "perPage",
                    "Записей на страницу, 1..100.",
                    json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 25 }),
                ),
                query_parameter(
                    "parentId",
                    "UUID папки; отсутствует для корня.",
                    json!({ "type": "string", "format": "uuid" }),
                ),
                query_parameter(
                    "q",
                    "Поиск по имени (ILIKE, без учёта регистра).",
                    json!({ "type": "string" }),
                ),
                query_parameter(
                    "sort",
                    "Поле сортировки: name, size, updated, type.",
                    json!({ "type": "string", "enum": ["name", "size", "updated", "type"], "default": "name" }),
                ),
                query_parameter(
                    "direction",
                    "Направление сортировки.",
                    json!({ "type": "string", "enum": ["asc", "desc"], "default": "asc" }),
                ),
                query_parameter(
                    "view",
                    "Представление: folder, trash, starred, recent, all.",
                    json!({ "type": "string", "enum": ["folder", "trash", "starred", "recent", "all"], "default": "folder" }),
                ),
                query_parameter(
                    "entryType",
                    "Фильтр типа записи.",
                    json!({ "type": "string", "enum": ["file", "folder"] }),
                ),
            ];
        }
        ("get", "/api/v1/users") => {
            queries = vec![
                query_parameter(
                    "page",
                    "Страница, начиная с 1.",
                    json!({ "type": "integer", "minimum": 1, "default": 1 }),
                ),
                query_parameter(
                    "perPage",
                    "Пользователей на страницу, 1..100.",
                    json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 25 }),
                ),
                query_parameter(
                    "q",
                    "Поиск по логину и отображаемому имени.",
                    json!({ "type": "string" }),
                ),
                query_parameter(
                    "status",
                    "Статус учётной записи.",
                    json!({ "type": "string", "enum": ["active", "disabled"] }),
                ),
            ];
        }
        ("get", "/api/v1/minecraft/builds") => {
            queries = vec![
                query_parameter(
                    "page",
                    "Страница, начиная с 1.",
                    json!({ "type": "integer", "minimum": 1, "default": 1 }),
                ),
                query_parameter(
                    "perPage",
                    "Сборок на страницу, 1..100.",
                    json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 24 }),
                ),
                query_parameter(
                    "q",
                    "Поиск по названию и slug.",
                    json!({ "type": "string" }),
                ),
                query_parameter(
                    "status",
                    "Статус сборки.",
                    json!({ "type": "string", "enum": ["draft", "published", "archived"] }),
                ),
                query_parameter(
                    "visibility",
                    "Видимость сборки.",
                    json!({ "type": "string", "enum": ["private", "public"] }),
                ),
                query_parameter(
                    "loader",
                    "Тип загрузчика: vanilla, fabric, forge, neoforge, quilt.",
                    json!({ "type": "string" }),
                ),
                query_parameter("category", "Категория сборки.", json!({ "type": "string" })),
                query_parameter(
                    "sort",
                    "Поле сортировки: updated, name, created, size, files.",
                    json!({ "type": "string", "enum": ["updated", "name", "created", "size", "files"], "default": "updated" }),
                ),
                query_parameter(
                    "direction",
                    "Направление сортировки.",
                    json!({ "type": "string", "enum": ["asc", "desc"], "default": "desc" }),
                ),
            ];
        }
        ("get", "/api/v1/system/audit") => {
            queries = vec![
                query_parameter(
                    "page",
                    "Страница, начиная с 1.",
                    json!({ "type": "integer", "minimum": 1, "default": 1 }),
                ),
                query_parameter(
                    "perPage",
                    "Событий на страницу, 1..100.",
                    json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 25 }),
                ),
                query_parameter(
                    "eventType",
                    "Точное имя типа события.",
                    json!({ "type": "string" }),
                ),
            ];
        }
        ("get", "/api/v1/jobs") => {
            queries = vec![
                query_parameter(
                    "status",
                    "Статус задачи.",
                    json!({ "type": "string", "enum": ["pending", "running", "paused", "completed", "failed", "cancelled"] }),
                ),
                query_parameter(
                    "jobType",
                    "Тип задачи, например cloud.export или storage.check.",
                    json!({ "type": "string" }),
                ),
                query_parameter(
                    "limit",
                    "Максимум записей, 1..200.",
                    json!({ "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }),
                ),
            ];
        }
        ("get", "/api/v1/monitoring/history") => {
            queries = vec![query_parameter(
                "window",
                "Окно истории: 5m, 15m, 1h, 24h, 7d.",
                json!({ "type": "string", "enum": ["5m", "15m", "1h", "24h", "7d"], "default": "5m" }),
            )];
        }
        _ => {}
    }
    parameters.extend(queries);
    parameters
}

fn operation_request_body(operation: &Operation) -> Option<Value> {
    let json_body = |schema: Value| {
        Some(json!({
            "required": true,
            "content": { "application/json": { "schema": schema } }
        }))
    };
    let binary_body = || {
        Some(json!({
            "required": true,
            "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } }
        }))
    };

    match (operation.method, operation.path) {
        ("post", "/api/v1/auth/login") => json_body(json!({
            "type": "object",
            "required": ["username", "password"],
            "properties": {
                "username": { "type": "string" },
                "password": { "type": "string", "format": "password" }
            }
        })),
        ("post", "/api/v1/launcher/auth/token") => {
            json_body(json!({ "$ref": "#/components/schemas/LauncherTokenRequest" }))
        }
        ("post", "/api/v1/cloud/uploads") => {
            json_body(json!({ "$ref": "#/components/schemas/UploadCreateRequest" }))
        }
        ("post", "/api/v1/minecraft/uploads") => json_body(json!({
            "$ref": "#/components/schemas/MinecraftUploadCreateRequest"
        })),
        ("post", "/api/v1/storage/volumes") => json_body(json!({
            "$ref": "#/components/schemas/StorageVolumeCreateRequest"
        })),
        ("post", "/api/v1/storage/volumes/scan") => json_body(json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Абсолютный локальный каталог, который проверяется перед регистрацией."
                }
            }
        })),
        ("patch", "/api/v1/storage/volumes/{id}") => json_body(json!({
            "$ref": "#/components/schemas/StorageVolumeUpdateRequest"
        })),
        ("post", path) if path.ends_with("/complete") && path.contains("/cloud/uploads/") => {
            json_body(json!({ "$ref": "#/components/schemas/CompleteUploadRequest" }))
        }
        ("post", path) if path.ends_with("/unlock") => json_body(json!({
            "type": "object",
            "properties": {
                "password": {
                    "type": "string",
                    "format": "password",
                    "description": "Пароль ссылки; пусто, если ссылка без пароля. Передаётся только в теле."
                }
            }
        })),
        ("put", path) if path.ends_with("/icon") => binary_body(),
        ("post", path) if path.ends_with("/replace") => binary_body(),
        ("patch", path) if path.contains("/uploads/{id}") => binary_body(),
        ("post", path)
            if path.ends_with("/ai/conversations/{id}/attachments")
                || path.ends_with("/ai/admin/skills/import") =>
        {
            Some(json!({
                "required": true,
                "content": {
                    "multipart/form-data": {
                        "schema": {
                            "type": "object",
                            "required": ["file"],
                            "properties": {
                                "file": { "type": "string", "format": "binary" }
                            }
                        }
                    }
                }
            }))
        }
        (method, _) if is_mutating(method) => json_body(json!({ "type": "object" })),
        _ => None,
    }
}

fn operation_object(operation: &Operation) -> Value {
    let security: Value = if operation.launcher {
        json!([{ "bearerAuth": [] }])
    } else if operation.session {
        json!([{ "cookieAuth": [] }])
    } else if operation.path.ends_with("/ai/runtime/connect") {
        json!([{ "bearerAuth": [] }])
    } else {
        json!([])
    };
    let mut object = json!({
        "summary": operation.summary,
        "tags": [operation.tag],
        "security": security,
        "responses": operation_responses(operation),
    });
    let parameters = operation_parameters(operation);
    if !parameters.is_empty() {
        object["parameters"] = json!(parameters);
    }
    if let Some(body) = operation_request_body(operation) {
        object["requestBody"] = body;
    }
    if operation.path == "/api/v1/ws" {
        object["description"] = json!(
            "WebSocket-канал прогресса. Авторизация — cookie nexus_session; \
             токен лаунчера не принимается. Сервер присылает текстовые JSON-кадры, \
             клиент может отправлять только Ping/Pong/Close."
        );
        object["x-websocket"] = json!({
            "auth": "cookie nexus_session",
            "frame": "text/json",
            "heartbeatSeconds": 30,
            "maxConnectionsPerUser": 3,
            "events": [
                "connected",
                "upload.progress",
                "upload.completed",
                "upload.failed",
                "download.progress",
                "download.completed",
                "download.failed",
                "build.validation.completed",
                "build.published",
                "build.publish.failed",
                "storage.volume.updated",
                "job.progress",
                "job.paused",
                "job.resumed",
                "job.completed",
                "job.failed",
                "job.cancelled",
                "system.metrics.updated",
                "ai.message.started",
                "ai.message.delta",
                "ai.message.completed",
                "ai.message.failed",
                "ai.task.queued",
                "ai.task.started",
                "ai.task.progress",
                "ai.tool.started",
                "ai.tool.progress",
                "ai.tool.waiting_approval",
                "ai.tool.completed",
                "ai.tool.failed",
                "ai.task.completed",
                "ai.task.failed",
                "ai.task.cancelled",
                "ai.runtime.connected",
                "ai.runtime.disconnected",
                "lagged",
                "closed"
            ]
        });
    }
    if operation.path == "/api/v1/ai/runtime/connect" {
        object["description"] = json!(
            "Единственный управляющий канал NEXUS AI Runtime. Устройство само \
             устанавливает исходящее WSS-соединение и аутентифицируется device token \
             (Authorization: Bearer nxa_…). Входящие порты на компьютере Runtime не открываются. \
             Контракт сообщений версионирован: protocolVersion, messageId, type, timestamp, \
             nodeId, correlationId, sequence, payload."
        );
        object["x-websocket"] = json!({
            "auth": "bearer device token",
            "protocolVersion": 1,
            "frame": "text/json",
            "heartbeatSeconds": 20,
            "maxConnectionsPerNode": 1,
        });
    }
    object
}

/// Собирает спецификацию OpenAPI 3.1.
pub fn specification(version: &str) -> Value {
    let mut paths: serde_json::Map<String, Value> = serde_json::Map::new();
    for operation in OPERATIONS.iter() {
        let entry = paths
            .entry(operation.path.to_owned())
            .or_insert_with(|| json!({}));
        entry[operation.method] = operation_object(operation);
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "NEXUS WORKSPACE API",
            "description": "Модульная платформа: Minecraft Build Manager, Cloud Storage и NEXUS AI Workspace. \
                            Все ответы обёрнуты в { success, data } или { success, error }; \
                            ошибки содержат код, безопасное сообщение и request_id. \
                            Браузер использует cookie nexus_session + X-CSRF-Token, \
                            Runtime — отдельный device token, лаунчер — Bearer-токен nxl_….",
            "version": version,
        },
        "servers": [{ "url": "/" }],
        "tags": [
            { "name": "Auth", "description": "Авторизация и сессии" },
            { "name": "Users", "description": "Пользователи и квоты" },
            { "name": "Roles", "description": "Роли и гранулярные права" },
            { "name": "Settings", "description": "Пользовательские и системные настройки" },
            { "name": "System", "description": "Состояние системы и модули" },
            { "name": "Storage", "description": "Диски и тома хранения" },
            { "name": "Cloud", "description": "Облачное хранилище" },
            { "name": "Public", "description": "Публичные ссылки" },
            { "name": "Minecraft", "description": "Сборки, файлы и релизы" },
            { "name": "Launcher", "description": "API для клиента лаунчера" },
            { "name": "Monitoring", "description": "Метрики и история" },
            { "name": "Jobs", "description": "Фоновые задачи" },
            { "name": "Realtime", "description": "WebSocket-события" },
            { "name": "AI", "description": "AI Chat, диалоги и Agent задачи" },
            { "name": "AI Admin", "description": "Runtime, провайдеры, workspace, Skills и политики" },
            { "name": "AI Runtime", "description": "Регистрация и исходящий канал NEXUS AI Runtime" },
        ],
        "components": {
            "securitySchemes": {
                "cookieAuth": {
                    "type": "apiKey",
                    "in": "cookie",
                    "name": "nexus_session",
                    "description": "Серверная сессия браузера (HttpOnly, SameSite=Lax).",
                },
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Токен лаунчера nxl_… в заголовке Authorization или X-Launcher-Token.",
                },
            },
            "parameters": {
                "CsrfToken": {
                    "name": "X-CSRF-Token",
                    "in": "header",
                    "required": true,
                    "schema": { "type": "string" },
                    "description": "Обязателен для изменяющих запросов браузера (POST/PUT/PATCH/DELETE). Значение приходит в GET /auth/me.",
                },
                "IdempotencyKey": {
                    "name": "Idempotency-Key",
                    "in": "header",
                    "required": false,
                    "schema": { "type": "string", "maxLength": 255, "pattern": "^[A-Za-z0-9_.:-]+$" },
                    "description": "Повторный запрос с тем же ключом и телом возвращает сохранённый результат и не меняет состояние.",
                },
                "UploadOffset": {
                    "name": "Upload-Offset",
                    "in": "header",
                    "required": true,
                    "schema": { "type": "integer", "minimum": 0 },
                    "description": "Смещение чанка в файле. Должно совпадать с текущим offset сессии.",
                },
            },
            "headers": {
                "RequestId": {
                    "description": "Идентификатор запроса; также возвращается в теле ошибки и заголовке X-Request-Id.",
                    "schema": { "type": "string" },
                },
                "RateLimitLimit": {
                    "description": "Ёмкость корзины запросов лаунчера (X-RateLimit-Limit).",
                    "schema": { "type": "integer" },
                },
                "RateLimitRemaining": {
                    "description": "Остаток запросов в корзине (X-RateLimit-Remaining).",
                    "schema": { "type": "integer" },
                },
                "RateLimitReset": {
                    "description": "Секунды до восстановления одного токена (X-RateLimit-Reset).",
                    "schema": { "type": "integer" },
                },
                "RetryAfter": {
                    "description": "Секунды до повторной попытки (Retry-After).",
                    "schema": { "type": "integer", "minimum": 1 },
                },
            },
            "responses": {
                "Error": error_response("Ошибка API"),
                "Unauthorized": error_response("Не аутентифицирован: сессия истекла или токен недействителен."),
                "Forbidden": error_response("Недостаточно прав, CSRF-проверка не пройдена или модуль отключён."),
                "NotFound": error_response("Ресурс не найден."),
                "Conflict": error_response("Конфликт состояния: имя занято, смещение загрузки не совпало, операция уже выполняется."),
                "PayloadTooLarge": error_response("Файл или чанк превышает лимит, либо исчерпана квота."),
                "ValidationError": error_response("Некорректные параметры, тело запроса или значение поля."),
                "Locked": error_response("Учётная запись временно заблокирована после неудачных попыток входа."),
                "TooManyRequests": {
                    "description": "Превышен лимит запросов.",
                    "headers": {
                        "X-Request-Id": { "$ref": "#/components/headers/RequestId" },
                        "Retry-After": { "$ref": "#/components/headers/RetryAfter" },
                        "X-RateLimit-Limit": { "$ref": "#/components/headers/RateLimitLimit" },
                        "X-RateLimit-Remaining": { "$ref": "#/components/headers/RateLimitRemaining" },
                        "X-RateLimit-Reset": { "$ref": "#/components/headers/RateLimitReset" }
                    },
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ApiError" }
                        }
                    }
                },
                "ServiceUnavailable": {
                    "description": "Исчерпан лимит параллельных скачиваний (DOWNLOAD_LIMIT_REACHED).",
                    "headers": {
                        "X-Request-Id": { "$ref": "#/components/headers/RequestId" },
                        "Retry-After": { "$ref": "#/components/headers/RetryAfter" }
                    },
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ApiError" }
                        }
                    }
                },
                "InsufficientStorage": error_response("Недостаточно места на выбранном томе."),
                "InternalError": error_response("Внутренняя ошибка сервера."),
            },
            "schemas": {
                "ApiError": {
                    "type": "object",
                    "required": ["success", "error", "request_id"],
                    "properties": {
                        "success": { "type": "boolean", "enum": [false] },
                        "error": {
                            "type": "object",
                            "required": ["code", "message"],
                            "properties": {
                                "code": { "type": "string", "description": "Машиночитаемый код, например FILE_NAME_CONFLICT." },
                                "message": { "type": "string", "description": "Безопасное сообщение для пользователя." },
                                "details": {
                                    "type": ["object", "null"],
                                    "additionalProperties": true,
                                    "description": "Структурированные детали; могут содержать offset, limitBytes, receivedBytes, required_permission и т. п."
                                }
                            }
                        },
                        "request_id": { "type": "string", "description": "Совпадает с заголовком X-Request-Id." }
                    }
                },
                "SuccessEnvelope": {
                    "type": "object",
                    "required": ["success", "data"],
                    "properties": {
                        "success": { "type": "boolean", "enum": [true] },
                        "data": {}
                    }
                },
                "StorageVolume": {
                    "type": "object",
                    "required": ["id", "name", "rootPath", "diskMount", "mode", "priority", "reservedBytes", "enabled", "state", "scope", "totalBytes", "availableBytes", "objects", "objectsBytes"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "name": { "type": "string" },
                        "rootPath": { "type": "string", "description": "Нормализованный корень: буква диска в верхнем регистре, без завершающего слэша." },
                        "diskMount": { "type": "string", "description": "Том ОС: буква диска или точка монтирования в папку." },
                        "mode": { "type": "string", "enum": ["auto", "manual"] },
                        "priority": { "type": "integer", "minimum": 0, "maximum": 1000 },
                        "reservedBytes": { "type": "integer", "minimum": 0 },
                        "maxBytes": { "type": ["integer", "null"] },
                        "enabled": { "type": "boolean" },
                        "state": { "type": "string", "enum": ["online", "offline", "error"] },
                        "stateMessage": { "type": "string" },
                        "totalBytes": { "type": "integer", "minimum": 0 },
                        "availableBytes": { "type": "integer", "minimum": 0 },
                        "lastCheckedAt": { "type": ["string", "null"], "format": "date-time" },
                        "createdAt": { "type": "string", "format": "date-time" },
                        "updatedAt": { "type": "string", "format": "date-time" },
                        "scope": {
                            "type": "string",
                            "enum": ["system", "cloud", "minecraft"],
                            "description": "system — общий том для обоих модулей; cloud/minecraft — только для своего модуля."
                        },
                        "ownerUserId": { "type": ["string", "null"], "format": "uuid", "description": "null — общий том." },
                        "ownerUsername": { "type": ["string", "null"] },
                        "objects": { "type": "integer", "minimum": 0 },
                        "objectsBytes": { "type": "integer", "minimum": 0 }
                    }
                },
                "StorageVolumeCreateRequest": {
                    "type": "object",
                    "required": ["name", "path"],
                    "properties": {
                        "name": { "type": "string", "minLength": 2, "maxLength": 64 },
                        "path": { "type": "string", "description": "Абсолютный локальный каталог. UNC и сетевые диски отклоняются." },
                        "mode": { "type": "string", "enum": ["auto", "manual"], "default": "auto" },
                        "priority": { "type": "integer", "minimum": 0, "maximum": 1000, "default": 100 },
                        "reservedBytes": { "type": "integer", "minimum": 0 },
                        "maxBytes": { "type": ["integer", "null"] },
                        "enabled": { "type": "boolean", "default": true },
                        "scope": { "type": "string", "enum": ["system", "cloud", "minecraft"], "default": "system" },
                        "ownerUserId": { "type": ["string", "null"], "format": "uuid", "description": "Персональный владелец; null — общий том." },
                        "adoptOrphans": {
                            "type": "boolean",
                            "default": false,
                            "description": "Импортировать найденные в каталоге объекты прежнего хранилища. Без подтверждения регистрация непустого `.nexus/objects` отклоняется."
                        }
                    }
                },
                "StorageVolumeUpdateRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 2, "maxLength": 64 },
                        "mode": { "type": "string", "enum": ["auto", "manual"] },
                        "priority": { "type": "integer", "minimum": 0, "maximum": 1000 },
                        "reservedBytes": { "type": "integer", "minimum": 0 },
                        "maxBytes": { "type": "integer", "minimum": 0, "description": "0 очищает лимит." },
                        "enabled": { "type": "boolean" },
                        "scope": { "type": "string", "enum": ["system", "cloud", "minecraft"] },
                        "ownerUserId": { "type": ["string", "null"], "format": "uuid", "description": "null делает том общим; отсутствие поля не меняет владельца." }
                    }
                },
                "VolumeOwner": {
                    "type": "object",
                    "required": ["id", "username", "displayName"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "username": { "type": "string" },
                        "displayName": { "type": "string" }
                    }
                },
                "DetectedDisk": {
                    "type": "object",
                    "required": ["mountPoint", "name", "fileSystem", "kind", "totalBytes", "availableBytes", "usedBytes", "removable", "system", "remote"],
                    "properties": {
                        "mountPoint": { "type": "string", "description": "Буква диска или точка монтирования в папку." },
                        "name": { "type": "string" },
                        "fileSystem": { "type": "string" },
                        "kind": { "type": "string", "enum": ["HDD", "SSD", "unknown"] },
                        "totalBytes": { "type": "integer", "minimum": 0 },
                        "availableBytes": { "type": "integer", "minimum": 0 },
                        "usedBytes": { "type": "integer", "minimum": 0 },
                        "removable": { "type": "boolean" },
                        "system": { "type": "boolean" },
                        "remote": { "type": "boolean", "description": "Сетевой диск; служба LocalSystem его не использует." }
                    }
                },
                "VolumeScanReport": {
                    "type": "object",
                    "required": ["path", "nexusPresent", "objectsFound", "objectsBytes", "stagingFiles", "stagingBytes", "quarantineFiles", "quarantineBytes", "foreignEntries", "skippedEntries", "truncated", "message"],
                    "properties": {
                        "path": { "type": "string" },
                        "nexusPresent": { "type": "boolean", "description": "В каталоге уже есть служебная папка `.nexus`." },
                        "objectsFound": { "type": "integer", "minimum": 0, "description": "Файлы в `.nexus/objects`, не учтённые в базе." },
                        "objectsBytes": { "type": "integer", "minimum": 0 },
                        "stagingFiles": { "type": "integer", "minimum": 0 },
                        "stagingBytes": { "type": "integer", "minimum": 0 },
                        "quarantineFiles": { "type": "integer", "minimum": 0 },
                        "quarantineBytes": { "type": "integer", "minimum": 0 },
                        "foreignEntries": { "type": "integer", "minimum": 0, "description": "Записи верхнего уровня, не относящиеся к `.nexus`." },
                        "skippedEntries": { "type": "integer", "minimum": 0, "description": "Ссылки, reparse points и нечитаемые записи." },
                        "truncated": { "type": "boolean", "description": "Обход ограничен предохранителем; регистрация с импортом блокируется." },
                        "message": { "type": "string" }
                    }
                },
                "Page": {
                    "type": "object",
                    "required": ["items", "page", "perPage", "total"],
                    "properties": {
                        "items": { "type": "array", "items": {} },
                        "page": { "type": "integer", "minimum": 1 },
                        "perPage": { "type": "integer", "minimum": 1, "maximum": 100 },
                        "total": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": true
                },
                "UploadCreateRequest": {
                    "type": "object",
                    "required": ["name", "size"],
                    "properties": {
                        "name": { "type": "string", "description": "Имя файла; проверяется на недопустимые символы и конфликт имён." },
                        "parentId": { "type": "string", "format": "uuid", "description": "Папка назначения; null — корень." },
                        "size": { "type": "integer", "minimum": 0, "description": "Заявленный размер в байтах." },
                        "contentType": { "type": "string", "default": "application/octet-stream" }
                    }
                },
                "UploadSession": {
                    "type": "object",
                    "required": ["id", "userId", "volumeId", "fileName", "contentType", "receivedBytes", "status", "createdAt", "expiresAt"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "userId": { "type": "string", "format": "uuid" },
                        "volumeId": { "type": "string", "format": "uuid" },
                        "parentId": { "type": ["string", "null"], "format": "uuid" },
                        "fileName": { "type": "string" },
                        "contentType": { "type": "string" },
                        "declaredSize": { "type": ["integer", "null"] },
                        "receivedBytes": { "type": "integer", "minimum": 0 },
                        "status": { "type": "string", "enum": ["active", "completed", "aborted", "expired"] },
                        "createdAt": { "type": "string", "format": "date-time" },
                        "expiresAt": { "type": "string", "format": "date-time" }
                    }
                },
                "MinecraftUploadCreateRequest": {
                    "type": "object",
                    "required": ["buildId", "path", "size"],
                    "properties": {
                        "buildId": { "type": "string", "format": "uuid" },
                        "path": { "type": "string", "description": "Относительный путь внутри сборки." },
                        "size": { "type": "integer", "minimum": 1 },
                        "mode": { "type": "string", "enum": ["create", "replace"], "default": "create" },
                        "category": { "type": "string" },
                        "side": { "type": "string", "enum": ["client", "server", "both"] },
                        "required": { "type": "boolean", "default": true },
                        "description": { "type": "string" },
                        "contentType": { "type": "string", "default": "application/octet-stream" }
                    }
                },
                "MinecraftUploadSession": {
                    "type": "object",
                    "required": ["id", "buildId", "userId", "volumeId", "targetPath", "declaredSize", "receivedBytes", "status", "createdAt", "expiresAt"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "buildId": { "type": "string", "format": "uuid" },
                        "userId": { "type": "string", "format": "uuid" },
                        "volumeId": { "type": "string", "format": "uuid" },
                        "targetPath": { "type": "string" },
                        "category": { "type": "string" },
                        "side": { "type": "string" },
                        "required": { "type": "boolean" },
                        "description": { "type": "string" },
                        "contentType": { "type": "string" },
                        "declaredSize": { "type": "integer" },
                        "receivedBytes": { "type": "integer", "minimum": 0 },
                        "status": { "type": "string", "enum": ["active", "completed", "aborted", "expired"] },
                        "createdAt": { "type": "string", "format": "date-time" },
                        "expiresAt": { "type": "string", "format": "date-time" }
                    }
                },
                "UploadStatus": {
                    "type": "object",
                    "required": ["id", "offset", "size", "status", "expiresAt", "chunkMaxBytes"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "offset": { "type": "integer", "minimum": 0 },
                        "size": { "type": ["integer", "null"], "description": "Заявленный размер файла." },
                        "status": { "type": "string" },
                        "expiresAt": { "type": "string", "format": "date-time" },
                        "chunkMaxBytes": { "type": "integer" }
                    }
                },
                "UploadChunkResponse": {
                    "type": "object",
                    "required": ["offset"],
                    "properties": {
                        "offset": { "type": "integer", "minimum": 0, "description": "Новое смещение после записи чанка." }
                    }
                },
                "CompleteUploadRequest": {
                    "type": "object",
                    "properties": {
                        "strategy": { "type": "string", "enum": ["auto", "replace", "rename"], "default": "auto" },
                        "newName": { "type": ["string", "null"], "description": "Новое имя при strategy=rename." }
                    }
                },
                "LauncherTokenRequest": {
                    "type": "object",
                    "required": ["name", "username", "password"],
                    "properties": {
                        "name": { "type": "string", "maxLength": 80, "description": "Имя устройства или клиента." },
                        "username": { "type": "string" },
                        "password": { "type": "string", "format": "password" },
                        "scopes": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["launcher.catalog", "launcher.artifacts"] },
                            "description": "По умолчанию оба scope."
                        },
                        "expiresInHours": { "type": "integer", "minimum": 1, "maximum": 8760, "default": 720 }
                    }
                },
                "LauncherTokenResponse": {
                    "type": "object",
                    "required": ["token", "tokenType", "scopes", "expiresAt"],
                    "properties": {
                        "token": { "type": "string", "description": "Секрет nxl_…; показывается один раз." },
                        "tokenType": { "type": "string", "enum": ["Bearer"] },
                        "scopes": { "type": "array", "items": { "type": "string" } },
                        "expiresAt": { "type": "string", "format": "date-time" }
                    }
                },
                "WebSocketConnected": {
                    "type": "object",
                    "required": ["kind", "userId", "system", "activeConnections"],
                    "properties": {
                        "kind": { "type": "string", "enum": ["connected"] },
                        "userId": { "type": "string", "format": "uuid" },
                        "system": { "type": "boolean", "description": "Доступны ли системные события." },
                        "activeConnections": { "type": "integer" }
                    }
                },
                "WebSocketEvent": {
                    "type": "object",
                    "required": ["kind", "payload", "at"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "upload.progress",
                                "upload.completed",
                                "upload.failed",
                                "download.progress",
                                "download.completed",
                                "download.failed",
                                "build.validation.completed",
                                "build.published",
                                "build.publish.failed",
                                "storage.volume.updated",
                                "job.progress",
                                "job.paused",
                                "job.resumed",
                                "job.completed",
                                "job.failed",
                                "job.cancelled",
                                "system.metrics.updated",
                                "ai.message.started",
                                "ai.message.delta",
                                "ai.message.completed",
                                "ai.message.failed",
                                "ai.task.queued",
                                "ai.task.started",
                                "ai.task.progress",
                                "ai.tool.started",
                                "ai.tool.progress",
                                "ai.tool.waiting_approval",
                                "ai.tool.completed",
                                "ai.tool.failed",
                                "ai.task.completed",
                                "ai.task.failed",
                                "ai.task.cancelled",
                                "ai.runtime.connected",
                                "ai.runtime.disconnected",
                                "lagged",
                                "closed"
                            ]
                        },
                        "userId": { "type": ["string", "null"], "format": "uuid", "description": "null для системных событий." },
                        "payload": {},
                        "at": { "type": "string", "format": "date-time" }
                    }
                }
            }
        },
        "paths": paths,
    })
}

/// Число описанных операций.
pub fn operation_count() -> usize {
    OPERATIONS.len()
}
