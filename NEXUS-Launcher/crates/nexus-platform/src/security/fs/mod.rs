//! Capability-style filesystem operations rooted in one canonical directory.
//!
//! Every operation accepts only a [`SafeRelativePath`]. The resolver walks the
//! real directory chain, rejects symlinks/junctions/reparse points and verifies
//! canonical containment. On Windows it also holds no-delete handles for every
//! ancestor directory, so the directory chain cannot be swapped between the
//! check and the actual I/O.
//!
//! The module is deliberately synchronous: installation workflows are easier to
//! reason about when a sequence of checks and one operation is not interleaved
//! with unrelated async work.

pub mod archive;
pub mod checksum;
pub mod guard;
pub mod path;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use nexus_core::NexusError;
use tempfile::NamedTempFile;
use uuid::Uuid;

use self::checksum::{FileDigest, Sha256Digest, copy_limited, hash_file};
use self::guard::{DirectoryGuard, is_reparse_point, map_io, reparse_error};
pub use self::path::SafeRelativePath;

pub use self::archive::{
    ArchiveLimits, ExtractionReport, ZipInspection, extract_zip_into, extract_zip_to_new_dir,
    inspect_zip,
};
pub use self::checksum::{
    ExpectedFile, FileDigest as VerifiedFileDigest, hash_reader, verify_file,
};
pub use self::path::SafeRelativePath as RelativePath;

/// A directory that owns a subtree. Paths passed to its methods are always
/// relative and can never escape the subtree.
#[derive(Debug, Clone)]
pub struct SafeFs {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub absolute: PathBuf,
    pub relative: SafeRelativePath,
    pub exists: bool,
    pub is_dir: bool,
}

impl SafeFs {
    /// Opens an existing directory and stores its canonical form.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, NexusError> {
        let root = root.as_ref();
        let metadata = fs::symlink_metadata(root).map_err(|error| map_io(root, error))?;
        if !metadata.is_dir() {
            return Err(NexusError::invalid_path(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        let canonical = fs::canonicalize(root).map_err(|error| map_io(root, error))?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a relative path without performing a write. Existing components
    /// are checked against the real filesystem.
    pub fn resolve(&self, relative: &SafeRelativePath) -> Result<ResolvedPath, NexusError> {
        let components: Vec<_> = relative.as_path().components().collect();
        let mut current = self.root.clone();
        let mut missing = false;
        let mut last_is_dir = false;

        for (index, component) in components.iter().enumerate() {
            let next = current.join(component.as_os_str());
            if missing {
                current = next;
                continue;
            }
            match fs::symlink_metadata(&next) {
                Ok(metadata) => {
                    if is_reparse_point(&metadata) {
                        return Err(reparse_error(&next));
                    }
                    let is_last = index + 1 == components.len();
                    if !is_last && !metadata.is_dir() {
                        return Err(NexusError::invalid_path(format!(
                            "{} is not a directory",
                            next.display()
                        )));
                    }
                    last_is_dir = metadata.is_dir();
                    ensure_contained(&self.root, &next)?;
                    current = next;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    missing = true;
                    current = next;
                }
                Err(error) => return Err(map_io(&next, error)),
            }
        }

        ensure_contained(&self.root, &current)?;
        Ok(ResolvedPath {
            absolute: current,
            relative: relative.clone(),
            exists: !missing,
            is_dir: !missing && last_is_dir,
        })
    }

    pub fn resolve_existing(
        &self,
        relative: &SafeRelativePath,
    ) -> Result<ResolvedPath, NexusError> {
        let resolved = self.resolve(relative)?;
        if !resolved.exists {
            return Err(NexusError::not_found(format!(
                "{} does not exist",
                resolved.absolute.display()
            )));
        }
        Ok(resolved)
    }

    pub fn exists(&self, relative: &SafeRelativePath) -> Result<bool, NexusError> {
        Ok(self.resolve(relative)?.exists)
    }

    /// Creates every missing directory component. Existing components are
    /// re-validated and never followed through a reparse point.
    pub fn ensure_dir(&self, relative: &SafeRelativePath) -> Result<PathBuf, NexusError> {
        let mut guards = vec![DirectoryGuard::open(&self.root)?];
        let mut current = self.root.clone();

        for component in relative.as_path().components() {
            let next = current.join(component.as_os_str());
            match fs::symlink_metadata(&next) {
                Ok(metadata) => {
                    if is_reparse_point(&metadata) {
                        return Err(reparse_error(&next));
                    }
                    if !metadata.is_dir() {
                        return Err(NexusError::invalid_path(format!(
                            "{} is not a directory",
                            next.display()
                        )));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&next).map_err(|error| map_io(&next, error))?;
                }
                Err(error) => return Err(map_io(&next, error)),
            }
            let guard = DirectoryGuard::open(&next)?;
            guards.push(guard);
            current = next;
        }

        ensure_contained(&self.root, &current)?;
        drop(guards);
        Ok(current)
    }

    /// Writes a file by creating a temporary sibling, flushing it and renaming
    /// it over the destination. The parent directory chain is guarded.
    pub fn write_atomic(
        &self,
        relative: &SafeRelativePath,
        bytes: &[u8],
        overwrite: bool,
    ) -> Result<PathBuf, NexusError> {
        self.write_atomic_with(relative, overwrite, |file| {
            file.write_all(bytes)
                .map_err(|error| NexusError::internal(format!("write failed: {error}")))
        })
    }

    /// Streams data into an atomic destination with a hard size limit.
    pub fn write_atomic_from_reader(
        &self,
        relative: &SafeRelativePath,
        reader: impl Read,
        max_size: u64,
        overwrite: bool,
    ) -> Result<(PathBuf, FileDigest), NexusError> {
        let mut digest = None;
        let path = self.write_atomic_with(relative, overwrite, |file| {
            let computed = copy_limited(reader, &mut *file, max_size)?;
            digest = Some(computed);
            Ok(())
        })?;
        Ok((
            path,
            digest.expect("copy_limited must produce a digest when write succeeds"),
        ))
    }

    pub(super) fn write_atomic_with<F>(
        &self,
        relative: &SafeRelativePath,
        overwrite: bool,
        write: F,
    ) -> Result<PathBuf, NexusError>
    where
        F: FnOnce(&mut File) -> Result<(), NexusError>,
    {
        let (parent, destination, _guards) = self.prepare_destination(relative, overwrite)?;

        let mut temp = NamedTempFile::new_in(&parent).map_err(|error| {
            NexusError::internal(format!(
                "cannot create temporary file in {}: {error}",
                parent.display()
            ))
        })?;
        write(temp.as_file_mut())?;
        temp.as_file_mut().sync_all().map_err(|error| {
            NexusError::internal(format!("cannot flush temporary file: {error}"))
        })?;

        // Re-check immediately before the rename. The held directory guards
        // prevent the parent chain from being replaced on Windows.
        ensure_contained(&self.root, &parent)?;
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if is_reparse_point(&metadata) {
                return Err(reparse_error(&destination));
            }
            if metadata.is_dir() {
                return Err(NexusError::conflict(format!(
                    "{} is a directory",
                    destination.display()
                )));
            }
            if !overwrite {
                return Err(NexusError::conflict(format!(
                    "{} already exists",
                    destination.display()
                )));
            }
        }

        if overwrite {
            temp.persist(&destination).map_err(|error| {
                NexusError::internal(format!(
                    "cannot replace {}: {}",
                    destination.display(),
                    error.error
                ))
            })?;
        } else {
            temp.persist_noclobber(&destination).map_err(|error| {
                NexusError::conflict(format!(
                    "cannot create {}: {}",
                    destination.display(),
                    error.error
                ))
            })?;
        }

        sync_directory(&parent);
        Ok(destination)
    }

    pub fn read(&self, relative: &SafeRelativePath, max_size: u64) -> Result<Vec<u8>, NexusError> {
        let resolved = self.resolve_existing(relative)?;
        if resolved.is_dir {
            return Err(NexusError::invalid_path(format!(
                "{} is a directory",
                resolved.absolute.display()
            )));
        }
        let file = guard::open_read_no_follow(&resolved.absolute)
            .map_err(|error| map_io(&resolved.absolute, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| map_io(&resolved.absolute, error))?;
        if is_reparse_point(&metadata) || !metadata.is_file() {
            return Err(NexusError::invalid_path(format!(
                "{} is not a regular file",
                resolved.absolute.display()
            )));
        }
        if metadata.len() > max_size {
            return Err(NexusError::file_too_large(format!(
                "{} is {} bytes, limit is {max_size}",
                resolved.absolute.display(),
                metadata.len()
            )));
        }
        let mut output = Vec::with_capacity(metadata.len() as usize);
        file.take(max_size + 1)
            .read_to_end(&mut output)
            .map_err(|error| NexusError::internal(format!("read failed: {error}")))?;
        if output.len() as u64 > max_size {
            return Err(NexusError::file_too_large(format!(
                "{} exceeds limit of {max_size} bytes",
                resolved.absolute.display()
            )));
        }
        Ok(output)
    }

    pub fn verify(
        &self,
        relative: &SafeRelativePath,
        expected: &ExpectedFile,
        max_size: u64,
    ) -> Result<FileDigest, NexusError> {
        let resolved = self.resolve_existing(relative)?;
        verify_file(&resolved.absolute, expected, max_size)
    }

    pub fn hash(
        &self,
        relative: &SafeRelativePath,
        max_size: u64,
    ) -> Result<FileDigest, NexusError> {
        let resolved = self.resolve_existing(relative)?;
        hash_file(&resolved.absolute, max_size)
    }

    pub fn remove_file(&self, relative: &SafeRelativePath) -> Result<(), NexusError> {
        let (parent, path, _guards) = self.prepare_parent(relative, false)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| map_io(&path, error))?;
        if is_reparse_point(&metadata) {
            return Err(reparse_error(&path));
        }
        if metadata.is_dir() {
            return Err(NexusError::invalid_path(format!(
                "{} is a directory",
                path.display()
            )));
        }
        fs::remove_file(&path).map_err(|error| map_io(&path, error))?;
        sync_directory(&parent);
        Ok(())
    }

    /// Recursively deletes a directory. Reparse points are removed as links and
    /// are never traversed, so a junction cannot delete data outside the root.
    pub fn remove_dir_all(&self, relative: &SafeRelativePath) -> Result<(), NexusError> {
        let (parent, path, _guards) = self.prepare_parent(relative, false)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(map_io(&path, error)),
        };
        if is_reparse_point(&metadata) {
            remove_link(&path, &metadata)?;
        } else if metadata.is_dir() {
            remove_tree(&path)?;
        } else {
            return Err(NexusError::invalid_path(format!(
                "{} is not a directory",
                path.display()
            )));
        }
        sync_directory(&parent);
        Ok(())
    }

    /// Moves a file or directory within the same root. If the operating system
    /// reports a cross-volume move, a verified copy-and-delete fallback is used.
    pub fn rename(
        &self,
        from: &SafeRelativePath,
        to: &SafeRelativePath,
        overwrite: bool,
    ) -> Result<PathBuf, NexusError> {
        let source = self.resolve_existing(from)?;
        if is_reparse_point(
            &fs::symlink_metadata(&source.absolute)
                .map_err(|error| map_io(&source.absolute, error))?,
        ) {
            return Err(reparse_error(&source.absolute));
        }
        if source.absolute == self.root {
            return Err(NexusError::invalid_path(
                "refusing to move the root directory",
            ));
        }

        let (destination_parent, destination, _guards) = self.prepare_destination(to, overwrite)?;
        if source.absolute == destination {
            return Ok(destination);
        }

        match fs::rename(&source.absolute, &destination) {
            Ok(()) => {
                sync_directory(&destination_parent);
                Ok(destination)
            }
            Err(error) if is_cross_device(&error) => {
                copy_across_volumes(
                    &source.absolute,
                    &destination,
                    &destination_parent,
                    overwrite,
                )?;
                if source.absolute.is_dir() {
                    remove_tree(&source.absolute)?;
                } else {
                    fs::remove_file(&source.absolute)
                        .map_err(|error| map_io(&source.absolute, error))?;
                }
                sync_directory(&destination_parent);
                Ok(destination)
            }
            Err(error) => Err(map_io(&destination, error)),
        }
    }

    /// Creates a unique staging directory below `.staging/`. The returned guard
    /// removes it on drop unless ownership is transferred with
    /// [`StagingDir::disarm`].
    pub fn create_staging(&self, label: &str) -> Result<StagingDir, NexusError> {
        let safe_label: String = label
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .take(32)
            .collect();
        let safe_label = if safe_label.is_empty() {
            "stage".to_owned()
        } else {
            safe_label
        };

        self.ensure_dir(&SafeRelativePath::parse(".staging").expect(".staging is a valid path"))?;
        let relative =
            SafeRelativePath::parse(format!(".staging/{safe_label}-{}", Uuid::new_v4()))?;
        self.ensure_dir(&relative)?;
        let absolute = self.root.join(relative.as_path());
        let staging_guard = DirectoryGuard::open(&absolute)?;
        drop(staging_guard);
        Ok(StagingDir {
            parent: self.clone(),
            relative,
            active: true,
        })
    }

    fn prepare_parent(
        &self,
        relative: &SafeRelativePath,
        create_missing: bool,
    ) -> Result<(PathBuf, PathBuf, Vec<DirectoryGuard>), NexusError> {
        let mut guards = vec![DirectoryGuard::open(&self.root)?];
        let mut parent = self.root.clone();

        if let Some(parent_relative) = relative.parent() {
            for component in parent_relative.as_path().components() {
                let next = parent.join(component.as_os_str());
                match fs::symlink_metadata(&next) {
                    Ok(metadata) => {
                        if is_reparse_point(&metadata) {
                            return Err(reparse_error(&next));
                        }
                        if !metadata.is_dir() {
                            return Err(NexusError::invalid_path(format!(
                                "{} is not a directory",
                                next.display()
                            )));
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound && create_missing => {
                        fs::create_dir(&next).map_err(|error| map_io(&next, error))?;
                    }
                    Err(error) => return Err(map_io(&next, error)),
                }
                let guard = DirectoryGuard::open(&next)?;
                guards.push(guard);
                parent = next;
            }
        }

        ensure_contained(&self.root, &parent)?;
        let file_name = relative
            .file_name()
            .ok_or_else(|| NexusError::invalid_path("path has no file name"))?;
        let destination = parent.join(file_name);
        Ok((parent, destination, guards))
    }

    fn prepare_destination(
        &self,
        relative: &SafeRelativePath,
        overwrite: bool,
    ) -> Result<(PathBuf, PathBuf, Vec<DirectoryGuard>), NexusError> {
        let (parent, destination, guards) = self.prepare_parent(relative, true)?;

        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if is_reparse_point(&metadata) {
                return Err(reparse_error(&destination));
            }
            if !overwrite {
                return Err(NexusError::conflict(format!(
                    "{} already exists",
                    destination.display()
                )));
            }
        }

        Ok((parent, destination, guards))
    }
}

/// A unique temporary directory. It is removed recursively on drop.
#[derive(Debug)]
pub struct StagingDir {
    parent: SafeFs,
    relative: SafeRelativePath,
    active: bool,
}

impl StagingDir {
    pub fn path(&self) -> PathBuf {
        self.parent.root().join(self.relative.as_path())
    }

    pub fn relative(&self) -> &SafeRelativePath {
        &self.relative
    }

    pub fn parent(&self) -> &SafeFs {
        &self.parent
    }

    /// Opens the staging directory as a new root. Validation starts at the
    /// staging directory, so extraction cannot reach the main root unless the
    /// caller explicitly commits the result.
    pub fn filesystem(&self) -> Result<SafeFs, NexusError> {
        SafeFs::open(self.path())
    }

    /// Transfers ownership: the directory is no longer removed on drop.
    pub fn disarm(&mut self) {
        self.active = false;
    }

    /// Verifies a staged file and atomically moves it into `destination`.
    pub fn commit_file(
        &self,
        staged: &SafeRelativePath,
        destination_parent: &SafeFs,
        destination: &SafeRelativePath,
        expected: &ExpectedFile,
        max_size: u64,
        overwrite: bool,
    ) -> Result<FileDigest, NexusError> {
        let staging_fs = self.filesystem()?;
        let digest = staging_fs.verify(staged, expected, max_size)?;
        let staged_absolute = staging_fs.resolve_existing(staged)?.absolute;
        let staged_metadata = fs::symlink_metadata(&staged_absolute)
            .map_err(|error| map_io(&staged_absolute, error))?;
        if is_reparse_point(&staged_metadata) || !staged_metadata.is_file() {
            return Err(NexusError::invalid_path(format!(
                "{} is not a regular staged file",
                staged_absolute.display()
            )));
        }

        let (parent, destination_absolute, _guards) =
            destination_parent.prepare_destination(destination, overwrite)?;

        match fs::rename(&staged_absolute, &destination_absolute) {
            Ok(()) => {}
            Err(error) if is_cross_device(&error) => {
                let mut input = guard::open_read_no_follow(&staged_absolute)
                    .map_err(|error| map_io(&staged_absolute, error))?;
                destination_parent.write_atomic_from_reader(
                    destination,
                    &mut input,
                    max_size,
                    overwrite,
                )?;
                fs::remove_file(&staged_absolute)
                    .map_err(|error| map_io(&staged_absolute, error))?;
            }
            Err(error) => return Err(map_io(&destination_absolute, error)),
        }

        sync_directory(&parent);

        let installed = destination_parent.hash(destination, max_size)?;
        if installed.size != digest.size || !installed.sha256.constant_time_eq(&digest.sha256) {
            let _ = destination_parent.remove_file(destination);
            return Err(NexusError::checksum_mismatch(format!(
                "{} changed while being installed",
                destination
            )));
        }

        Ok(installed)
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if self.active {
            let _ = self.parent.remove_dir_all(&self.relative);
        }
    }
}

fn ensure_contained(root: &Path, path: &Path) -> Result<(), NexusError> {
    if !path.starts_with(root) {
        return Err(NexusError::new(
            nexus_core::ErrorCode::PathOutsideRoot,
            "Путь находится вне разрешённого каталога.",
        )
        .with_detail(format!("{} escapes {}", path.display(), root.display())));
    }
    Ok(())
}

fn sync_directory(path: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn remove_link(path: &Path, metadata: &fs::Metadata) -> Result<(), NexusError> {
    #[cfg(windows)]
    {
        let _ = metadata;
        // Junctions are directories and must be removed with RemoveDirectory,
        // while a symlink to a file is removed with DeleteFile. Metadata does
        // not always distinguish them reliably, so try the directory form
        // first and fall back to the file form.
        match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(directory_error) => fs::remove_file(path).map_err(|file_error| {
                map_io(
                    path,
                    if file_error.kind() == io::ErrorKind::PermissionDenied {
                        directory_error
                    } else {
                        file_error
                    },
                )
            }),
        }
    }

    #[cfg(not(windows))]
    {
        let result = if metadata.is_dir() {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|error| map_io(path, error))
    }
}

fn remove_tree(path: &Path) -> Result<(), NexusError> {
    let entries = fs::read_dir(path).map_err(|error| map_io(path, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| map_io(path, error))?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| map_io(&child, error))?;
        if is_reparse_point(&metadata) {
            remove_link(&child, &metadata)?;
        } else if metadata.is_dir() {
            remove_tree(&child)?;
        } else {
            fs::remove_file(&child).map_err(|error| map_io(&child, error))?;
        }
    }
    fs::remove_dir(path).map_err(|error| map_io(path, error))
}

fn is_cross_device(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(17) | Some(18))
}

fn copy_across_volumes(
    source: &Path,
    destination: &Path,
    destination_parent: &Path,
    overwrite: bool,
) -> Result<(), NexusError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| map_io(source, error))?;
    if is_reparse_point(&metadata) {
        return Err(reparse_error(source));
    }

    if metadata.is_dir() {
        let temp = unique_child(destination_parent, "move")?;
        fs::create_dir(&temp).map_err(|error| map_io(&temp, error))?;
        let result = copy_tree(source, &temp).and_then(|()| {
            if overwrite && destination.exists() {
                let existing = fs::symlink_metadata(destination)
                    .map_err(|error| map_io(destination, error))?;
                if is_reparse_point(&existing) {
                    return Err(reparse_error(destination));
                }
                if existing.is_dir() {
                    remove_tree(destination)?;
                } else {
                    fs::remove_file(destination).map_err(|error| map_io(destination, error))?;
                }
            }
            fs::rename(&temp, destination).map_err(|error| map_io(destination, error))
        });
        if result.is_err() {
            let _ = remove_tree(&temp);
        }
        return result;
    }

    let temp = unique_child(destination_parent, "move")?;
    let result = (|| {
        let source_digest = hash_file(source, u64::MAX)?;
        let mut input = guard::open_read_no_follow(source).map_err(|e| map_io(source, e))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| map_io(&temp, error))?;
        io::copy(&mut input, &mut output)
            .map_err(|error| NexusError::internal(format!("copy failed: {error}")))?;
        output
            .sync_all()
            .map_err(|error| NexusError::internal(format!("flush failed: {error}")))?;
        drop(output);

        let copied_digest = hash_file(&temp, u64::MAX)?;
        if copied_digest.size != source_digest.size
            || !copied_digest.sha256.constant_time_eq(&source_digest.sha256)
        {
            return Err(NexusError::checksum_mismatch(format!(
                "{} changed while being moved across volumes",
                source.display()
            )));
        }

        if overwrite && destination.exists() {
            let existing =
                fs::symlink_metadata(destination).map_err(|error| map_io(destination, error))?;
            if is_reparse_point(&existing) {
                return Err(reparse_error(destination));
            }
            fs::remove_file(destination).map_err(|error| map_io(destination, error))?;
        }
        fs::rename(&temp, destination).map_err(|error| map_io(destination, error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), NexusError> {
    for entry in fs::read_dir(source).map_err(|error| map_io(source, error))? {
        let entry = entry.map_err(|error| map_io(source, error))?;
        let source_child = entry.path();
        let metadata =
            fs::symlink_metadata(&source_child).map_err(|error| map_io(&source_child, error))?;
        if is_reparse_point(&metadata) {
            return Err(reparse_error(&source_child));
        }
        let destination_child = destination.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir(&destination_child)
                .map_err(|error| map_io(&destination_child, error))?;
            copy_tree(&source_child, &destination_child)?;
        } else {
            fs::copy(&source_child, &destination_child)
                .map_err(|error| map_io(&destination_child, error))?;
        }
    }
    Ok(())
}

fn unique_child(parent: &Path, label: &str) -> Result<PathBuf, NexusError> {
    let name = format!(".{label}-{}", Uuid::new_v4());
    let path = parent.join(name);
    if path.exists() {
        return Err(NexusError::conflict("temporary name collision"));
    }
    Ok(path)
}

/// Computes a SHA-256 digest for an on-disk file.
pub fn sha256_file(path: &Path, max_size: u64) -> Result<Sha256Digest, NexusError> {
    Ok(hash_file(path, max_size)?.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> (tempfile::TempDir, SafeFs) {
        let temp = tempfile::tempdir().unwrap();
        let fs = SafeFs::open(temp.path()).unwrap();
        (temp, fs)
    }

    #[test]
    fn atomic_write_replaces_and_preserves_previous_on_new_file_flag() {
        let (_temp, fs) = fs();
        let rel = SafeRelativePath::parse("a/b.txt").unwrap();
        fs.write_atomic(&rel, b"one", false).unwrap();
        assert_eq!(fs.read(&rel, 1024).unwrap(), b"one");
        assert!(fs.write_atomic(&rel, b"two", false).is_err());
        fs.write_atomic(&rel, b"two", true).unwrap();
        assert_eq!(fs.read(&rel, 1024).unwrap(), b"two");
    }

    #[test]
    fn atomic_write_rejects_symlink_destination() {
        let (_temp, fs) = fs();
        let rel = SafeRelativePath::parse("link.txt").unwrap();
        #[cfg(windows)]
        {
            let target = fs.root().join("target.txt");
            fs::write(&target, b"secret").unwrap();
            if std::os::windows::fs::symlink_file(&target, fs.root().join("link.txt")).is_err() {
                return; // symlink creation needs developer mode / privilege
            }
            assert!(fs.write_atomic(&rel, b"overwrite", true).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"secret");
        }
    }

    #[test]
    fn safe_move_and_safe_delete() {
        let (_temp, fs) = fs();
        let from = SafeRelativePath::parse("from/file.txt").unwrap();
        let to = SafeRelativePath::parse("to/file.txt").unwrap();
        fs.write_atomic(&from, b"payload", false).unwrap();
        fs.rename(&from, &to, false).unwrap();
        assert!(!fs.exists(&from).unwrap());
        assert_eq!(fs.read(&to, 1024).unwrap(), b"payload");
        fs.remove_dir_all(&SafeRelativePath::parse("to").unwrap())
            .unwrap();
        assert!(!fs.exists(&to).unwrap());
    }

    #[test]
    fn staging_is_unique_and_removed_on_drop() {
        let (_temp, fs) = fs();
        let first = fs.create_staging("test").unwrap();
        let second = fs.create_staging("test").unwrap();
        assert_ne!(first.path(), second.path());
        let first_path = first.path();
        drop(first);
        assert!(!first_path.exists());
        assert!(second.path().exists());
    }

    #[test]
    fn staging_commit_verifies_checksum_before_overwriting() {
        let (temp, fs) = fs();
        let staging = fs.create_staging("install").unwrap();
        let staged = SafeRelativePath::parse("payload.bin").unwrap();
        let staging_fs = staging.filesystem().unwrap();
        staging_fs.write_atomic(&staged, b"payload", false).unwrap();

        let dest = SafeRelativePath::parse("installed/payload.bin").unwrap();
        let wrong = ExpectedFile::sha256(Sha256Digest::from_hex(&"0".repeat(64)).unwrap());
        assert!(
            staging
                .commit_file(&staged, &fs, &dest, &wrong, 1024, false)
                .is_err()
        );
        assert!(!temp.path().join("installed/payload.bin").exists());

        let digest = staging_fs.hash(&staged, 1024).unwrap();
        let correct = ExpectedFile::new(digest.sha256, digest.size);
        let installed = staging
            .commit_file(&staged, &fs, &dest, &correct, 1024, false)
            .unwrap();
        assert_eq!(installed.size, 7);
        assert_eq!(fs.read(&dest, 1024).unwrap(), b"payload");
    }

    #[test]
    fn root_cannot_be_deleted() {
        let root = SafeRelativePath::parse(".").unwrap_err();
        assert_eq!(root.code(), nexus_core::ErrorCode::InvalidPath);
    }
}
