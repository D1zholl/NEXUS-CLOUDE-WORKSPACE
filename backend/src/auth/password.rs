use std::sync::OnceLock;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use crate::core::error::{ApiError, ApiResult};

pub const MIN_PASSWORD_LEN: usize = 12;
pub const MAX_PASSWORD_LEN: usize = 128;
/// Допустимый диапазон настройки `security.password_min_length`.
pub const PASSWORD_MIN_ALLOWED: usize = 8;
pub const PASSWORD_MAX_ALLOWED: usize = 64;

pub fn validate_password(password: &str, min_len: usize) -> Result<(), ApiError> {
    let len = password.chars().count();
    if len < min_len {
        return Err(ApiError::validation(format!(
            "Пароль должен содержать не менее {min_len} символов."
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(ApiError::validation(format!(
            "Пароль должен содержать не более {MAX_PASSWORD_LEN} символов."
        )));
    }
    let has_letter = password.chars().any(|c| c.is_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    if !has_letter || !has_digit {
        return Err(ApiError::validation(
            "Пароль должен содержать буквы и цифры.",
        ));
    }
    Ok(())
}

fn hash_blocking(password: String) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_blocking(password: String, hash: String) -> bool {
    let Ok(parsed) = PasswordHash::new(&hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || hash_blocking(password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_blocking(password, hash))
        .await
        .unwrap_or(false)
}

/// Хеш-заглушка для выравнивания времени ответа при несуществующем пользователе.
pub fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_blocking("nexus-dummy-password-0000".into()).unwrap_or_else(|_| String::new())
    })
}

pub fn validate_username(username: &str) -> Result<(), ApiError> {
    let len = username.chars().count();
    if !(3..=32).contains(&len) {
        return Err(ApiError::validation(
            "Имя пользователя должно содержать от 3 до 32 символов.",
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ApiError::validation(
            "Имя пользователя может содержать только латинские буквы, цифры, точку, дефис и подчёркивание.",
        ));
    }
    Ok(())
}
