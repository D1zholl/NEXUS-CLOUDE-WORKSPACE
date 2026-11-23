//! Staging layout and sidecar metadata.
//!
//! Nothing is written directly into a managed root. Every transfer goes to
//! `<data>/downloads/staging/<task_id>/<file_id>.part` and gets a sibling
//! `.part.json` sidecar. The sidecar records the exact byte offset, remote
//! validators and expected hash. A resume is only attempted when the sidecar
//! still describes the same remote object; otherwise the transfer starts from
//! zero so bytes from two file versions can never be concatenated.

use nexus_core::{Clock, NexusError};
use serde::{Deserialize, Serialize};

use crate::security::fs::{SafeFs, SafeRelativePath};

const META_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagingMeta {
    pub version: u32,
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    pub expected_sha256: Option<String>,
    pub expected_size: Option<u64>,
    pub bytes_done: u64,
    pub updated_at: i64,
}

impl StagingMeta {
    pub fn new(url: &str, expected_sha256: Option<String>, expected_size: Option<u64>) -> Self {
        Self {
            version: META_VERSION,
            url: url.to_owned(),
            etag: None,
            last_modified: None,
            content_length: None,
            expected_sha256,
            expected_size,
            bytes_done: 0,
            updated_at: 0,
        }
    }

    pub fn is_compatible(&self, url: &str, sha256: Option<&str>) -> bool {
        self.version == META_VERSION && self.url == url && self.expected_sha256.as_deref() == sha256
    }
}

pub(crate) fn staging_dir_rel(task_id: &str) -> Result<SafeRelativePath, NexusError> {
    SafeRelativePath::parse(format!("downloads/staging/{task_id}"))
}

pub(crate) fn part_rel(task_id: &str, file_id: &str) -> Result<SafeRelativePath, NexusError> {
    SafeRelativePath::parse(format!("downloads/staging/{task_id}/{file_id}.part"))
}

pub(crate) fn meta_rel(task_id: &str, file_id: &str) -> Result<SafeRelativePath, NexusError> {
    SafeRelativePath::parse(format!("downloads/staging/{task_id}/{file_id}.part.json"))
}

pub(crate) fn ensure_staging_dir(
    fs: &SafeFs,
    task_id: &str,
) -> Result<SafeRelativePath, NexusError> {
    let relative = staging_dir_rel(task_id)?;
    fs.ensure_dir(&relative)?;
    Ok(relative)
}

pub(crate) fn read_meta(
    fs: &SafeFs,
    task_id: &str,
    file_id: &str,
) -> Result<Option<StagingMeta>, NexusError> {
    let relative = meta_rel(task_id, file_id)?;
    if !fs.exists(&relative)? {
        return Ok(None);
    }
    let bytes = fs.read(&relative, 64 * 1024)?;
    match serde_json::from_slice::<StagingMeta>(&bytes) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) => {
            tracing::warn!(
                file_id,
                %error,
                "staging sidecar is damaged; the transfer will restart"
            );
            Ok(None)
        }
    }
}

pub(crate) fn write_meta(
    fs: &SafeFs,
    task_id: &str,
    file_id: &str,
    meta: &StagingMeta,
    clock: &dyn Clock,
) -> Result<(), NexusError> {
    let mut meta = meta.clone();
    meta.updated_at = clock.now_ms();
    let bytes = serde_json::to_vec(&meta).map_err(|error| {
        NexusError::internal(format!("cannot encode staging metadata: {error}"))
    })?;
    let relative = meta_rel(task_id, file_id)?;
    fs.write_atomic(&relative, &bytes, true)?;
    Ok(())
}

pub(crate) fn remove_meta(fs: &SafeFs, task_id: &str, file_id: &str) {
    if let Ok(relative) = meta_rel(task_id, file_id)
        && let Err(error) = fs.remove_file(&relative)
        && error.code() != nexus_core::ErrorCode::NotFound
    {
        tracing::warn!(file_id, code = %error.code(), "cannot remove staging sidecar");
    }
}

pub(crate) fn staged_len(
    fs: &SafeFs,
    task_id: &str,
    file_id: &str,
) -> Result<Option<u64>, NexusError> {
    let relative = part_rel(task_id, file_id)?;
    let resolved = fs.resolve(&relative)?;
    if !resolved.exists {
        return Ok(None);
    }
    let metadata = std::fs::metadata(&resolved.absolute).map_err(|error| {
        NexusError::internal(format!(
            "cannot read staged file {}: {error}",
            resolved.absolute.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(NexusError::invalid_path(format!(
            "{} is not a regular staged file",
            resolved.absolute.display()
        )));
    }
    Ok(Some(metadata.len()))
}

pub(crate) fn remove_part(fs: &SafeFs, task_id: &str, file_id: &str) {
    if let Ok(relative) = part_rel(task_id, file_id)
        && let Err(error) = fs.remove_file(&relative)
        && error.code() != nexus_core::ErrorCode::NotFound
    {
        tracing::warn!(file_id, code = %error.code(), "cannot remove staged part");
    }
}

pub(crate) fn remove_task_staging(fs: &SafeFs, task_id: &str) {
    if let Ok(relative) = staging_dir_rel(task_id)
        && let Err(error) = fs.remove_dir_all(&relative)
        && error.code() != nexus_core::ErrorCode::NotFound
    {
        tracing::warn!(task_id, code = %error.code(), "cannot remove task staging");
    }
}

/// Removes a staging directory only when it contains no entries. A failed task
/// keeps partial files for retry, but an empty directory left by a checksum
/// rejection must not accumulate.
pub(crate) fn remove_staging_dir_if_empty(fs: &SafeFs, task_id: &str) {
    let Ok(relative) = staging_dir_rel(task_id) else {
        return;
    };
    let Ok(resolved) = fs.resolve(&relative) else {
        return;
    };
    if !resolved.exists {
        return;
    }
    let empty = std::fs::read_dir(&resolved.absolute)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    if empty
        && let Err(error) = fs.remove_dir_all(&relative)
        && error.code() != nexus_core::ErrorCode::NotFound
    {
        tracing::warn!(task_id, code = %error.code(), "cannot remove empty staging directory");
    }
}

pub(crate) fn absolute_part_path(
    fs: &SafeFs,
    task_id: &str,
    file_id: &str,
) -> Result<std::path::PathBuf, NexusError> {
    Ok(fs.root().join(part_rel(task_id, file_id)?.as_path()))
}
