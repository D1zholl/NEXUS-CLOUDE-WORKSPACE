//! Публикация AI-событий в существующий realtime Hub.
//!
//! WebSocket — только уведомление. Источник истины всегда PostgreSQL.

use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

pub fn user(state: &AppState, owner_id: Uuid, kind: &str, payload: Value) {
    state.realtime.publish_user(kind, owner_id, payload);
}

pub fn system(state: &AppState, kind: &str, payload: Value) {
    state.realtime.publish_system(kind, payload);
}
