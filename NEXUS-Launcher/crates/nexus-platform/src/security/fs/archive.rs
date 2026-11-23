//! Hardened ZIP inspection and extraction.
//!
//! Protection is layered:
//! * every entry name is validated as a portable relative path before any I/O;
//! * absolute paths, drive-relative paths, UNC names, `..`, ADS syntax and
//!   reserved Windows device names are rejected;
//! * symlink entries are rejected (not rewritten or followed);
//! * encrypted entries are rejected;
//! * entry count, per-entry size, total uncompressed size and compression ratio
//!   are bounded *before* decompression and enforced again while streaming;
//! * CRC-32 and the actual byte count are checked for every file;
//! * extraction happens in a random staging directory and only then is the
//!   result moved into place;
//! * the finished tree is scanned for reparse points as a postcondition.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

#[cfg(windows)]
use std::fs::OpenOptions;

use nexus_core::NexusError;
use zip::read::ZipArchive;
use zip::result::ZipError;

use super::SafeFs;
use super::guard::{assert_tree_has_no_reparse_points, is_reparse_point, map_io};
use super::path::SafeRelativePath;

const READ_BUFFER: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ArchiveLimits {
    pub max_archive_size: u64,
    pub max_entries: usize,
    pub max_entry_size: u64,
    pub max_total_size: u64,
    pub max_compression_ratio: u64,
    pub max_path_chars: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_archive_size: 2 * 1024 * 1024 * 1024,
            max_entries: 100_000,
            max_entry_size: 512 * 1024 * 1024,
            max_total_size: 4 * 1024 * 1024 * 1024,
            max_compression_ratio: 200,
            max_path_chars: 32_767,
        }
    }
}

impl ArchiveLimits {
    pub fn modpack() -> Self {
        Self::default()
    }

    pub fn strict() -> Self {
        Self {
            max_archive_size: 512 * 1024 * 1024,
            max_entries: 25_000,
            max_entry_size: 128 * 1024 * 1024,
            max_total_size: 1024 * 1024 * 1024,
            max_compression_ratio: 100,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedEntry {
    pub path: SafeRelativePath,
    pub is_dir: bool,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub crc32: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipInspection {
    pub entries: Vec<InspectedEntry>,
    pub file_count: usize,
    pub directory_count: usize,
    pub total_uncompressed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionReport {
    pub files: usize,
    pub directories: usize,
    pub total_bytes: u64,
}

/// Validates an archive without extracting anything.
pub fn inspect_zip(path: &Path, limits: &ArchiveLimits) -> Result<ZipInspection, NexusError> {
    let mut archive = open_archive(path, limits)?;
    inspect_archive(&mut archive, limits)
}

/// Extracts into a new directory transactionally.
///
/// The destination must not exist unless `replace` is true. In the replace case
/// the old directory is first moved aside, the new tree is moved into place and
/// only then is the old tree deleted.
pub fn extract_zip_to_new_dir(
    zip_path: &Path,
    root: &SafeFs,
    destination: &SafeRelativePath,
    limits: &ArchiveLimits,
    replace: bool,
) -> Result<ExtractionReport, NexusError> {
    let existing = root.resolve(destination)?;
    if existing.exists {
        if !replace {
            return Err(NexusError::conflict(format!(
                "{} already exists",
                existing.absolute.display()
            )));
        }
        let metadata = std::fs::symlink_metadata(&existing.absolute)
            .map_err(|e| map_io(&existing.absolute, e))?;
        if is_reparse_point(&metadata) {
            return Err(NexusError::invalid_path(format!(
                "{} is a reparse point",
                existing.absolute.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(NexusError::conflict(format!(
                "{} is not a directory",
                existing.absolute.display()
            )));
        }
    }

    let mut staging = root.create_staging("zip")?;
    let staging_fs = staging.filesystem()?;
    let report = extract_into(&staging_fs, zip_path, limits)?;
    assert_tree_has_no_reparse_points(&staging.path())?;

    if existing.exists {
        let old_name = format!(
            "{}.old-{}",
            destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archive".to_owned()),
            uuid::Uuid::new_v4()
        );
        let old_relative = match destination.parent() {
            Some(parent) => SafeRelativePath::parse(parent.as_path().join(old_name))?,
            None => SafeRelativePath::parse(old_name)?,
        };

        root.rename(destination, &old_relative, false)?;
        match root.rename(staging.relative(), destination, false) {
            Ok(_) => {
                staging.disarm();
                let _ = root.remove_dir_all(&old_relative);
            }
            Err(error) => {
                // Best-effort rollback: put the previous installation back.
                let _ = root.rename(&old_relative, destination, false);
                return Err(error);
            }
        }
    } else {
        root.rename(staging.relative(), destination, false)?;
        staging.disarm();
    }

    Ok(report)
}

/// Extracts into an existing directory. Each file is moved into place
/// individually, so an interrupted merge leaves the previously installed files
/// that were already committed intact.
pub fn extract_zip_into(
    zip_path: &Path,
    root: &SafeFs,
    destination: &SafeRelativePath,
    limits: &ArchiveLimits,
    overwrite: bool,
) -> Result<ExtractionReport, NexusError> {
    root.ensure_dir(destination)?;
    let staging = root.create_staging("zip-merge")?;
    let staging_fs = staging.filesystem()?;
    let report = extract_into(&staging_fs, zip_path, limits)?;
    assert_tree_has_no_reparse_points(&staging.path())?;

    merge_directory(&staging, &staging_fs, root, destination, overwrite, limits)?;
    Ok(report)
}

fn open_archive(path: &Path, limits: &ArchiveLimits) -> Result<ZipArchive<File>, NexusError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    if is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(NexusError::archive_invalid(format!(
            "{} is not a regular archive file",
            path.display()
        )));
    }
    if metadata.len() > limits.max_archive_size {
        return Err(NexusError::file_too_large(format!(
            "archive is {} bytes, limit is {}",
            metadata.len(),
            limits.max_archive_size
        )));
    }

    let file = open_archive_file(path).map_err(|error| map_io(path, error))?;
    ZipArchive::new(file).map_err(|error| zip_error("cannot open archive", error))
}

#[cfg(windows)]
fn open_archive_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // Share only reads while the archive is being inspected/extracted. A
    // second process cannot mutate the archive under us.
    OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001)
        .open(path)
}

#[cfg(not(windows))]
fn open_archive_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

fn inspect_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    limits: &ArchiveLimits,
) -> Result<ZipInspection, NexusError> {
    if archive.len() > limits.max_entries {
        return Err(NexusError::quota_exceeded(format!(
            "archive has {} entries, limit is {}",
            archive.len(),
            limits.max_entries
        )));
    }

    let mut entries = Vec::with_capacity(archive.len());
    let mut all_names = HashSet::new();
    let mut files = HashSet::new();
    let mut directories = HashSet::new();
    let mut total = 0u64;
    let mut file_count = 0usize;
    let mut directory_count = 0usize;

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| zip_error("cannot read archive entry", error))?;

        let raw_name = entry.name();
        if raw_name.chars().count() > limits.max_path_chars {
            return Err(NexusError::archive_invalid(format!(
                "entry name is longer than {} characters",
                limits.max_path_chars
            )));
        }
        let path = SafeRelativePath::parse_archive_entry(raw_name)?;

        if entry.is_symlink() || is_unix_symlink(entry.unix_mode()) {
            return Err(NexusError::symlink_rejected(format!(
                "archive entry {raw_name:?} is a symbolic link"
            )));
        }
        if entry.encrypted() {
            return Err(NexusError::archive_invalid(format!(
                "archive entry {raw_name:?} is encrypted"
            )));
        }

        let is_dir = entry.is_dir() || raw_name.ends_with('/');

        if entry.size() > limits.max_entry_size {
            return Err(NexusError::file_too_large(format!(
                "archive entry {raw_name:?} is {} bytes, limit is {}",
                entry.size(),
                limits.max_entry_size
            )));
        }
        if entry.size() > 0 {
            let ratio = if entry.compressed_size() == 0 {
                u64::MAX
            } else {
                entry.size() / entry.compressed_size().max(1)
            };
            if ratio > limits.max_compression_ratio {
                return Err(NexusError::archive_invalid(format!(
                    "archive entry {raw_name:?} has compression ratio {ratio}, limit is {}",
                    limits.max_compression_ratio
                )));
            }
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| NexusError::quota_exceeded("archive size overflow"))?;
        if total > limits.max_total_size {
            return Err(NexusError::quota_exceeded(format!(
                "archive total uncompressed size exceeds {} bytes",
                limits.max_total_size
            )));
        }

        let key = path.collision_key();
        if !all_names.insert(key.clone()) {
            return Err(NexusError::archive_invalid(format!(
                "archive contains duplicate path {raw_name:?}"
            )));
        }

        if is_dir {
            if files.contains(&key) {
                return Err(NexusError::archive_invalid(format!(
                    "archive entry {raw_name:?} is both a file and a directory"
                )));
            }
            directories.insert(key);
            directory_count += 1;
        } else {
            if directories.contains(&key) {
                return Err(NexusError::archive_invalid(format!(
                    "archive entry {raw_name:?} is both a file and a directory"
                )));
            }
            let mut ancestor = path.parent();
            while let Some(parent) = ancestor {
                let parent_key = parent.collision_key();
                if files.contains(&parent_key) {
                    return Err(NexusError::archive_invalid(format!(
                        "archive entry {raw_name:?} is nested below a file"
                    )));
                }
                directories.insert(parent_key);
                ancestor = parent.parent();
            }
            files.insert(key);
            file_count += 1;
        }

        entries.push(InspectedEntry {
            path,
            is_dir,
            uncompressed_size: entry.size(),
            compressed_size: entry.compressed_size(),
            crc32: entry.crc32(),
        });
    }

    Ok(ZipInspection {
        entries,
        file_count,
        directory_count,
        total_uncompressed_size: total,
    })
}

fn extract_into(
    destination: &SafeFs,
    zip_path: &Path,
    limits: &ArchiveLimits,
) -> Result<ExtractionReport, NexusError> {
    let mut archive = open_archive(zip_path, limits)?;
    let inspection = inspect_archive(&mut archive, limits)?;

    let mut report = ExtractionReport {
        files: 0,
        directories: 0,
        total_bytes: 0,
    };

    for (index, inspected) in inspection.entries.iter().enumerate() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| zip_error("cannot read archive entry", error))?;

        if inspected.is_dir {
            destination.ensure_dir(&inspected.path)?;
            report.directories += 1;
            continue;
        }

        let mut crc = crc32fast::Hasher::new();
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut written = 0u64;

        destination.write_atomic_with(&inspected.path, false, |output| {
            loop {
                let read = entry.read(&mut buffer).map_err(|error| {
                    NexusError::archive_invalid(format!("cannot decompress archive entry: {error}"))
                })?;
                if read == 0 {
                    break;
                }
                written = written
                    .checked_add(read as u64)
                    .ok_or_else(|| NexusError::file_too_large("archive entry size overflow"))?;
                if written > inspected.uncompressed_size || written > limits.max_entry_size {
                    return Err(NexusError::archive_invalid(format!(
                        "archive entry {:?} expands beyond its declared size",
                        inspected.path.to_string()
                    )));
                }
                output.write_all(&buffer[..read]).map_err(|error| {
                    NexusError::internal(format!("cannot write extracted file: {error}"))
                })?;
                crc.update(&buffer[..read]);
            }
            Ok(())
        })?;

        if written != inspected.uncompressed_size {
            return Err(NexusError::archive_invalid(format!(
                "archive entry {:?} has an inconsistent size",
                inspected.path.to_string()
            )));
        }
        if crc.finalize() != inspected.crc32 {
            return Err(NexusError::archive_invalid(format!(
                "archive entry {:?} failed CRC-32 verification",
                inspected.path.to_string()
            )));
        }

        report.files += 1;
        report.total_bytes = report
            .total_bytes
            .checked_add(written)
            .ok_or_else(|| NexusError::quota_exceeded("extraction size overflow"))?;
    }

    Ok(report)
}

fn merge_directory(
    staging: &super::StagingDir,
    staging_fs: &SafeFs,
    destination_root: &SafeFs,
    destination: &SafeRelativePath,
    overwrite: bool,
    limits: &ArchiveLimits,
) -> Result<(), NexusError> {
    merge_entries(
        staging,
        staging_fs,
        staging_fs.root(),
        destination_root,
        destination,
        overwrite,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
fn merge_entries(
    staging: &super::StagingDir,
    staging_fs: &SafeFs,
    current_directory: &Path,
    destination_root: &SafeFs,
    destination: &SafeRelativePath,
    overwrite: bool,
    limits: &ArchiveLimits,
) -> Result<(), NexusError> {
    for entry in
        std::fs::read_dir(current_directory).map_err(|error| map_io(current_directory, error))?
    {
        let entry = entry.map_err(|error| map_io(current_directory, error))?;
        let absolute = entry.path();
        let metadata =
            std::fs::symlink_metadata(&absolute).map_err(|error| map_io(&absolute, error))?;
        if is_reparse_point(&metadata) {
            return Err(NexusError::reparse_rejected(format!(
                "{} is a reparse point",
                absolute.display()
            )));
        }

        let relative_to_staging = absolute
            .strip_prefix(staging_fs.root())
            .map_err(|_| NexusError::internal("staging path escaped its root"))?;
        let relative = SafeRelativePath::parse(relative_to_staging)?;
        let target = SafeRelativePath::parse(destination.as_path().join(relative.as_path()))?;

        if metadata.is_dir() {
            destination_root.ensure_dir(&target)?;
            merge_entries(
                staging,
                staging_fs,
                &absolute,
                destination_root,
                destination,
                overwrite,
                limits,
            )?;
        } else {
            let digest = staging_fs.hash(&relative, limits.max_entry_size)?;
            let expected = super::ExpectedFile::new(digest.sha256, digest.size);
            if let Some(parent) = target.parent() {
                destination_root.ensure_dir(&parent)?;
            }
            staging.commit_file(
                &relative,
                destination_root,
                &target,
                &expected,
                limits.max_entry_size,
                overwrite,
            )?;
        }
    }
    Ok(())
}

fn is_unix_symlink(mode: Option<u32>) -> bool {
    mode.is_some_and(|mode| mode & 0o170_000 == 0o120_000)
}

fn zip_error(context: &str, error: ZipError) -> NexusError {
    NexusError::archive_invalid(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn safe_fs() -> (tempfile::TempDir, SafeFs) {
        let temp = tempfile::tempdir().unwrap();
        let fs = SafeFs::open(temp.path()).unwrap();
        (temp, fs)
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn validates_safe_archive() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("safe.zip");
        write_zip(
            &archive,
            &[("mods/example.jar", b"jar"), ("config/a.txt", b"x")],
        );
        let inspection = inspect_zip(&archive, &ArchiveLimits::default()).unwrap();
        assert_eq!(inspection.file_count, 2);
        assert_eq!(inspection.total_uncompressed_size, 4);
    }

    #[test]
    fn rejects_zip_slip_and_absolute_paths() {
        for name in [
            "../evil.txt",
            "..\\evil.txt",
            "/absolute.txt",
            "C:\\evil.txt",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let archive = temp.path().join("evil.zip");
            write_zip(&archive, &[(name, b"evil")]);
            let error = inspect_zip(&archive, &ArchiveLimits::default()).unwrap_err();
            assert_eq!(
                error.code(),
                nexus_core::ErrorCode::ArchiveInvalid,
                "{name}"
            );
        }
    }

    #[test]
    fn extraction_never_writes_outside_destination() {
        let (temp, fs) = safe_fs();
        let archive = temp.path().join("evil.zip");
        write_zip(
            &archive,
            &[("../outside.txt", b"evil"), ("safe.txt", b"ok")],
        );
        let dest = SafeRelativePath::parse("instance").unwrap();
        assert!(
            extract_zip_to_new_dir(&archive, &fs, &dest, &ArchiveLimits::default(), false).is_err()
        );
        assert!(!temp.path().join("outside.txt").exists());
        assert!(!temp.path().join("instance").exists());
    }

    #[test]
    fn rejects_declared_zip_bomb() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("bomb.zip");
        write_zip(&archive, &[("big.bin", &vec![0u8; 1024 * 1024])]);
        let limits = ArchiveLimits {
            max_entry_size: 1024,
            ..ArchiveLimits::default()
        };
        assert!(inspect_zip(&archive, &limits).is_err());
    }

    #[test]
    fn rejects_symlink_entries_even_when_target_is_relative() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("symlink.zip");
        {
            let file = File::create(&archive).unwrap();
            let mut zip = ZipWriter::new(file);
            zip.add_symlink(
                "link",
                "../../outside",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let error = inspect_zip(&archive, &ArchiveLimits::default()).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::SymlinkRejected);
    }

    #[test]
    fn rejects_file_directory_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("conflict.zip");
        write_zip(&archive, &[("a", b"file"), ("a/b", b"nested")]);
        let error = inspect_zip(&archive, &ArchiveLimits::default()).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::ArchiveInvalid);
    }

    #[test]
    fn rejects_excessive_compression_ratio() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("ratio.zip");
        write_zip(&archive, &[("zeros.bin", &vec![0u8; 1024 * 1024])]);
        let limits = ArchiveLimits {
            max_compression_ratio: 10,
            ..ArchiveLimits::default()
        };
        assert!(inspect_zip(&archive, &limits).is_err());
    }

    #[test]
    fn rejects_corrupt_archive() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("broken.zip");
        std::fs::write(&archive, b"PK\x03\x04 definitely not a zip").unwrap();
        let error = inspect_zip(&archive, &ArchiveLimits::default()).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::ArchiveInvalid);
    }

    #[test]
    fn extracts_transactionally_and_verifies_crc() {
        let (temp, fs) = safe_fs();
        let archive = temp.path().join("pack.zip");
        write_zip(
            &archive,
            &[("mods/a.jar", b"aaa"), ("config/settings.txt", b"bbb")],
        );
        let dest = SafeRelativePath::parse("instance").unwrap();
        let report =
            extract_zip_to_new_dir(&archive, &fs, &dest, &ArchiveLimits::default(), false).unwrap();
        assert_eq!(report.files, 2);
        assert_eq!(
            std::fs::read(temp.path().join("instance/mods/a.jar")).unwrap(),
            b"aaa"
        );
        assert_eq!(
            std::fs::read(temp.path().join("instance/config/settings.txt")).unwrap(),
            b"bbb"
        );
    }

    #[test]
    fn replace_swaps_directories_without_partial_result() {
        let (temp, fs) = safe_fs();
        let archive = temp.path().join("pack.zip");
        write_zip(&archive, &[("new.txt", b"new")]);
        let dest = SafeRelativePath::parse("instance").unwrap();
        fs.write_atomic(
            &SafeRelativePath::parse("instance/old.txt").unwrap(),
            b"old",
            false,
        )
        .unwrap();
        extract_zip_to_new_dir(&archive, &fs, &dest, &ArchiveLimits::default(), true).unwrap();
        assert!(!temp.path().join("instance/old.txt").exists());
        assert!(temp.path().join("instance/new.txt").exists());
    }

    #[test]
    fn merge_installs_into_existing_directory_and_cleans_staging() {
        let (temp, fs) = safe_fs();
        let archive = temp.path().join("pack.zip");
        write_zip(&archive, &[("mods/new.jar", b"new")]);
        let dest = SafeRelativePath::parse("instance").unwrap();
        fs.write_atomic(
            &SafeRelativePath::parse("instance/keep.txt").unwrap(),
            b"keep",
            false,
        )
        .unwrap();

        extract_zip_into(&archive, &fs, &dest, &ArchiveLimits::default(), false).unwrap();
        assert_eq!(
            std::fs::read(temp.path().join("instance/keep.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(
            std::fs::read(temp.path().join("instance/mods/new.jar")).unwrap(),
            b"new"
        );
        let staging_entries = std::fs::read_dir(temp.path().join(".staging"))
            .unwrap()
            .count();
        assert_eq!(staging_entries, 0, "staging directory must be cleaned up");
    }
}
