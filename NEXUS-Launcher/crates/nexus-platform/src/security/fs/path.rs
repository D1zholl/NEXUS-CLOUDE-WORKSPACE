//! Validation of paths that are relative to a [`crate::security::fs::SafeFs`] root.
//!
//! Validation is intentionally stricter than the operating system. A path is
//! accepted only if every component is a plain, portable file name. This is the
//! *first* layer: the resolver still checks the real filesystem for reparse
//! points and canonical containment before any I/O happens.

use std::path::{Component, Path, PathBuf};

use nexus_core::NexusError;

const MAX_PATH_CHARS: usize = 32_767;
const MAX_COMPONENT_CHARS: usize = 255;

/// A path that has been validated as a relative, non-escaping path.
///
/// The type can only be constructed through [`SafeRelativePath::parse`] or
/// [`SafeRelativePath::parse_archive_entry`], so APIs that accept it do not
/// need to repeat string checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SafeRelativePath {
    path: PathBuf,
}

impl SafeRelativePath {
    /// Parses a regular relative path (configuration, instance file, ...).
    pub fn parse(raw: impl AsRef<Path>) -> Result<Self, NexusError> {
        Self::from_path(raw.as_ref(), false)
    }

    /// Parses a ZIP entry name.
    ///
    /// ZIP stores `/` as separator, but Windows-created archives may contain
    /// `\`. Both are treated as separators. A trailing slash is allowed
    /// because it denotes a directory entry. Absolute names, drive-relative
    /// names (`C:evil`), UNC names and `..` components are rejected.
    pub fn parse_archive_entry(raw: &str) -> Result<Self, NexusError> {
        if raw.is_empty() {
            return Err(NexusError::archive_invalid("empty archive entry name"));
        }
        if raw.chars().any(char::is_control) {
            return Err(NexusError::archive_invalid(
                "archive entry contains control characters",
            ));
        }

        let normalized = raw.replace('\\', "/");
        if normalized.starts_with('/') {
            return Err(NexusError::archive_invalid(
                "absolute archive entry path is forbidden",
            ));
        }
        let bytes = normalized.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            return Err(NexusError::archive_invalid(
                "drive-relative archive entry path is forbidden",
            ));
        }

        let trimmed = normalized.trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(NexusError::archive_invalid(
                "archive entry does not name a file or directory",
            ));
        }

        Self::from_path(Path::new(trimmed), true)
    }

    fn from_path(path: &Path, from_archive: bool) -> Result<Self, NexusError> {
        let make_error = |detail: String| {
            if from_archive {
                NexusError::archive_invalid(detail)
            } else {
                NexusError::invalid_path(detail)
            }
        };

        if path.as_os_str().is_empty() {
            return Err(make_error("path is empty".to_owned()));
        }
        if path.to_string_lossy().chars().count() > MAX_PATH_CHARS {
            return Err(make_error("path is too long".to_owned()));
        }
        let raw = path.to_string_lossy().replace('\\', "/");
        if raw.contains("//") {
            return Err(make_error("path contains an empty component".to_owned()));
        }

        let mut cleaned = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(make_error("absolute paths are not allowed".to_owned()));
                }
                Component::ParentDir => {
                    return Err(make_error(
                        "parent directory components are not allowed".to_owned(),
                    ));
                }
                Component::CurDir => {
                    return Err(make_error(
                        "current directory components are not allowed".to_owned(),
                    ));
                }
                Component::Normal(part) => {
                    let part = part.to_string_lossy();
                    validate_component(&part, from_archive)?;
                    cleaned.push(part.as_ref());
                }
            }
        }

        if cleaned.as_os_str().is_empty() {
            return Err(make_error("path is empty".to_owned()));
        }
        Ok(Self { path: cleaned })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.path
    }

    /// Returns the parent, or `None` when the path is a single component.
    pub fn parent(&self) -> Option<SafeRelativePath> {
        self.path.parent().and_then(|parent| {
            if parent.as_os_str().is_empty() {
                None
            } else {
                Some(Self {
                    path: parent.to_path_buf(),
                })
            }
        })
    }

    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.path.file_name()
    }

    /// Case-insensitive key used to detect duplicates from Windows archives.
    pub fn collision_key(&self) -> String {
        self.path
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
    }
}

impl std::fmt::Display for SafeRelativePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for component in self.path.components() {
            if !first {
                f.write_str("/")?;
            }
            f.write_str(&component.as_os_str().to_string_lossy())?;
            first = false;
        }
        Ok(())
    }
}

fn validate_component(component: &str, from_archive: bool) -> Result<(), NexusError> {
    let make_error = |detail: String| {
        if from_archive {
            NexusError::archive_invalid(detail)
        } else {
            NexusError::invalid_path(detail)
        }
    };

    if component.is_empty() {
        return Err(make_error("path contains an empty component".to_owned()));
    }
    if component.chars().count() > MAX_COMPONENT_CHARS {
        return Err(make_error(format!(
            "path component is longer than {MAX_COMPONENT_CHARS} characters"
        )));
    }
    if component == "." || component == ".." {
        return Err(make_error(
            "relative path component is forbidden".to_owned(),
        ));
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(make_error(
            "path component ends with a dot or space".to_owned(),
        ));
    }

    for ch in component.chars() {
        if ch.is_control() {
            return Err(make_error(
                "path component contains a control character".to_owned(),
            ));
        }
        // These characters are invalid on Windows. Rejecting them on every
        // platform keeps archives and manifests portable and blocks NTFS
        // alternate data stream syntax (`file.txt:payload`).
        if matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
            return Err(make_error(format!(
                "path component contains reserved character {ch:?}"
            )));
        }
        if ch == '/' || ch == '\\' {
            return Err(make_error(
                "path component contains a path separator".to_owned(),
            ));
        }
    }

    if is_reserved_windows_name(component) {
        return Err(make_error(format!(
            "path component uses reserved Windows name {component:?}"
        )));
    }

    Ok(())
}

fn is_reserved_windows_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || is_reserved_numbered_name(&upper, "COM")
        || is_reserved_numbered_name(&upper, "LPT")
}

fn is_reserved_numbered_name(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_regular_relative_paths() {
        assert_eq!(
            SafeRelativePath::parse("mods/example.jar")
                .unwrap()
                .as_path(),
            Path::new("mods/example.jar")
        );
        assert!(SafeRelativePath::parse("a").is_ok());
        assert!(SafeRelativePath::parse("a/b/c.txt").is_ok());
    }

    #[test]
    fn rejects_traversal_absolute_and_reserved_paths() {
        for raw in [
            "../evil",
            "a/../../evil",
            "/etc/passwd",
            "C:\\Windows\\System32",
            "C:evil",
            "\\\\server\\share",
            "a//b",
            "a/.\u{0}",
            "CON",
            "con.txt",
            "COM1.jar",
            "lpt9",
            "file.txt:stream",
            "a/b.",
            "a/b ",
            "a/<b>",
        ] {
            assert!(
                SafeRelativePath::parse(raw).is_err(),
                "{raw:?} must be rejected"
            );
        }
    }

    #[test]
    fn archive_entries_normalize_windows_separators() {
        assert_eq!(
            SafeRelativePath::parse_archive_entry("mods\\example.jar")
                .unwrap()
                .as_path(),
            Path::new("mods/example.jar")
        );
        assert!(SafeRelativePath::parse_archive_entry("mods/").is_ok());
        for raw in [
            "",
            "/",
            "../evil",
            "..\\evil",
            "C:\\evil",
            "C:evil",
            "\\\\server\\share",
            "mods/CON",
            "mods/file.txt:stream",
            "mods/\u{1}",
        ] {
            assert!(
                SafeRelativePath::parse_archive_entry(raw).is_err(),
                "{raw:?} must be rejected"
            );
        }
    }

    #[test]
    fn display_is_stable_for_diagnostics() {
        let path = SafeRelativePath::parse("mods/a.jar").unwrap();
        assert_eq!(path.to_string(), "mods/a.jar");
        assert_eq!(path.collision_key(), "mods/a.jar");
    }
}
