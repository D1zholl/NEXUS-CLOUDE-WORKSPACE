//! Согласованность OpenAPI-спецификации с реализацией (PLAN §11).
//!
//! Тест читает исходники роутеров и сравнивает число зарегистрированных
//! операций с числом операций в спецификации. Если добавить маршрут и забыть
//! описать его, тест упадёт.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Каждый роутер описан парой: файл, имя функции-конструктора, префикс.
const ROUTERS: &[(&str, &str, &str)] = &[
    ("src/auth/routes.rs", "pub fn router", "/api/v1/auth"),
    ("src/cloud/routes.rs", "pub fn router", "/api/v1/cloud"),
    (
        "src/cloud/routes.rs",
        "pub fn public_router",
        "/api/v1/public/shares",
    ),
    (
        "src/launcher/routes.rs",
        "pub fn router",
        "/api/v1/launcher",
    ),
    (
        "src/launcher/routes.rs",
        "pub fn admin_router",
        "/api/v1/admin/launcher",
    ),
    ("src/launcher/repository.rs", "pub fn router", ""),
    (
        "src/minecraft/routes.rs",
        "pub fn router",
        "/api/v1/minecraft",
    ),
    (
        "src/permissions/routes.rs",
        "pub fn roles_router",
        "/api/v1/roles",
    ),
    (
        "src/permissions/routes.rs",
        "pub fn permissions_router",
        "/api/v1/permissions",
    ),
    ("src/storage/routes.rs", "pub fn router", "/api/v1/storage"),
    ("src/system/jobs_routes.rs", "pub fn router", "/api/v1/jobs"),
    (
        "src/system/monitoring_routes.rs",
        "pub fn router",
        "/api/v1/monitoring",
    ),
    ("src/system/routes.rs", "pub fn router", "/api/v1/system"),
    ("src/system/routes.rs", "pub fn openapi_router", "/api/v1"),
    ("src/system/settings.rs", "pub fn me_router", "/api/v1/me"),
    (
        "src/system/settings.rs",
        "pub fn public_router",
        "/api/v1/public/appearance",
    ),
    (
        "src/system/settings.rs",
        "pub fn system_router",
        "/api/v1/system",
    ),
    ("src/ai/routes.rs", "pub fn router", "/api/v1/ai"),
    (
        "src/ai/admin_routes.rs",
        "pub fn router",
        "/api/v1/ai/admin",
    ),
    ("src/users/routes.rs", "pub fn router", "/api/v1/users"),
];

/// Извлекает тело функции-конструктора роутера.
fn function_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let Some(start) = source.find(marker) else {
        return "";
    };
    let rest = &source[start..];
    // Тело заканчивается на следующем объявлении функции верхнего уровня.
    let end = rest[1..]
        .find("\npub fn ")
        .or_else(|| rest[1..].find("\nasync fn "))
        .or_else(|| rest[1..].find("\nfn "))
        .map(|index| index + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Грубый, но устойчивый разбор: считает вызовы `.route("path", ...)` и
/// методы в цепочке (`get`, `post`, `put`, `patch`, `delete`).
fn scan_routes(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(index) = rest.find(".route(") {
        let after = &rest[index + ".route(".len()..];
        let Some(quote_start) = after.find('"') else {
            break;
        };
        let after_quote = &after[quote_start + 1..];
        let Some(quote_end) = after_quote.find('"') else {
            break;
        };
        let path = &after_quote[..quote_end];
        let mut depth = 1usize;
        let mut end = 0usize;
        let bytes = after.as_bytes();
        for (position, byte) in bytes.iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = position;
                        break;
                    }
                }
                _ => {}
            }
        }
        let expression = &after[..end];
        for method in ["get", "post", "put", "patch", "delete"] {
            if expression.contains(&format!("{method}(")) {
                out.push((method.to_owned(), path.to_owned()));
            }
        }
        rest = &after[end..];
    }
    out
}

fn join_path(prefix: &str, path: &str) -> String {
    let normalized = if path == "/" { "" } else { path };
    format!("{}{}", prefix.trim_end_matches('/'), normalized)
}

/// Считает вызовы `.route(` во всех исходниках `backend/src`.
fn count_route_registrations(dir: &Path) -> usize {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += count_route_registrations(&path);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            let source = fs::read_to_string(&path).unwrap_or_default();
            total += source.matches(".route(").count();
        }
    }
    total
}

#[test]
fn specification_covers_every_route() {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for (file, marker, prefix) in ROUTERS {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("не удалось прочитать {}: {error}", path.display()));
        let body = function_body(&source, marker);
        assert!(!body.is_empty(), "не найдена функция {marker} в {file}");
        for (method, route_path) in scan_routes(body) {
            declared.insert(format!("{method} {}", join_path(prefix, &route_path)));
        }
    }

    let spec = nexus::api::openapi::specification("test");
    let mut described: BTreeSet<String> = BTreeSet::new();
    let paths = spec["paths"].as_object().expect("paths");
    for (path, item) in paths {
        for method in item.as_object().expect("path item").keys() {
            if matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                described.insert(format!("{method} {path}"));
            }
        }
    }

    // Маршруты, зарегистрированные в lib.rs вне отдельных роутеров.
    let mut normalized_declared = declared.clone();
    normalized_declared.insert("get /api/v1/health".to_owned());
    normalized_declared.insert("get /api/v1/ws".to_owned());

    let only_declared: Vec<&String> = normalized_declared.difference(&described).collect();
    assert!(
        only_declared.is_empty(),
        "маршруты без описания в OpenAPI: {only_declared:#?}"
    );

    let only_described: Vec<&String> = described.difference(&normalized_declared).collect();
    assert!(
        only_described.is_empty(),
        "спецификация описывает несуществующие маршруты: {only_described:#?}"
    );
}

#[test]
fn spec_is_valid_openapi_31() {
    let spec = nexus::api::openapi::specification("1.2.3");
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["version"], "1.2.3");
    assert!(
        spec["paths"]
            .as_object()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
    );
    assert!(spec["components"]["securitySchemes"]["cookieAuth"].is_object());
    assert!(spec["components"]["securitySchemes"]["bearerAuth"].is_object());

    let schemas = spec["components"]["schemas"].as_object().expect("schemas");
    for required in [
        "ApiError",
        "Page",
        "UploadSession",
        "UploadStatus",
        "UploadChunkResponse",
        "MinecraftUploadSession",
        "WebSocketEvent",
        "WebSocketConnected",
        "LauncherTokenRequest",
        "LauncherTokenResponse",
    ] {
        assert!(schemas.contains_key(required), "нет схемы {required}");
    }

    let responses = spec["components"]["responses"]
        .as_object()
        .expect("responses");
    for required in [
        "Unauthorized",
        "Forbidden",
        "NotFound",
        "Conflict",
        "ValidationError",
        "TooManyRequests",
        "InternalError",
    ] {
        assert!(responses.contains_key(required), "нет ответа {required}");
    }

    let websocket = &spec["paths"]["/api/v1/ws"]["get"];
    assert_eq!(websocket["x-websocket"]["frame"], "text/json");
    assert!(websocket["responses"]["101"].is_object());

    let token = &spec["paths"]["/api/v1/launcher/auth/token"]["post"];
    assert!(
        token["responses"]["429"].is_object(),
        "нет 429 у токена лаунчера"
    );
    assert!(
        token["responses"]["423"].is_object(),
        "нет 423 у блокировки"
    );
    let public_download = &spec["paths"]["/api/v1/public/shares/{token}/download"]["get"];
    assert!(
        public_download["responses"]["403"].is_object(),
        "нет 403 у публичной ссылки с паролем"
    );
}

/// CI-проверка из A12: число шаблонов путей в спецификации совпадает
/// с числом зарегистрированных `.route(` в исходниках.
#[test]
fn path_count_matches_route_registrations() {
    let spec = nexus::api::openapi::specification("test");
    let paths = spec["paths"].as_object().expect("paths").len();
    let registrations =
        count_route_registrations(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    assert_eq!(
        paths, registrations,
        "число путей в OpenAPI ({paths}) не совпадает с числом .route( ({registrations})"
    );
}

#[test]
fn unique_operations() {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for operation in nexus::api::openapi::OPERATIONS.iter() {
        let key = format!("{} {}", operation.method, operation.path);
        assert!(seen.insert(key.clone()), "дубликат операции: {key}");
    }
}

#[test]
fn path_parameters_are_declared() {
    let spec = nexus::api::openapi::specification("test");
    for (path, item) in spec["paths"].as_object().unwrap() {
        let expected: Vec<&str> = path
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
            .collect();
        if expected.is_empty() {
            continue;
        }
        for (method, operation) in item.as_object().unwrap() {
            let params = operation["parameters"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for name in &expected {
                let found = params
                    .iter()
                    .any(|param| param["in"] == "path" && param["name"] == *name);
                assert!(found, "{method} {path}: не описан path-параметр {name}");
            }
        }
    }
}
