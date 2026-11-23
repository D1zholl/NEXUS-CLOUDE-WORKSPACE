//! Streaming SHA-256 and size verification.
//!
//! Files are hashed from a handle opened without following a final reparse
//! point on Windows. The size is checked before and after reading, so a file
//! that grows or shrinks while it is being hashed is reported as tampered with
//! instead of silently accepted.

use std::io::{self, Read};
use std::path::Path;

use nexus_core::NexusError;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::guard::{is_reparse_point, map_io, open_read_no_follow, reparse_error};

const CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(raw: &str) -> Result<Self, NexusError> {
        if raw.len() != 64 {
            return Err(NexusError::invalid_argument(
                "SHA-256 must contain exactly 64 hexadecimal characters",
            ));
        }
        let mut output = [0u8; 32];
        let (pairs, remainder) = raw.as_bytes().as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(NexusError::invalid_argument("SHA-256 has an odd length"));
        }
        for (index, pair) in pairs.iter().enumerate() {
            let pair = std::str::from_utf8(pair)
                .map_err(|_| NexusError::invalid_argument("SHA-256 contains invalid UTF-8"))?;
            output[index] = u8::from_str_radix(pair, 16).map_err(|_| {
                NexusError::invalid_argument("SHA-256 contains non-hexadecimal characters")
            })?;
        }
        Ok(Self(output))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    pub fn constant_time_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDigest {
    pub sha256: Sha256Digest,
    pub size: u64,
}

impl FileDigest {
    pub fn verify(&self, expected: &ExpectedFile) -> Result<(), NexusError> {
        if let Some(size) = expected.size
            && self.size != size
        {
            return Err(NexusError::checksum_mismatch(format!(
                "expected {size} bytes, got {}",
                self.size
            )));
        }
        if let Some(hash) = expected.sha256
            && !self.sha256.constant_time_eq(&hash)
        {
            return Err(NexusError::checksum_mismatch(format!(
                "expected {}, got {}",
                hash, self.sha256
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedFile {
    pub sha256: Option<Sha256Digest>,
    pub size: Option<u64>,
}

impl ExpectedFile {
    pub fn sha256(hash: Sha256Digest) -> Self {
        Self {
            sha256: Some(hash),
            size: None,
        }
    }

    pub fn new(sha256: Sha256Digest, size: u64) -> Self {
        Self {
            sha256: Some(sha256),
            size: Some(size),
        }
    }

    pub fn size_only(size: u64) -> Self {
        Self {
            sha256: None,
            size: Some(size),
        }
    }
}

pub fn hash_file(path: &Path, max_size: u64) -> Result<FileDigest, NexusError> {
    let file = open_read_no_follow(path).map_err(|error| map_io(path, error))?;
    let before = file.metadata().map_err(|error| map_io(path, error))?;
    if is_reparse_point(&before) {
        return Err(reparse_error(path));
    }
    if !before.is_file() {
        return Err(NexusError::invalid_path(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if before.len() > max_size {
        return Err(NexusError::file_too_large(format!(
            "{} is {} bytes, limit is {max_size}",
            path.display(),
            before.len()
        )));
    }

    let (sha256, size) = hash_reader(file, max_size)?;

    let after = std::fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    if is_reparse_point(&after) {
        return Err(reparse_error(path));
    }
    if after.len() != size || before.len() != size {
        return Err(NexusError::checksum_mismatch(format!(
            "{} changed while it was being hashed",
            path.display()
        )));
    }

    Ok(FileDigest { sha256, size })
}

pub fn hash_reader(
    mut reader: impl Read,
    max_size: u64,
) -> Result<(Sha256Digest, u64), NexusError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;

    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            NexusError::internal(format!("cannot read file while hashing: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| NexusError::file_too_large("file size overflow"))?;
        if total > max_size {
            return Err(NexusError::file_too_large(format!(
                "input exceeds limit of {max_size} bytes"
            )));
        }
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok((Sha256Digest(bytes), total))
}

pub fn verify_file(
    path: &Path,
    expected: &ExpectedFile,
    max_size: u64,
) -> Result<FileDigest, NexusError> {
    let digest = hash_file(path, max_size)?;
    digest.verify(expected)?;
    Ok(digest)
}

/// Copies at most `max_size` bytes from `reader` to `writer`, hashing the
/// stream as it goes. Used for downloads and archive entries so the size limit
/// is enforced while data is in flight, not only from an untrusted header.
pub fn copy_limited(
    mut reader: impl Read,
    mut writer: impl io::Write,
    max_size: u64,
) -> Result<FileDigest, NexusError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| NexusError::internal(format!("read failed: {error}")))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| NexusError::file_too_large("stream size overflow"))?;
        if total > max_size {
            return Err(NexusError::file_too_large(format!(
                "stream exceeds limit of {max_size} bytes"
            )));
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| NexusError::internal(format!("write failed: {error}")))?;
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(FileDigest {
        sha256: Sha256Digest(bytes),
        size: total,
    })
}

pub fn file_size(path: &Path, max_size: u64) -> Result<u64, NexusError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| map_io(path, error))?;
    if is_reparse_point(&metadata) {
        return Err(reparse_error(path));
    }
    if !metadata.is_file() {
        return Err(NexusError::invalid_path(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > max_size {
        return Err(NexusError::file_too_large(format!(
            "{} is {} bytes, limit is {max_size}",
            path.display(),
            metadata.len()
        )));
    }
    Ok(metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn known_sha256_vectors() {
        let known = Sha256Digest::from_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .unwrap();
        assert_eq!(hash_reader(Cursor::new(b""), 10).unwrap().0, known);

        let hello = Sha256Digest::from_hex(
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        )
        .unwrap();
        assert_eq!(hash_reader(Cursor::new(b"hello"), 10).unwrap().0, hello);
        assert_ne!(known.to_hex(), hello.to_hex());
    }

    #[test]
    fn digest_parsing_is_strict() {
        assert!(Sha256Digest::from_hex("abc").is_err());
        assert!(Sha256Digest::from_hex(&"z".repeat(64)).is_err());
        assert!(Sha256Digest::from_hex(&"A".repeat(64)).is_ok());
    }

    #[test]
    fn size_limit_is_enforced_during_streaming() {
        let error = hash_reader(Cursor::new(vec![0u8; 10]), 9).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::FileTooLarge);
    }

    #[test]
    fn file_verification_detects_changed_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jar");
        std::fs::write(&path, b"original").unwrap();
        let original = hash_file(&path, 1024).unwrap();

        std::fs::write(&path, b"substituted").unwrap();
        let error = verify_file(&path, &ExpectedFile::sha256(original.sha256), 1024).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::ChecksumMismatch);
    }

    #[test]
    fn file_verification_enforces_expected_size() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jar");
        std::fs::write(&path, b"1234").unwrap();
        let error = verify_file(&path, &ExpectedFile::size_only(5), 1024).unwrap_err();
        assert_eq!(error.code(), nexus_core::ErrorCode::ChecksumMismatch);
    }
}
