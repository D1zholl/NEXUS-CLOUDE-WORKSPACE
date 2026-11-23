use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::auth::session::{AuthUser, SESSION_COOKIE, cookie_value, load_user_for_token};
use crate::core::error::ApiError;
use crate::state::AppState;

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token =
            cookie_value(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::session_expired)?;
        load_user_for_token(state, &token).await
    }
}
