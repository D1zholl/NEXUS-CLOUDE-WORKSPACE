use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use tempfile::NamedTempFile;

use crate::error::NexusError;
use crate::time::system_now_ms;

/// All per-user data roots used by the launcher.
///
/// The root is fixed to `%LOCALAPPDATA%\NEXUS-Launcher` by default. Tests and
/// development builds may override it with `NEXUS_LAUNCHER_DATA_DIR`. The root
/// cannot be moved from `config.toml` (chicken-and-egg problem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    pub fn resolve_default() -> Result<Self, NexusError> {
        if let Some(raw) = std::env::var_os("NEXUS_LAUNCHER_DATA_DIR") {
            let raw = raw.to_string_lossy();
            if raw.trim().is_empty() {
                return Err(NexusError::invalid_path("NEXUS_LAUNCHER_DATA_DIR is empty"));
            }
            return Self::with_root(PathBuf::from(raw.as_ref()));
        }

        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(PathBuf::from)
                    .map(|home| home.join("AppData").join("Local"))
            })
            .ok_or_else(|| NexusError::invalid_path("LOCALAPPDATA is not defined for this user"))?;

        Self::with_root(local_app_data.join("NEXUS-Launcher"))
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Result<Self, NexusError> {
        let root = root.into();
        let raw = root.to_string_lossy();
        let validated = validate_directory(&raw)?;
        Ok(Self { root: validated })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("launcher.db")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn db_backups_dir(&self) -> PathBuf {
        self.root.join("backups").join("database")
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn servers_dir(&self) -> PathBuf {
        self.root.join("servers")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn java_dir(&self) -> PathBuf {
        self.root.join("java")
    }

    pub fn shared_dir(&self) -> PathBuf {
        self.root.join("shared")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn secrets_dir(&self) -> PathBuf {
        self.root.join("secrets")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Creates the directory layout. Existing directories and files are never
    /// deleted, so calling this on every start is safe and idempotent.
    pub fn ensure_layout(&self) -> Result<(), NexusError> {
        let directories = [
            self.root.clone(),
            self.logs_dir(),
            self.backups_dir(),
            self.db_backups_dir(),
            self.instances_dir(),
            self.servers_dir(),
            self.downloads_dir(),
            self.java_dir(),
            self.shared_dir(),
            self.cache_dir(),
            self.secrets_dir(),
            self.runs_dir(),
        ];

        for dir in directories {
            fs::create_dir_all(&dir).map_err(|error| {
                NexusError::invalid_path(format!("cannot create {}: {error}", dir.display()))
            })?;
        }
        Ok(())
    }

    /// Display form used by diagnostics and `app_info`.
    pub fn display_root(&self) -> String {
        let root = self.root.to_string_lossy();
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let local_app_data = PathBuf::from(local_app_data);
            if let Ok(relative) = self.root.strip_prefix(&local_app_data) {
                return format!("%LOCALAPPDATA%\\{}", relative.to_string_lossy());
            }
        }
        root.into_owned()
    }
}

/// Validates an absolute directory selected by the user or configured in TOML.
///
/// Relative paths, empty strings, control characters, `..` components and
/// Windows-invalid trailing dots/spaces are rejected. The directory is not
/// required to exist yet.
pub fn validate_directory(raw: &str) -> Result<PathBuf, NexusError> {
    if raw.is_empty() {
        return Err(NexusError::invalid_path("path is empty"));
    }
    if raw.len() > 32_767 {
        return Err(NexusError::invalid_path("path is too long"));
    }
    if raw != raw.trim() {
        return Err(NexusError::invalid_path(
            "path has leading or trailing whitespace",
        ));
    }
    if raw.chars().any(|ch| ch.is_control()) {
        return Err(NexusError::invalid_path("path contains control characters"));
    }

    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(NexusError::invalid_path("path must be absolute"));
    }

    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(NexusError::invalid_path("path must not contain '..'"));
            }
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.ends_with('.') || part.ends_with(' ') {
                    return Err(NexusError::invalid_path(
                        "path component ends with a dot or space",
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(path)
}

/// Atomically replaces a text/binary file.
///
/// The bytes are written to a temporary file in the same directory, flushed to
/// disk and then renamed over the destination. A crash cannot leave a
/// half-written `config.toml` or settings backup.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), NexusError> {
    let parent = path.parent().ok_or_else(|| {
        NexusError::config_write(format!("{} has no parent directory", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        NexusError::config_write(format!("cannot create {}: {error}", parent.display()))
    })?;

    let mut temp = NamedTempFile::new_in(parent).map_err(|error| {
        NexusError::config_write(format!(
            "cannot create temp file in {}: {error}",
            parent.display()
        ))
    })?;
    temp.write_all(bytes).map_err(|error| {
        NexusError::config_write(format!("cannot write {}: {error}", path.display()))
    })?;
    temp.as_file_mut().sync_all().map_err(|error| {
        NexusError::config_write(format!("cannot flush {}: {error}", path.display()))
    })?;

    if path.exists() {
        // `persist` replaces the destination atomically on Windows.
        temp.persist(path).map_err(|error| {
            NexusError::config_write(format!(
                "cannot replace {}: {}",
                path.display(),
                error.error
            ))
        })?;
    } else {
        temp.persist_noclobber(path).map_err(|error| {
            NexusError::config_write(format!("cannot create {}: {}", path.display(), error.error))
        })?;
    }
    Ok(())
}

/// Keeps a single `.bak` copy of the previous file content.
pub fn backup_file(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let backup = sibling_with_suffix(path, "bak");
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

/// Keeps a timestamped copy of a file that could not be parsed.
pub fn quarantine_file(path: &Path, reason: &str) -> io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let backup = sibling_with_suffix(path, &format!("{reason}-{}", system_now_ms()));
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    path.with_file_name(format!("{file_name}.{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_and_dangerous_paths() {
        assert!(validate_directory("").is_err());
        assert!(validate_directory("relative\\path").is_err());
        assert!(validate_directory("C:\\a\\..\\b").is_err());
        assert!(validate_directory("C:\\a\nb").is_err());
        assert!(validate_directory("C:\\a. ").is_err());
        assert!(validate_directory(" C:\\a").is_err());
    }

    #[test]
    fn accepts_absolute_windows_path() {
        let path = validate_directory("C:\\Users\\Player\\NEXUS").unwrap();
        assert!(path.is_absolute());
    }

    #[test]
    fn layout_is_idempotent_and_preserves_files() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DataPaths::with_root(temp.path().join("data")).unwrap();
        paths.ensure_layout().unwrap();
        let marker = paths.root().join("user-data.txt");
        fs::write(&marker, b"keep me").unwrap();
        paths.ensure_layout().unwrap();
        assert_eq!(fs::read(&marker).unwrap(), b"keep me");
        assert!(paths.instances_dir().is_dir());
        assert!(paths.db_backups_dir().is_dir());
    }

    #[test]
    fn atomic_write_replaces_existing_content_and_keeps_backup() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("config.toml");
        atomic_write(&file, b"first").unwrap();
        backup_file(&file).unwrap().unwrap();
        atomic_write(&file, b"second").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"second");
        assert_eq!(
            fs::read(temp.path().join("config.toml.bak")).unwrap(),
            b"first"
        );
    }
}
