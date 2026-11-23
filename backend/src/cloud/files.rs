//! Чтение и скачивание файлов Cloud Storage.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use super::models::CloudEntry;
use super::service;
use crate::api::{ApiOk, Page, normalize_pagination};
use crate::core::error::{ApiError, ApiResult};
use crate::storage::engine::{self, ObjectLocation};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub parent_id: Option<Uuid>,
    pub q: Option<String>,
    pub sort: Option<String>,
    pub direction: Option<String>,
    pub view: Option<String>,
    pub entry_type: Option<String>,
    /// Категория файла для фильтра в списке (`image`, `video`, `audio`,
    /// `document`, `archive`, `other`). `None`/`all` — без фильтра.
    pub category: Option<String>,
    /// Глубина «Недавних» в днях; по умолчанию 30.
    pub since_days: Option<i64>,
}

/// Окно «Недавних» по умолчанию.
pub const RECENT_DAYS_DEFAULT: i64 = 30;

fn sort_column(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("name") {
        "size" => "e.size_bytes",
        "updated" => "e.updated_at",
        "type" => "e.entry_type",
        _ => "e.name",
    }
}

fn sort_direction(direction: Option<&str>) -> &'static str {
    match direction.unwrap_or("asc") {
        "desc" => "desc",
        _ => "asc",
    }
}

/// Условие категории. Фильтр серверный: `entry_type` отсекает папки, а
/// `content_type` (с запасным вариантом по расширению для файлов с
/// `application/octet-stream`) определяет категорию.
fn category_condition(category: Option<&str>) -> Option<&'static str> {
    match category.unwrap_or("all") {
        "image" => Some(
            "e.entry_type = 'file' and (e.content_type like 'image/%' or e.name ~* '\\.(png|jpe?g|webp|gif|bmp|avif|svg|tiff?|heic|heif)$')",
        ),
        "video" => Some(
            "e.entry_type = 'file' and (e.content_type like 'video/%' or e.name ~* '\\.(mp4|webm|mov|avi|mkv|m4v)$')",
        ),
        "audio" => Some(
            "e.entry_type = 'file' and (e.content_type like 'audio/%' or e.name ~* '\\.(mp3|wav|ogg|oga|flac|aac|m4a|opus)$')",
        ),
        "document" => Some(concat!(
            "e.entry_type = 'file' and (e.content_type like 'text/%' or e.content_type in ",
            "('application/pdf', 'application/json', 'application/xml', 'application/rtf', ",
            "'application/msword', ",
            "'application/vnd.openxmlformats-officedocument.wordprocessingml.document', ",
            "'application/vnd.ms-excel', ",
            "'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', ",
            "'application/vnd.ms-powerpoint', ",
            "'application/vnd.openxmlformats-officedocument.presentationml.presentation') or ",
            "e.name ~* '\\.(pdf|txt|log|md|csv|json|xml|yml|yaml|toml|ini|docx?|xlsx?|pptx?|rtf|odt|ods|odp)$')"
        )),
        "archive" => Some(concat!(
            "e.entry_type = 'file' and (e.content_type in ('application/zip', ",
            "'application/gzip', 'application/x-gzip', 'application/x-tar', ",
            "'application/x-7z-compressed', 'application/x-rar-compressed', ",
            "'application/x-bzip2', 'application/x-xz') or ",
            "e.name ~* '\\.(zip|rar|7z|tar|gz|tgz|bz2|xz)$')"
        )),
        "other" => Some(concat!(
            "e.entry_type = 'file' and not (",
            "(e.content_type like 'image/%' or e.name ~* '\\.(png|jpe?g|webp|gif|bmp|avif|svg|tiff?|heic|heif)$') or ",
            "(e.content_type like 'video/%' or e.name ~* '\\.(mp4|webm|mov|avi|mkv|m4v)$') or ",
            "(e.content_type like 'audio/%' or e.name ~* '\\.(mp3|wav|ogg|oga|flac|aac|m4a|opus)$') or ",
            "(e.content_type like 'text/%' or e.content_type in ('application/pdf', 'application/json', 'application/xml', 'application/rtf', 'application/msword', 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'application/vnd.ms-excel', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 'application/vnd.ms-powerpoint', 'application/vnd.openxmlformats-officedocument.presentationml.presentation') or e.name ~* '\\.(pdf|txt|log|md|csv|json|xml|yml|yaml|toml|ini|docx?|xlsx?|pptx?|rtf|odt|ods|odp)$') or ",
            "(e.content_type in ('application/zip', 'application/gzip', 'application/x-gzip', 'application/x-tar', 'application/x-7z-compressed', 'application/x-rar-compressed', 'application/x-bzip2', 'application/x-xz') or e.name ~* '\\.(zip|rar|7z|tar|gz|tgz|bz2|xz)$')",
            ")"
        )),
        _ => None,
    }
}

/// Список записей: папка, поиск, избранное, корзина или недавние.
pub async fn list_entries(
    db: &PgPool,
    owner_id: Uuid,
    query: &ListQuery,
) -> ApiResult<Page<CloudEntry>> {
    let (page, per_page, offset) = normalize_pagination(query.page, query.per_page);
    let view = query.view.as_deref().unwrap_or("folder");
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", value.replace('%', "\\%").replace('_', "\\_")));

    let order = sort_column(query.sort.as_deref());
    let direction = sort_direction(query.direction.as_deref());

    let (where_sql, binds_parent) = match view {
        "trash" => (
            "e.owner_id = $1 and e.trashed_at is not null".to_string(),
            false,
        ),
        "starred" => (
            "e.owner_id = $1 and e.trashed_at is null and f.entry_id is not null".to_string(),
            false,
        ),
        "recent" => {
            // E9: «Недавние» — только записи, изменённые за последние N дней.
            let days = query
                .since_days
                .unwrap_or(RECENT_DAYS_DEFAULT)
                .clamp(1, 3650);
            (
                format!(
                    "e.owner_id = $1 and e.trashed_at is null \
                     and e.updated_at >= now() - make_interval(days => {days})"
                ),
                false,
            )
        }
        "all" => (
            "e.owner_id = $1 and e.trashed_at is null and e.entry_type = 'file'".to_string(),
            false,
        ),
        _ => (
            "e.owner_id = $1 and e.trashed_at is null and e.parent_id is not distinct from $2"
                .to_string(),
            true,
        ),
    };

    let mut sql = format!(
        "select e.id, e.owner_id, e.parent_id, e.name, e.entry_type, e.size_bytes,
                e.content_type, coalesce(f.entry_id is not null, false) as starred,
                e.created_at, e.updated_at, e.trashed_at,
                u.username as owner_name
         from cloud_entries e
         left join cloud_favorites f on f.entry_id = e.id and f.user_id = e.owner_id
         join users u on u.id = e.owner_id
         where {where_sql}"
    );
    let mut count_sql = format!(
        "select count(*) from cloud_entries e
         left join cloud_favorites f on f.entry_id = e.id and f.user_id = e.owner_id
         where {where_sql}"
    );

    // Нумерация параметров зависит от того, привязан ли родитель: $1 — владелец,
    // $2 — родитель (только для просмотра папки), далее поиск и пагинация.
    let mut next_param = if binds_parent { 3 } else { 2 };

    if search.is_some() {
        let clause = format!(" and e.name ilike ${next_param} escape '\\'");
        next_param += 1;
        sql.push_str(&clause);
        count_sql.push_str(&clause);
    }
    if let Some(entry_type) = query
        .entry_type
        .as_deref()
        .filter(|value| matches!(*value, "file" | "folder"))
    {
        sql.push_str(" and e.entry_type = '");
        sql.push_str(entry_type);
        sql.push('\'');
        count_sql.push_str(" and e.entry_type = '");
        count_sql.push_str(entry_type);
        count_sql.push('\'');
    }
    if let Some(condition) = category_condition(query.category.as_deref()) {
        sql.push_str(" and ");
        sql.push_str(condition);
        count_sql.push_str(" and ");
        count_sql.push_str(condition);
    }

    // В «Недавних» по умолчанию сортируем по дате изменения (свежие сверху),
    // а не по имени. Остальные разделы сохраняют прежний порядок.
    let order_clause = if view == "recent" {
        let (order, direction) = if query.sort.is_none() {
            ("e.updated_at", "desc")
        } else {
            (order, direction)
        };
        format!(" order by {order} {direction}, e.id")
    } else {
        format!(" order by e.entry_type desc, {order} {direction}, e.id")
    };
    sql.push_str(&order_clause);
    sql.push_str(&format!(" limit ${next_param} offset ${}", next_param + 1));

    let mut items_query = sqlx::query_as::<_, CloudEntry>(&sql).bind(owner_id);
    if binds_parent {
        items_query = items_query.bind(query.parent_id);
    }
    if let Some(pattern) = &search {
        items_query = items_query.bind(pattern);
    }
    items_query = items_query.bind(per_page).bind(offset);
    let items = items_query.fetch_all(db).await?;

    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql).bind(owner_id);
    if binds_parent {
        count_query = count_query.bind(query.parent_id);
    }
    if let Some(pattern) = &search {
        count_query = count_query.bind(pattern);
    }
    let total = count_query.fetch_one(db).await?;

    Ok(Page::new(items, page, per_page, total))
}

/// Загружает объект и том для отдачи файла.
pub struct ResolvedObject {
    pub entry: CloudEntry,
    pub location: ObjectLocation,
}

pub async fn resolve_object(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
) -> ApiResult<ResolvedObject> {
    let entry = service::load_entry(db, owner_id, entry_id).await?;
    if entry.entry_type != "file" {
        return Err(ApiError::validation("Запись не является файлом."));
    }
    if entry.trashed_at.is_some() {
        return Err(ApiError::validation("Файл находится в корзине."));
    }
    let row: Option<(String, Uuid, String)> = sqlx::query_as(
        "select o.object_key, v.id, v.root_path
         from cloud_entries e
         join storage_objects o on o.id = e.object_id
         join storage_volumes v on v.id = o.volume_id
         where e.id = $1 and e.owner_id = $2 and o.state = 'committed'",
    )
    .bind(entry_id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?;

    let (key, volume_id, root_path) = row.ok_or_else(|| {
        ApiError::conflict(
            "OBJECT_UNAVAILABLE",
            "Файл недоступен: объект не найден или хранилище отключено.",
        )
    })?;

    let root = PathBuf::from(&root_path);
    let location = ObjectLocation {
        volume_id,
        key: key.clone(),
        absolute: ObjectLocation::objects_root(&root).join(&key),
        root,
    };
    Ok(ResolvedObject { entry, location })
}

/// Безопасный `Content-Disposition` с поддержкой UTF-8.
///
/// `filename` — ASCII-fallback для старых клиентов; `filename*` — имя в
/// кодировке UTF-8 по RFC 5987 (percent-encoding). Edge/Chrome выбирают
/// `filename*`, поэтому не-ASCII имя сохраняется без искажений.
fn content_disposition(name: &str) -> String {
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_filename(name),
        url_encode(name)
    )
}

/// То же для inline-предпросмотра: браузер показывает исходное имя файла.
fn inline_content_disposition(name: &str) -> String {
    format!(
        "inline; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_filename(name),
        url_encode(name)
    )
}

fn ascii_filename(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ' | '(' | ')') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Безопасный MIME для inline-просмотра. HTML/JS/SVG никогда не отдаются
/// с активным типом: HTML/SVG — только скачивание, JS и прочий текст —
/// `text/plain`. Тип берётся из сохранённого (определённого по содержимому)
/// `content_type`, расширение используется только как совместимый fallback
/// для старых объектов с `application/octet-stream`.
pub fn preview_content_type(entry: &CloudEntry) -> Option<&'static str> {
    const TEXT: &str = "text/plain; charset=utf-8";
    let ext = entry
        .name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    // Даже текстовый по содержимому файл с такими расширениями не должен
    // открываться inline: браузер не получит шанс интерпретировать разметку.
    if matches!(
        ext.as_str(),
        "html" | "htm" | "xhtml" | "shtml" | "svg" | "svgz"
    ) {
        return None;
    }

    let mime = entry
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        "image/bmp" => Some("image/bmp"),
        "image/avif" => Some("image/avif"),
        "video/mp4" => Some("video/mp4"),
        "video/webm" => Some("video/webm"),
        "audio/mpeg" => Some("audio/mpeg"),
        "audio/wav" | "audio/x-wav" => Some("audio/wav"),
        "audio/ogg" => Some("audio/ogg"),
        "audio/flac" | "audio/x-flac" => Some("audio/flac"),
        "application/pdf" => Some("application/pdf"),
        "text/plain" | "text/markdown" | "text/csv" | "application/json" => Some(TEXT),
        // Активная разметка: HTML/SVG — скачивание; скрипты — как текст.
        "text/html" | "application/xhtml+xml" | "image/svg+xml" => None,
        "text/javascript"
        | "application/javascript"
        | "application/x-javascript"
        | "text/ecmascript"
        | "application/ecmascript"
        | "text/xml"
        | "application/xml"
        | "text/x-shellscript" => Some(TEXT),
        "" | "application/octet-stream" => extension_preview_type(&ext),
        _ => None,
    }
}

/// Совместимый allowlist по расширению для объектов, загруженных до H1.
fn extension_preview_type(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" | "oga" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "pdf" => Some("application/pdf"),
        "txt" | "log" | "md" | "json" | "csv" | "yml" | "yaml" | "toml" | "ini" | "js" | "mjs"
        | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "sh" | "bash" | "sql" | "rs" | "java"
        | "properties" => Some("text/plain; charset=utf-8"),
        _ => None,
    }
}

/// Обёртка над читателем: считает переданные байты, публикует прогресс
/// не чаще заданного интервала и уменьшает счётчик активных скачиваний при
/// завершении или обрыве потока — счётчик не «залипает».
struct CountedReader<R> {
    inner: R,
    counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    progress: Box<DownloadProgress>,
}

/// Троттлинг прогресса: не чаще 4 раз/с; для длинных медиа — раз в 500 мс.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const LONG_MEDIA_INTERVAL: Duration = Duration::from_millis(500);
const LONG_MEDIA_BYTES: u64 = 16 * 1024 * 1024;

struct DownloadProgress {
    hub: crate::realtime::Hub,
    user_id: Uuid,
    entry_id: Uuid,
    name: String,
    size: u64,
    transferred: u64,
    last_emit: Instant,
    interval: Duration,
    completed: bool,
    failed: bool,
}

impl DownloadProgress {
    fn new(
        hub: crate::realtime::Hub,
        user_id: Uuid,
        entry_id: Uuid,
        name: &str,
        size: u64,
    ) -> Self {
        let mut progress = Self {
            hub,
            user_id,
            entry_id,
            name: name.to_owned(),
            size,
            transferred: 0,
            last_emit: Instant::now(),
            interval: if size >= LONG_MEDIA_BYTES {
                LONG_MEDIA_INTERVAL
            } else {
                PROGRESS_INTERVAL
            },
            completed: false,
            failed: false,
        };
        progress.emit("download.progress");
        progress
    }

    fn percent(&self) -> f64 {
        if self.size == 0 {
            100.0
        } else {
            ((self.transferred as f64 / self.size as f64) * 1000.0).round() / 10.0
        }
    }

    fn payload(&self) -> Value {
        json!({
            "entryId": self.entry_id,
            "name": self.name,
            "transferred": self.transferred,
            "size": self.size,
            "percent": self.percent(),
        })
    }

    fn emit(&mut self, kind: &str) {
        self.last_emit = Instant::now();
        self.hub.publish_user(kind, self.user_id, self.payload());
    }

    /// Учитывает прочитанные байты и публикует прогресс с троттлингом.
    fn record(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.transferred = self.transferred.saturating_add(bytes as u64);
        let finished = self.size > 0 && self.transferred >= self.size;
        if finished {
            self.transferred = self.size;
            self.completed = true;
            // Финальный 100% публикуется всегда, даже если интервал не истёк.
            self.emit("download.progress");
        } else if self.last_emit.elapsed() >= self.interval {
            self.emit("download.progress");
        }
    }

    /// Достигнут конец потока. Пустой файл тоже считается переданным полностью.
    fn eof(&mut self) {
        if self.size == 0 && !self.completed {
            self.completed = true;
            self.emit("download.progress");
        }
    }

    fn publish_failed(&mut self, code: &str, message: &str) {
        let mut payload = self.payload();
        payload["code"] = json!(code);
        payload["message"] = json!(message);
        self.hub
            .publish_user("download.failed", self.user_id, payload);
    }
}

impl<R> Drop for CountedReader<R> {
    fn drop(&mut self) {
        self.counter
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        if self.progress.completed {
            self.progress.emit("download.completed");
        } else if self.progress.failed {
            self.progress.publish_failed(
                "DOWNLOAD_FAILED",
                "Не удалось прочитать файл из хранилища. Повторите попытку.",
            );
        } else {
            // Обрыв до полной передачи: соединение закрылось или поток упал.
            self.progress.publish_failed(
                "DOWNLOAD_INTERRUPTED",
                "Скачивание прервано до завершения. Повторите попытку.",
            );
        }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for CountedReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        match std::pin::Pin::new(&mut self.inner).poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(())) => {
                let read = buf.filled().len().saturating_sub(before);
                if read == 0 {
                    self.progress.eof();
                } else {
                    self.progress.record(read);
                }
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Ready(Err(error)) => {
                self.progress.failed = true;
                std::task::Poll::Ready(Err(error))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

/// Отдаёт файл потоком. Поддерживает HTTP Range для медиа и больших файлов.
pub async fn stream_file(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
    headers: &HeaderMap,
    inline: bool,
) -> ApiResult<Response> {
    stream_file_with_state(None, db, owner_id, entry_id, headers, inline).await
}

/// Вариант с доступом к `AppState`: ведёт счётчики и публикует события realtime.
pub async fn stream_file_with_state(
    state: Option<&crate::state::AppState>,
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
    headers: &HeaderMap,
    inline: bool,
) -> ApiResult<Response> {
    let resolved = resolve_object(db, owner_id, entry_id).await?;
    let file = engine::open_object(&resolved.location).await?;
    let total = engine::stat_object(&resolved.location)
        .map_err(|_| ApiError::conflict("OBJECT_UNAVAILABLE", "Файл отсутствует в хранилище."))?;

    let preview_type = if inline {
        preview_content_type(&resolved.entry)
    } else {
        None
    };
    let content_type = preview_type.unwrap_or("application/octet-stream");

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_range(value, total));

    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0, must-revalidate"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Небезопасный или неизвестный тип при запросе предпросмотра отдаётся
    // как скачивание, а не inline.
    let disposition = match preview_type {
        Some(_) => inline_content_disposition(&resolved.entry.name),
        None => content_disposition(&resolved.entry.name),
    };
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        response_headers.insert(header::CONTENT_DISPOSITION, value);
    }

    let mut file = file;
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None => (0, total.saturating_sub(1), StatusCode::OK),
    };

    if range.is_some() {
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return Err(ApiError::internal("не удалось позиционировать файл"));
        }
        let length = end - start + 1;
        let reader: Box<dyn tokio::io::AsyncRead + Send + Unpin> = Box::new(file.take(length));
        let stream = tokio_util::io::ReaderStream::new(reader);
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
        );
        response_headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&length.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("0")),
        );
        return Ok((status, response_headers, Body::from_stream(stream)).into_response());
    }

    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&total.to_string()).unwrap_or_else(|_| HeaderValue::from_static("0")),
    );

    let reader: Box<dyn tokio::io::AsyncRead + Send + Unpin> = match state {
        Some(state) => {
            state.requests.begin_download();
            let progress = DownloadProgress::new(
                state.realtime.clone(),
                owner_id,
                entry_id,
                &resolved.entry.name,
                total,
            );
            Box::new(CountedReader {
                inner: file,
                counter: state.requests.download_counter(),
                progress: Box::new(progress),
            })
        }
        None => Box::new(file),
    };
    let stream = tokio_util::io::ReaderStream::new(reader);
    Ok((status, response_headers, Body::from_stream(stream)).into_response())
}

/// Отдаёт объект хранения потоком с поддержкой `Range`.
/// Общий хелпер для Cloud Storage и артефактов сборок.
pub async fn stream_object(
    location: &ObjectLocation,
    total: i64,
    headers: &HeaderMap,
    content_type: &'static str,
) -> ApiResult<Response> {
    crate::storage::http::stream_object(location, total, headers, content_type).await
}

/// Разбор `Range: bytes=start-end`. Некорректные и множественные диапазоны игнорируются.
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total == 0 {
        return None;
    }
    let (start_raw, end_raw) = spec.split_once('-')?;
    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        let suffix: u64 = end_raw.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix);
        return Some((start, total - 1));
    }

    let start: u64 = start_raw.parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_raw.is_empty() {
        total - 1
    } else {
        end_raw.parse::<u64>().ok()?.min(total - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

/// Текстовый предпросмотр: ограниченный объём, без исполнения содержимого.
pub async fn preview_text(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
    limit: usize,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let resolved = resolve_object(db, owner_id, entry_id).await?;
    if preview_content_type(&resolved.entry).is_none() {
        return Err(ApiError::validation(
            "Предпросмотр этого формата недоступен.",
        ));
    }
    let mut file = engine::open_object(&resolved.location).await?;
    let limit = limit.clamp(1024, 512 * 1024);
    let mut buffer = vec![0u8; limit];
    let read = file
        .read(&mut buffer)
        .await
        .map_err(|_| ApiError::internal("не удалось прочитать файл"))?;
    let truncated = resolved.entry.size_bytes as usize > read;
    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
    Ok(ApiOk(json!({
        "text": text,
        "truncated": truncated,
        "contentType": preview_content_type(&resolved.entry),
        "size": resolved.entry.size_bytes,
    })))
}

/// Метаданные файла для панели предпросмотра.
pub async fn entry_details(
    db: &PgPool,
    owner_id: Uuid,
    entry_id: Uuid,
) -> ApiResult<ApiOk<serde_json::Value>> {
    let entry = service::load_entry(db, owner_id, entry_id).await?;
    let breadcrumb = service::breadcrumb_path(db, &entry).await?;
    let shares: i64 = sqlx::query_scalar(
        "select count(*) from cloud_shares
         where entry_id = $1 and revoked_at is null
           and (expires_at is null or expires_at > now())",
    )
    .bind(entry_id)
    .fetch_one(db)
    .await?;
    Ok(ApiOk(json!({
        "entry": entry,
        "breadcrumb": breadcrumb
            .into_iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect::<Vec<_>>(),
        "preview": preview_content_type(&entry),
        "activeShares": shares,
    })))
}
