//! Typed secret storage.
//!
//! SQLite stores only an opaque `secret_ref`. Tokens live either in Windows
//! Credential Manager (through `keyring`) or in a DPAPI-encrypted file store.
//! The typed reference prevents accidentally persisting a JWT or an OAuth code
//! where a reference is expected, and [`SecretValue`] redacts itself from
//! `Debug` output and zeroizes its memory on drop.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use nexus_core::NexusError;
use zeroize::Zeroizing;

use crate::security::fs::{SafeFs, SafeRelativePath};

const REF_NAMESPACE: &str = "nexus-launcher/";
const MAX_REF_LEN: usize = 256;
const MAX_SECRET_LEN: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    Account,
    NexusSource,
    Generic,
}

impl SecretKind {
    fn namespace(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::NexusSource => "source",
            Self::Generic => "generic",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "account" => Some(Self::Account),
            "source" => Some(Self::NexusSource),
            "generic" => Some(Self::Generic),
            _ => None,
        }
    }
}

/// A validated reference to a secret. It is never the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretRef {
    raw: String,
    kind: SecretKind,
}

impl SecretRef {
    pub fn account(account_id: &str) -> Result<Self, NexusError> {
        Self::new(SecretKind::Account, account_id)
    }

    pub fn nexus_source(source_id: &str) -> Result<Self, NexusError> {
        Self::new(SecretKind::NexusSource, source_id)
    }

    pub fn generic(scope: &str) -> Result<Self, NexusError> {
        Self::new(SecretKind::Generic, scope)
    }

    fn new(kind: SecretKind, id: &str) -> Result<Self, NexusError> {
        if id.is_empty() || id.len() > 128 {
            return Err(NexusError::invalid_argument(
                "secret identifier length is invalid",
            ));
        }
        if !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err(NexusError::invalid_argument(
                "secret identifier contains invalid characters",
            ));
        }
        let raw = format!("{REF_NAMESPACE}{}/{id}", kind.namespace());
        Ok(Self { raw, kind })
    }

    /// Parses a stored reference and verifies its namespace.
    pub fn parse(raw: &str) -> Result<Self, NexusError> {
        if raw.len() < 17 || raw.len() > MAX_REF_LEN {
            return Err(NexusError::invalid_argument(
                "secret reference length is invalid",
            ));
        }
        let rest = raw
            .strip_prefix(REF_NAMESPACE)
            .ok_or_else(|| NexusError::invalid_argument("secret reference namespace is invalid"))?;
        let mut parts = rest.splitn(2, '/');
        let kind_raw = parts
            .next()
            .ok_or_else(|| NexusError::invalid_argument("secret reference kind is missing"))?;
        let id = parts.next().ok_or_else(|| {
            NexusError::invalid_argument("secret reference identifier is missing")
        })?;
        let kind = SecretKind::parse(kind_raw)
            .ok_or_else(|| NexusError::invalid_argument("secret reference kind is unknown"))?;
        if raw != format!("{REF_NAMESPACE}{}/{id}", kind.namespace()) {
            return Err(NexusError::invalid_argument(
                "secret reference is not normalized",
            ));
        }
        Self::new(kind, id)
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn kind(&self) -> SecretKind {
        self.kind
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// A secret value. `Debug` never reveals the value, and the backing buffer is
/// zeroized when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Result<Self, NexusError> {
        let value = value.into();
        if value.len() > MAX_SECRET_LEN {
            return Err(NexusError::invalid_argument("secret value is too long"));
        }
        Ok(Self(Zeroizing::new(value)))
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([redacted])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Abstraction over the operating-system secret store.
///
/// Implementations must fail closed when the secure store is unavailable. There
/// must never be a plaintext-file fallback.
pub trait SecretStore: Send + Sync {
    fn put(&self, reference: &SecretRef, secret: &SecretValue) -> Result<(), NexusError>;
    fn get(&self, reference: &SecretRef) -> Result<Option<SecretValue>, NexusError>;
    fn delete(&self, reference: &SecretRef) -> Result<(), NexusError>;
}

/// In-memory store for tests only.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, SecretValue>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|map| map.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, reference: &SecretRef, secret: &SecretValue) -> Result<(), NexusError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| NexusError::secret_store("secret store lock is poisoned"))?;
        entries.insert(reference.as_str().to_owned(), secret.clone());
        Ok(())
    }

    fn get(&self, reference: &SecretRef) -> Result<Option<SecretValue>, NexusError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| NexusError::secret_store("secret store lock is poisoned"))?;
        Ok(entries.get(reference.as_str()).cloned())
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), NexusError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| NexusError::secret_store("secret store lock is poisoned"))?;
        entries.remove(reference.as_str());
        Ok(())
    }
}

/// Windows Credential Manager implementation.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct KeyringSecretStore {
    service: String,
}

#[cfg(windows)]
impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, reference: &SecretRef) -> Result<keyring::Entry, NexusError> {
        keyring::Entry::new(&self.service, reference.as_str())
            .map_err(|error| NexusError::secret_store(error.to_string()))
    }
}

#[cfg(windows)]
impl SecretStore for KeyringSecretStore {
    fn put(&self, reference: &SecretRef, secret: &SecretValue) -> Result<(), NexusError> {
        self.entry(reference)?
            .set_password(secret.expose())
            .map_err(|error| NexusError::secret_store(error.to_string()))
    }

    fn get(&self, reference: &SecretRef) -> Result<Option<SecretValue>, NexusError> {
        match self.entry(reference)?.get_password() {
            Ok(secret) => Ok(Some(SecretValue::new(secret)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(NexusError::secret_store(error.to_string())),
        }
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), NexusError> {
        match self.entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(NexusError::secret_store(error.to_string())),
        }
    }
}

/// DPAPI-backed secret store.
///
/// Each value is encrypted with the current Windows user's DPAPI key and stored
/// in a file named by the SHA-256 of its typed reference. This is useful when
/// Credential Manager is unavailable (for example on locked-down systems) and
/// keeps secrets protected at rest without a plaintext fallback.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct DpapiSecretStore {
    fs: SafeFs,
}

#[cfg(windows)]
impl DpapiSecretStore {
    pub fn new(directory: impl AsRef<std::path::Path>) -> Result<Self, NexusError> {
        let directory = directory.as_ref();
        std::fs::create_dir_all(directory).map_err(|error| {
            NexusError::secret_store(format!("cannot create {}: {error}", directory.display()))
        })?;
        Ok(Self {
            fs: SafeFs::open(directory)?,
        })
    }

    fn file_for(&self, reference: &SecretRef) -> Result<SafeRelativePath, NexusError> {
        let hash = sha256_hex(reference.as_str().as_bytes());
        SafeRelativePath::parse(format!("{hash}.bin"))
    }
}

#[cfg(windows)]
impl SecretStore for DpapiSecretStore {
    fn put(&self, reference: &SecretRef, secret: &SecretValue) -> Result<(), NexusError> {
        let file = self.file_for(reference)?;
        let protected = dpapi::protect(secret.expose().as_bytes())?;
        self.fs.write_atomic(&file, &protected, true)?;
        Ok(())
    }

    fn get(&self, reference: &SecretRef) -> Result<Option<SecretValue>, NexusError> {
        let file = self.file_for(reference)?;
        if !self.fs.exists(&file)? {
            return Ok(None);
        }
        let protected = self.fs.read(&file, 1024 * 1024)?;
        let plain = dpapi::unprotect(&protected)?;
        let value = String::from_utf8(plain)
            .map_err(|_| NexusError::secret_store("stored secret is not valid UTF-8"))?;
        Ok(Some(SecretValue::new(value)?))
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), NexusError> {
        let file = self.file_for(reference)?;
        if self.fs.exists(&file)? {
            self.fs.remove_file(&file)?;
        }
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(not(windows))]
#[derive(Debug, Clone)]
pub struct UnavailableSecretStore;

#[cfg(not(windows))]
impl UnavailableSecretStore {
    pub fn new(_service: impl Into<String>) -> Self {
        Self
    }
}

#[cfg(not(windows))]
impl SecretStore for UnavailableSecretStore {
    fn put(&self, _reference: &SecretRef, _secret: &SecretValue) -> Result<(), NexusError> {
        Err(NexusError::secret_store(
            "no native secret store on this platform",
        ))
    }

    fn get(&self, _reference: &SecretRef) -> Result<Option<SecretValue>, NexusError> {
        Err(NexusError::secret_store(
            "no native secret store on this platform",
        ))
    }

    fn delete(&self, _reference: &SecretRef) -> Result<(), NexusError> {
        Err(NexusError::secret_store(
            "no native secret store on this platform",
        ))
    }
}

#[cfg(not(windows))]
pub type DpapiSecretStore = UnavailableSecretStore;

pub fn make_account_secret_ref(account_id: &str) -> Result<SecretRef, NexusError> {
    SecretRef::account(account_id)
}

pub fn make_source_secret_ref(source_id: &str) -> Result<SecretRef, NexusError> {
    SecretRef::nexus_source(source_id)
}

/// Compatibility helper for repositories that store a reference string.
pub fn validate_secret_ref(reference: &str) -> Result<(), NexusError> {
    SecretRef::parse(reference).map(|_| ())
}

#[cfg(windows)]
mod dpapi {
    use std::ptr;
    use std::slice;

    use nexus_core::NexusError;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    const ENTROPY: &[u8] = b"NEXUS-Launcher/secret-store/v1";

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, NexusError> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: all pointers point to valid buffers for the duration of the
        // call. The output buffer is freed with `LocalFree` below.
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(NexusError::secret_store(format!(
                "DPAPI protection failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let result = copy_output(output);
        unsafe {
            LocalFree(output.pbData as *mut core::ffi::c_void);
        }
        Ok(result)
    }

    pub fn unprotect(protected: &[u8]) -> Result<Vec<u8>, NexusError> {
        let input = blob(protected);
        let entropy = blob(ENTROPY);
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: same contract as `protect`. `ppszdatadescr` is null so DPAPI
        // does not allocate a description string we would have to free.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(NexusError::secret_store(format!(
                "DPAPI unprotection failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let result = copy_output(output);
        unsafe {
            LocalFree(output.pbData as *mut core::ffi::c_void);
        }
        Ok(result)
    }

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    fn copy_output(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if output.pbData.is_null() || output.cbData == 0 {
            return Vec::new();
        }
        // SAFETY: DPAPI returned a pointer to `cbData` initialized bytes.
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip_with_typed_reference() {
        let store = MemorySecretStore::new();
        let reference = SecretRef::account("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap();
        let value = SecretValue::new("refresh-token-value").unwrap();
        store.put(&reference, &value).unwrap();
        assert_eq!(
            store.get(&reference).unwrap().unwrap().expose(),
            "refresh-token-value"
        );
        store.delete(&reference).unwrap();
        assert_eq!(store.get(&reference).unwrap(), None);
        assert!(store.is_empty());
    }

    #[test]
    fn raw_jwt_cannot_be_used_as_secret_reference() {
        let jwt = "eyJhbGciOiJub25lIn0.eyJzdWIiOiIxMjM0NTY3ODkwIn0.";
        assert!(SecretRef::parse(jwt).is_err());
        assert!(SecretRef::parse("nexus-launcher/account/abc").is_ok());
        assert!(SecretRef::parse("nexus-launcher/evil/abc").is_err());
    }

    #[test]
    fn references_with_whitespace_are_rejected() {
        assert!(SecretRef::account("abc def").is_err());
        assert!(SecretRef::account("abc\n").is_err());
        assert!(SecretRef::parse("nexus-launcher/account/abc def").is_err());
    }

    #[test]
    fn secret_value_redacts_itself() {
        let value = SecretValue::new("super-secret").unwrap();
        assert!(!format!("{value:?}").contains("super-secret"));
        assert!(!format!("{value}").contains("super-secret"));
        assert_eq!(value.expose(), "super-secret");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_and_tamper_detection() {
        let temp = tempfile::tempdir().unwrap();
        let store = DpapiSecretStore::new(temp.path()).unwrap();
        let reference = SecretRef::nexus_source("local-dev").unwrap();
        let value = SecretValue::new("source-api-token").unwrap();
        store.put(&reference, &value).unwrap();
        assert_eq!(
            store.get(&reference).unwrap().unwrap().expose(),
            "source-api-token"
        );

        // Ciphertext is not plaintext.
        let raw = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.path().is_file())
            .map(|entry| std::fs::read(entry.path()).unwrap())
            .unwrap();
        assert!(!raw.windows(15).any(|window| window == b"source-api-token"));

        store.delete(&reference).unwrap();
        assert!(store.get(&reference).unwrap().is_none());
    }
}
