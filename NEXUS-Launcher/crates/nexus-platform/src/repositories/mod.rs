//! Small, explicit repositories.
//!
//! Repositories accept a `&SqlitePool` instead of owning it. This keeps the
//! database module free of domain logic and makes the functions easy to use
//! from services and integration tests.

pub mod accounts;
pub mod activity;
pub mod downloads;
pub mod instances;
pub mod mods;
pub mod sources;

use nexus_core::{Clock, NexusError};

pub(crate) fn now_ms(clock: &dyn Clock) -> i64 {
    clock.now_ms()
}

pub(crate) fn validate_name(value: &str, field: &str, max: usize) -> Result<String, NexusError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max {
        return Err(NexusError::invalid_argument(format!(
            "{field} must contain between 1 and {max} characters"
        )));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(NexusError::invalid_argument(format!(
            "{field} contains control characters"
        )));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn validate_loader(
    loader_type: &str,
    loader_version: Option<&str>,
) -> Result<(), NexusError> {
    let allowed = ["vanilla", "fabric", "quilt", "forge", "neoforge"];
    if !allowed.contains(&loader_type) {
        return Err(NexusError::invalid_argument(format!(
            "unsupported loader_type: {loader_type}"
        )));
    }
    match (loader_type, loader_version) {
        ("vanilla", None | Some("")) => Ok(()),
        ("vanilla", Some(_)) => Err(NexusError::invalid_argument(
            "vanilla instances must not have loader_version",
        )),
        (_, Some(version)) if !version.trim().is_empty() => Ok(()),
        (_, _) => Err(NexusError::invalid_argument(
            "non-vanilla loader requires loader_version",
        )),
    }
}

pub(crate) fn validate_sha256(value: Option<&str>, field: &str) -> Result<(), NexusError> {
    if let Some(value) = value
        && (value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return Err(NexusError::invalid_argument(format!(
            "{field} must be a 64-character hex string"
        )));
    }
    Ok(())
}

pub(crate) fn validate_sha1(value: Option<&str>, field: &str) -> Result<(), NexusError> {
    if let Some(value) = value
        && (value.len() != 40 || !value.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return Err(NexusError::invalid_argument(format!(
            "{field} must be a 40-character hex string"
        )));
    }
    Ok(())
}

pub(crate) fn validate_relative_path(value: &str) -> Result<String, NexusError> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty() || normalized.len() > 4096 {
        return Err(NexusError::invalid_path("relative path length is invalid"));
    }
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err(NexusError::invalid_path(
            "relative path must not be absolute",
        ));
    }
    if normalized.split('/').any(|part| part == "..") {
        return Err(NexusError::invalid_path(
            "relative path must not contain '..'",
        ));
    }
    if normalized.chars().any(|ch| ch.is_control()) {
        return Err(NexusError::invalid_path(
            "relative path contains control characters",
        ));
    }
    Ok(normalized)
}

pub(crate) fn path_key(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_consistency_is_enforced() {
        assert!(validate_loader("vanilla", None).is_ok());
        assert!(validate_loader("vanilla", Some("1")).is_err());
        assert!(validate_loader("fabric", Some("0.16.9")).is_ok());
        assert!(validate_loader("fabric", None).is_err());
        assert!(validate_loader("unknown", None).is_err());
    }

    #[test]
    fn relative_paths_cannot_escape() {
        assert_eq!(validate_relative_path("mods\\a.jar").unwrap(), "mods/a.jar");
        assert!(validate_relative_path("../a.jar").is_err());
        assert!(validate_relative_path("C:/a.jar").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
    }
}
