use axum::Json;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

pub mod openapi;

/// Успешный ответ API: `{ "success": true, "data": ... }`.
#[derive(Debug)]
pub struct ApiOk<T>(pub T);

impl<T: Serialize> IntoResponse for ApiOk<T> {
    fn into_response(self) -> Response {
        let data = serde_json::to_value(self.0).unwrap_or(Value::Null);
        Json(json!({ "success": true, "data": data })).into_response()
    }
}

/// Страница списка с серверной пагинацией.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, page: i64, per_page: i64, total: i64) -> Self {
        Self {
            items,
            page,
            per_page,
            total,
        }
    }
}

pub fn normalize_pagination(page: Option<i64>, per_page: Option<i64>) -> (i64, i64, i64) {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(25).clamp(1, 100);
    let offset = (page - 1) * per_page;
    (page, per_page, offset)
}
