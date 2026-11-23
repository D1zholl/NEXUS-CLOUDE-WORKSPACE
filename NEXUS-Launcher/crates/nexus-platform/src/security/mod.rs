//! Security primitives for the launcher.
//!
//! * [`fs`] is the only layer that should perform filesystem mutations for game
//!   data, instances and archives;
//! * [`net`] validates user and provider URLs and performs bounded HTTP
//!   requests;
//! * [`secrets`] stores tokens in Windows Credential Manager or DPAPI;
//! * [`redaction`] keeps secrets out of logs and `activity_logs`.

pub mod fs;
pub mod net;
pub mod redaction;
pub mod secrets;

pub use fs::archive::{
    ArchiveLimits, ExtractionReport, InspectedEntry, ZipInspection, extract_zip_into,
    extract_zip_to_new_dir, inspect_zip,
};
pub use fs::checksum::{ExpectedFile, FileDigest, Sha256Digest};
pub use fs::{
    RelativePath, ResolvedPath, SafeFs, SafeRelativePath, StagingDir, sha256_file, verify_file,
};
pub use net::{FetchLimits, NexusSourceUrl, SafeFetcher, SafeResponse, UrlPolicy, UrlPurpose};
pub use secrets::{
    MemorySecretStore, SecretKind, SecretRef, SecretStore, SecretValue, make_account_secret_ref,
    make_source_secret_ref, validate_secret_ref,
};

#[cfg(windows)]
pub use secrets::{DpapiSecretStore, KeyringSecretStore};

#[cfg(not(windows))]
pub use secrets::UnavailableSecretStore as KeyringSecretStore;

#[cfg(not(windows))]
pub use secrets::UnavailableSecretStore as DpapiSecretStore;
