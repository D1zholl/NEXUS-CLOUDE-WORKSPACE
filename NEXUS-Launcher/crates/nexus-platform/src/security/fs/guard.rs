//! Windows reparse-point handling and directory guards.
//!
//! A lexical path check is not enough on Windows: a directory below the root
//! can be replaced by a junction or a symlink between validation and use. Two
//! defences are used here:
//!
//! 1. every existing component is checked with `symlink_metadata` and rejected
//!    when it carries `FILE_ATTRIBUTE_REPARSE_POINT`;
//! 2. on Windows an open handle to every ancestor directory is kept for the
//!    duration of the operation. The handles do not share `DELETE`, so another
//!    process cannot rename or delete those directories while we work. This
//!    closes the classic check-then-use window for the directory chain.
//!
//! It does not protect against a process running as the same user that already
//! has write access to the leaf file itself; the checksum/verify layer covers
//! that case.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use nexus_core::NexusError;

#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Returns true for symlinks and for any other Windows reparse point,
/// including junctions and mount points.
pub fn is_reparse_point(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn reparse_error(path: &Path) -> NexusError {
    NexusError::reparse_rejected(format!(
        "{} is a symbolic link, junction or reparse point",
        path.display()
    ))
}

/// A held directory handle. Dropping it releases the no-delete share lock.
#[derive(Debug)]
pub struct DirectoryGuard {
    file: File,
    path: PathBuf,
}

impl DirectoryGuard {
    pub fn open(path: &Path) -> Result<Self, NexusError> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
        if is_reparse_point(&metadata) {
            return Err(reparse_error(path));
        }
        if !metadata.is_dir() {
            return Err(NexusError::invalid_path(format!(
                "{} is not a directory",
                path.display()
            )));
        }

        let file = open_directory(path).map_err(|error| map_io(path, error))?;

        // Re-check through the handle itself: the path may have changed between
        // `symlink_metadata` and `open`.
        let held = file.metadata().map_err(|error| map_io(path, error))?;
        if is_reparse_point(&held) {
            return Err(reparse_error(path));
        }
        if !held.is_dir() {
            return Err(NexusError::invalid_path(format!(
                "{} is not a directory",
                path.display()
            )));
        }

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub fn file(&self) -> &File {
        &self.file
    }
}

#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_directory(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Opens a file for reading without following a final-component symlink or
/// reparse point on Windows. The returned metadata is checked by the caller.
pub fn open_read_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

pub fn map_io(path: &Path, error: io::Error) -> NexusError {
    match error.kind() {
        io::ErrorKind::NotFound => NexusError::not_found(format!("{} not found", path.display())),
        io::ErrorKind::PermissionDenied => {
            NexusError::permission_denied(format!("{}: {error}", path.display()))
        }
        io::ErrorKind::AlreadyExists => {
            NexusError::conflict(format!("{} already exists", path.display()))
        }
        _ => NexusError::internal(format!("{}: {error}", path.display())),
    }
}

/// Recursively verifies that a freshly extracted tree contains no symlinks or
/// reparse points. This is a postcondition check for archive extraction.
pub fn assert_tree_has_no_reparse_points(root: &Path) -> Result<(), NexusError> {
    let metadata = std::fs::symlink_metadata(root).map_err(|error| map_io(root, error))?;
    if is_reparse_point(&metadata) {
        return Err(reparse_error(root));
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root).map_err(|error| map_io(root, error))? {
        let entry = entry.map_err(|error| map_io(root, error))?;
        assert_tree_has_no_reparse_points(&entry.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_directory_is_guarded() {
        let temp = tempfile::tempdir().unwrap();
        let guard = DirectoryGuard::open(temp.path()).unwrap();
        assert_eq!(guard.path(), temp.path());
    }

    #[test]
    fn regular_file_is_not_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"data").unwrap();
        assert!(DirectoryGuard::open(&file).is_err());
    }
}
