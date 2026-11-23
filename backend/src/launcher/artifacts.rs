//! Отдача артефактов лаунчеру: поток, Range, логирование загрузок.

use std::sync::Arc;

use axum::response::Response;
use sqlx::PgPool;
use uuid::Uuid;

use super::catalog::{self, ArtifactRef};
use crate::core::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::engine::{self, ObjectLocation};

/// Загружает объект и том для артефакта.
pub async fn resolve(db: &PgPool, artifact: &ArtifactRef) -> ApiResult<(ObjectLocation, i64)> {
    let row: Option<(String, Uuid, String, i64)> = sqlx::query_as(
        "select o.object_key, v.id, v.root_path, o.size_bytes
         from storage_objects o
         join storage_volumes v on v.id = o.volume_id
         where o.id = $1 and o.state = 'committed'",
    )
    .bind(artifact.object_id)
    .fetch_optional(db)
    .await?;
    let Some((key, volume_id, root_path, size)) = row else {
        return Err(ApiError::conflict(
            "OBJECT_UNAVAILABLE",
            "Артефакт недоступен в хранилище.",
        ));
    };
    let root = std::path::PathBuf::from(&root_path);
    let location = ObjectLocation {
        volume_id,
        absolute: ObjectLocation::objects_root(&root).join(&key),
        key,
        root,
    };
    // Размер отдаём из объекта хранения: он сверен при фиксации загрузки.
    let size = if size > 0 { size } else { artifact.size_bytes };
    Ok((location, size))
}

/// Отдаёт артефакт потоком с поддержкой `Range` и записывает факт загрузки.
pub async fn download(
    state: &Arc<AppState>,
    artifact_id: Uuid,
    token_id: Uuid,
    viewer_user_id: Uuid,
    ip: &str,
    user_agent: &str,
    headers: &axum::http::HeaderMap,
) -> ApiResult<Response> {
    let artifact = catalog::find_artifact(&state.db, artifact_id, viewer_user_id).await?;
    download_resolved(
        state,
        artifact_id,
        &artifact,
        Some(token_id),
        ip,
        user_agent,
        headers,
    )
    .await
}

/// Отдача уже проверенного артефакта. `token_id = None` — публичный запрос
/// MyLauncher, который не передаёт токен.
pub async fn download_resolved(
    state: &Arc<AppState>,
    artifact_id: Uuid,
    artifact: &ArtifactRef,
    token_id: Option<Uuid>,
    ip: &str,
    user_agent: &str,
    headers: &axum::http::HeaderMap,
) -> ApiResult<Response> {
    let (location, total) = resolve(&state.db, artifact).await?;
    let range_header = headers
        .get(axum::http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();

    // Проверка целостности перед отдачей: файл должен существовать на томе.
    if engine::stat_object(&location).is_err() {
        return Err(ApiError::conflict(
            "OBJECT_UNAVAILABLE",
            "Файл артефакта отсутствует в хранилище.",
        ));
    }

    let response = crate::storage::http::stream_object_strict_ranges(
        &location,
        total,
        headers,
        "application/octet-stream",
    )
    .await?;

    let status = response.status().as_u16() as i32;
    let sent = response
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(total);

    let mut response = response;
    response.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            sanitize_filename(&artifact.path)
        ))
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&artifact.sha256) {
        response
            .headers_mut()
            .insert(axum::http::HeaderName::from_static("x-file-sha256"), value);
    }

    // Логирование загрузок: не блокирует ответ, ошибки записи не влияют на отдачу.
    let db = state.db.clone();
    let ip = ip.to_owned();
    let user_agent = user_agent.to_owned();
    let path = artifact.path.clone();
    let build_id = artifact.build_id;
    let release_id = artifact.release_id;
    tokio::spawn(async move {
        let _ = sqlx::query(
            "insert into launcher_downloads
               (token_id, build_id, release_id, artifact_id, path, ip, user_agent,
                bytes_sent, range_header, status)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(token_id)
        .bind(build_id)
        .bind(release_id)
        .bind(artifact_id)
        .bind(&path)
        .bind(&ip)
        .bind(&user_agent)
        .bind(sent)
        .bind(&range_header)
        .bind(status)
        .execute(&db)
        .await;
    });

    Ok(response)
}

fn sanitize_filename(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or("artifact")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '(' | ')') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(sanitize_filename("mods/example.jar"), "example.jar");
        // Каждый не-ASCII символ заменяется на одно подчёркивание.
        assert_eq!(sanitize_filename("config/й.json"), "_.json");
        assert_eq!(sanitize_filename("config/файл.json"), "____.json");
        assert_eq!(sanitize_filename("no-slash.bin"), "no-slash.bin");
    }
}
