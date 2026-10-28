//! : Mac device key (Secure Enclave, login Keychain, or a test file).
//!
//! The private key is kept inside the selected backend. Only the DER SPKI and
//! DER ECDSA signatures cross the `ExternalSigner` boundary.

use crate::{mac_ffi, platform::PlatformError};
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
    PublicKey,
};
use rsp_transport::identity::{ExternalSigner, SignError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_char, c_void, CString},
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

pub const KEYCHAIN_SERVICE: &str = "ai.gxlself.remotescreen.desktop";
pub const KEYCHAIN_ACCOUNT: &str = "device-key";
pub const KEYSTORE_ENV: &str = "RS_DESKTOP_KEYSTORE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    SecureEnclave,
    Keychain,
    File,
}

/// Keychain abstraction. Production uses the Swift implementation; tests use
/// an in-memory implementation so backend selection can be exercised without
/// touching a user's login Keychain.
pub trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<Vec<u8>>, PlatformError>;
    fn set(&self, value: &[u8]) -> Result<(), PlatformError>;
}

#[derive(Debug, Serialize, Deserialize)]
struct Metadata {
    backend: Backend,
    #[serde(rename = "createdAtUs")]
    created_at_us: u64,
    #[serde(rename = "spkiSha256")]
    spki_sha256: String,
}

struct SoftwareSigner {
    key: Mutex<SigningKey>,
    spki: Vec<u8>,
}

impl ExternalSigner for SoftwareSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignError> {
        let started = std::time::Instant::now();
        let key = self
            .key
            .lock()
            .map_err(|_| SignError("software signer lock poisoned".into()))?;
        let signature: Signature = key.sign(message);
        tracing::debug!(
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "device key signed"
        );
        Ok(signature.to_der().as_bytes().to_vec())
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

struct SecureEnclaveSigner {
    /// The retained Swift object is represented as an integer so this Rust
    /// wrapper remains `Send + Sync` without an unsafe implementation. Swift's
    /// immutable CryptoKit key is used behind the signing gate below.
    handle: usize,
    spki: Vec<u8>,
    signing_gate: Mutex<()>,
}

impl ExternalSigner for SecureEnclaveSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignError> {
        let started = std::time::Instant::now();
        let _guard = self
            .signing_gate
            .lock()
            .map_err(|_| SignError("secure enclave signer lock poisoned".into()))?;
        let mut signature = mac_ffi::RsDtBuf::empty();
        let mut error = mac_ffi::err_buf();
        // SAFETY: `handle` is retained by this signer until Drop; `message` is
        // borrowed for the duration of the synchronous Swift call; `signature`
        // and `error` point to writable Rust-owned output buffers.
        let status = unsafe {
            mac_ffi::rs_dt_se_sign(
                self.handle as *mut c_void,
                message.as_ptr(),
                message.len(),
                (&mut signature as *mut mac_ffi::RsDtBuf).cast::<c_void>(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let bytes = signature.take();
        if status != 0 {
            return Err(SignError(native_error(
                "secure enclave signing",
                status,
                &error,
            )));
        }
        tracing::debug!(
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "device key signed"
        );
        Ok(bytes)
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

impl Drop for SecureEnclaveSigner {
    fn drop(&mut self) {
        if self.handle == 0 {
            return;
        }
        // SAFETY: the handle was returned by `rs_dt_se_open` and is released
        // exactly once when the last Rust owner is dropped.
        unsafe { mac_ffi::rs_dt_se_close(self.handle as *mut c_void) };
        self.handle = 0;
    }
}

struct NativeKeychain;

impl SecretStore for NativeKeychain {
    fn get(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        let service = c_string(KEYCHAIN_SERVICE)?;
        let account = c_string(KEYCHAIN_ACCOUNT)?;
        let mut output = mac_ffi::RsDtBuf::empty();
        let mut error = mac_ffi::err_buf();
        // SAFETY: the C strings live through the call; output and error are
        // writable Rust-owned values that Swift fills synchronously.
        let status = unsafe {
            mac_ffi::rs_dt_kc_get(
                service.as_ptr(),
                account.as_ptr(),
                (&mut output as *mut mac_ffi::RsDtBuf).cast::<c_void>(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let value = output.take();
        match status {
            0 => Ok(Some(value)),
            1 => Ok(None),
            other => Err(PlatformError::Failed(native_error(
                "keychain read",
                other,
                &error,
            ))),
        }
    }

    fn set(&self, value: &[u8]) -> Result<(), PlatformError> {
        let service = c_string(KEYCHAIN_SERVICE)?;
        let account = c_string(KEYCHAIN_ACCOUNT)?;
        let mut error = mac_ffi::err_buf();
        // SAFETY: the C strings and value slice remain valid for the
        // synchronous call; Swift copies the value before returning.
        let status = unsafe {
            mac_ffi::rs_dt_kc_set(
                service.as_ptr(),
                account.as_ptr(),
                value.as_ptr(),
                value.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(PlatformError::Failed(native_error(
                "keychain write",
                status,
                &error,
            )))
        }
    }
}

/// Open the configured Mac device key. `RS_DESKTOP_KEYSTORE=file` is intended
/// for development and tests only; it deliberately emits a warning.
pub fn open_or_create(data_dir: &Path) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let prefer = match std::env::var(KEYSTORE_ENV).as_deref() {
        Ok("file") => {
            tracing::warn!("RS_DESKTOP_KEYSTORE=file is enabled; device key is stored on disk");
            Some(Backend::File)
        }
        _ => None,
    };
    let keychain: Arc<dyn SecretStore> = Arc::new(NativeKeychain);
    open_with(data_dir, prefer, keychain, se_available()).map(|(signer, _, _)| signer)
}

/// Testable backend entry point. `prefer` is an explicit backend choice used by
/// tests (and by the file environment override); otherwise metadata and the
/// platform capability determine the backend.
pub fn open_with(
    data_dir: &Path,
    prefer: Option<Backend>,
    keychain: Arc<dyn SecretStore>,
    se_is_available: bool,
) -> Result<(Arc<dyn ExternalSigner>, Backend, bool), PlatformError> {
    ensure_data_dir(data_dir)?;
    let metadata = read_metadata(data_dir)?;
    let selected = prefer
        .or_else(|| metadata.as_ref().map(|value| value.backend))
        .unwrap_or(if se_is_available {
            Backend::SecureEnclave
        } else {
            Backend::Keychain
        });

    match selected {
        Backend::File => match load_file(data_dir, metadata.as_ref()) {
            Ok(signer) => {
                write_metadata_if_missing(data_dir, Backend::File, &signer, metadata.as_ref())?;
                Ok((signer, Backend::File, false))
            }
            Err(error) => {
                tracing::warn!(%error, "file device key is unavailable; generating a new key");
                let (signer, pkcs8, _scalar, spki) = new_software_material()?;
                persist_file_key(data_dir, pkcs8.as_slice())?;
                write_metadata(data_dir, Backend::File, &spki)?;
                Ok((signer, Backend::File, true))
            }
        },
        Backend::Keychain => match load_keychain(keychain.as_ref(), metadata.as_ref()) {
            Ok(signer) => {
                write_metadata_if_missing(data_dir, Backend::Keychain, &signer, metadata.as_ref())?;
                Ok((signer, Backend::Keychain, false))
            }
            Err(error) => {
                tracing::warn!(%error, "device keychain entry is unavailable; generating a new key; device must be re-registered and paired");
                create_keychain(data_dir, keychain, "keychain entry was unavailable")
            }
        },
        Backend::SecureEnclave => {
            if !se_is_available {
                tracing::warn!(
                    "Secure Enclave is unavailable; using the login Keychain for the device key"
                );
                return create_keychain(data_dir, keychain, "Secure Enclave is unavailable");
            }
            match load_secure_enclave(data_dir, metadata.as_ref()) {
                Ok(signer) => {
                    write_metadata_if_missing(
                        data_dir,
                        Backend::SecureEnclave,
                        &signer,
                        metadata.as_ref(),
                    )?;
                    Ok((signer, Backend::SecureEnclave, false))
                }
                Err(error) => {
                    if metadata
                        .as_ref()
                        .is_some_and(|value| value.backend == Backend::SecureEnclave)
                    {
                        tracing::warn!(%error, "device key is unavailable, generating a new one; device must be re-registered and paired");
                    }
                    match create_secure_enclave(data_dir) {
                        Ok((signer, spki)) => {
                            write_metadata(data_dir, Backend::SecureEnclave, &spki)?;
                            Ok((signer, Backend::SecureEnclave, true))
                        }
                        Err(create_error) => {
                            tracing::warn!(%create_error, "Secure Enclave key creation failed; falling back to the login Keychain");
                            create_keychain(
                                data_dir,
                                keychain,
                                "Secure Enclave key creation failed",
                            )
                        }
                    }
                }
            }
        }
    }
}

/// Whether this Mac exposes the Secure Enclave API.
pub fn se_available() -> bool {
    // SAFETY: this is a pure Swift capability query with no pointer arguments.
    unsafe { mac_ffi::rs_dt_se_available() }
}

fn create_keychain(
    data_dir: &Path,
    keychain: Arc<dyn SecretStore>,
    reason: &str,
) -> Result<(Arc<dyn ExternalSigner>, Backend, bool), PlatformError> {
    tracing::warn!(%reason, "using the login Keychain for the device key");
    let (signer, _pkcs8, scalar, spki) = new_software_material()?;
    let secret = scalar;
    keychain.set(secret.as_slice())?;
    write_metadata(data_dir, Backend::Keychain, &spki)?;
    Ok((signer, Backend::Keychain, true))
}

/// `(signer, PKCS#8 DER, scalar, SPKI DER)`.
type SoftwareMaterial = (
    Arc<dyn ExternalSigner>,
    Zeroizing<Vec<u8>>,
    Zeroizing<Vec<u8>>,
    Vec<u8>,
);

fn new_software_material() -> Result<SoftwareMaterial, PlatformError> {
    let key = random_signing_key()?;
    let spki = public_spki(&key)?;
    let pkcs8 = Zeroizing::new(
        key.to_pkcs8_der()
            .map_err(|error| failure(format!("encode device key: {error}")))?
            .as_bytes()
            .to_vec(),
    );
    let scalar = Zeroizing::new(key.to_bytes().to_vec());
    let signer: Arc<dyn ExternalSigner> = Arc::new(SoftwareSigner {
        key: Mutex::new(key),
        spki: spki.clone(),
    });
    Ok((signer, pkcs8, scalar, spki))
}

fn random_signing_key() -> Result<SigningKey, PlatformError> {
    loop {
        let mut scalar = Zeroizing::new([0_u8; 32]);
        getrandom::fill(&mut *scalar)
            .map_err(|error| PlatformError::Failed(format!("random device key: {error}")))?;
        if let Ok(key) = SigningKey::from_slice(&*scalar) {
            return Ok(key);
        }
    }
}

fn load_file(
    data_dir: &Path,
    metadata: Option<&Metadata>,
) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let path = data_dir.join("device_key.p8");
    let encoded =
        Zeroizing::new(fs::read(&path).map_err(|error| io_error("read file key", error))?);
    let key = if encoded.len() == 32 {
        SigningKey::from_slice(encoded.as_slice())
            .map_err(|_| failure("invalid file device key"))?
    } else {
        SigningKey::from_pkcs8_der(encoded.as_slice())
            .map_err(|_| failure("invalid file device key"))?
    };
    let signer = software_signer_from_key(key)?;
    validate_metadata(&signer.spki_der(), metadata)?;
    set_file_mode(&path, 0o600)?;
    Ok(signer)
}

fn load_keychain(
    keychain: &dyn SecretStore,
    metadata: Option<&Metadata>,
) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let value = keychain
        .get()?
        .ok_or_else(|| failure("device keychain entry is missing"))?;
    let scalar = Zeroizing::new(value);
    if scalar.len() != 32 {
        return Err(failure("device keychain entry has an invalid length"));
    }
    let key = SigningKey::from_slice(scalar.as_slice())
        .map_err(|_| failure("invalid keychain device key"))?;
    let signer = software_signer_from_key(key)?;
    validate_metadata(&signer.spki_der(), metadata)?;
    Ok(signer)
}

fn software_signer_from_key(key: SigningKey) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let spki = public_spki(&key)?;
    Ok(Arc::new(SoftwareSigner {
        key: Mutex::new(key),
        spki,
    }))
}

fn load_secure_enclave(
    data_dir: &Path,
    metadata: Option<&Metadata>,
) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let path = data_dir.join("device_key.se");
    let blob = fs::read(&path).map_err(|error| io_error("read Secure Enclave key", error))?;
    let (signer, spki) = open_secure_enclave(&blob)?;
    validate_metadata(&spki, metadata)?;
    set_file_mode(&path, 0o600)?;
    Ok(signer)
}

fn create_secure_enclave(
    data_dir: &Path,
) -> Result<(Arc<dyn ExternalSigner>, Vec<u8>), PlatformError> {
    let mut blob = mac_ffi::RsDtBuf::empty();
    let mut x963 = mac_ffi::RsDtBuf::empty();
    let mut error = mac_ffi::err_buf();
    // SAFETY: all output pointers refer to writable Rust values and Swift
    // returns before those values are read.
    let status = unsafe {
        mac_ffi::rs_dt_se_create(
            (&mut blob as *mut mac_ffi::RsDtBuf).cast::<c_void>(),
            (&mut x963 as *mut mac_ffi::RsDtBuf).cast::<c_void>(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    let blob_bytes = blob.take();
    let _created_x963 = x963.take();
    if status != 0 {
        return Err(failure(native_error(
            "Secure Enclave key creation",
            status,
            &error,
        )));
    }
    let (signer, spki) = open_secure_enclave(&blob_bytes)?;
    write_private_file(&data_dir.join("device_key.se"), &blob_bytes)?;
    Ok((signer, spki))
}

fn open_secure_enclave(blob: &[u8]) -> Result<(Arc<dyn ExternalSigner>, Vec<u8>), PlatformError> {
    let mut x963 = mac_ffi::RsDtBuf::empty();
    let mut error = mac_ffi::err_buf();
    // SAFETY: `blob` remains alive for the synchronous Swift call; output and
    // error point to writable Rust-owned values.
    let handle = unsafe {
        mac_ffi::rs_dt_se_open(
            blob.as_ptr(),
            blob.len(),
            (&mut x963 as *mut mac_ffi::RsDtBuf).cast::<c_void>(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    let x963_bytes = x963.take();
    let handle = handle.cast::<c_void>();
    if handle.is_null() {
        return Err(failure(if mac_ffi::err_string(&error).is_empty() {
            "Secure Enclave key blob could not be opened".into()
        } else {
            mac_ffi::err_string(&error)
        }));
    }
    let spki = match spki_from_x963(&x963_bytes) {
        Ok(spki) => spki,
        Err(error) => {
            // SAFETY: `handle` is the retained object returned by Swift and
            // must be released when the public key is malformed.
            unsafe { mac_ffi::rs_dt_se_close(handle) };
            return Err(error);
        }
    };
    let signer: Arc<dyn ExternalSigner> = Arc::new(SecureEnclaveSigner {
        handle: handle as usize,
        spki: spki.clone(),
        signing_gate: Mutex::new(()),
    });
    Ok((signer, spki))
}

fn persist_file_key(data_dir: &Path, pkcs8: &[u8]) -> Result<(), PlatformError> {
    write_private_file(&data_dir.join("device_key.p8"), pkcs8)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), PlatformError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| io_error("open private key file", error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write private key file", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync private key file", error))?;
    set_file_mode(path, 0o600)
}

fn write_metadata(data_dir: &Path, backend: Backend, spki: &[u8]) -> Result<(), PlatformError> {
    let metadata = Metadata {
        backend,
        created_at_us: unix_micros(),
        spki_sha256: spki_hash(spki),
    };
    let bytes = serde_json::to_vec(&metadata)
        .map_err(|error| failure(format!("serialize device key metadata: {error}")))?;
    write_private_file(&data_dir.join("device_key.json"), &bytes)
}

fn write_metadata_if_missing(
    data_dir: &Path,
    backend: Backend,
    signer: &Arc<dyn ExternalSigner>,
    metadata: Option<&Metadata>,
) -> Result<(), PlatformError> {
    if metadata.is_none() {
        write_metadata(data_dir, backend, &signer.spki_der())?;
    }
    Ok(())
}

fn read_metadata(data_dir: &Path) -> Result<Option<Metadata>, PlatformError> {
    let path = data_dir.join("device_key.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read device key metadata", error)),
    };
    set_file_mode(&path, 0o600)?;
    match serde_json::from_slice(&bytes) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) => {
            tracing::warn!(%error, "device key metadata is invalid; selecting a new backend");
            Ok(None)
        }
    }
}

fn validate_metadata(spki: &[u8], metadata: Option<&Metadata>) -> Result<(), PlatformError> {
    if let Some(metadata) = metadata {
        if metadata.spki_sha256 != spki_hash(spki) {
            return Err(failure("device key metadata does not match the key"));
        }
    }
    Ok(())
}

fn public_spki(key: &SigningKey) -> Result<Vec<u8>, PlatformError> {
    key.verifying_key()
        .to_public_key_der()
        .map(|der| der.as_bytes().to_vec())
        .map_err(|error| failure(format!("encode device key public key: {error}")))
}

fn spki_from_x963(x963: &[u8]) -> Result<Vec<u8>, PlatformError> {
    if x963.len() != 65 || x963.first() != Some(&4) {
        return Err(failure(
            "Secure Enclave returned an invalid P-256 public key",
        ));
    }
    let public = PublicKey::from_sec1_bytes(x963)
        .map_err(|_| failure("Secure Enclave returned an invalid P-256 public key"))?;
    public
        .to_public_key_der()
        .map(|der| der.as_bytes().to_vec())
        .map_err(|error| failure(format!("encode device key public key: {error}")))
}

fn spki_hash(spki: &[u8]) -> String {
    let digest = Sha256::digest(spki);
    let mut result = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn ensure_data_dir(path: &Path) -> Result<(), PlatformError> {
    fs::create_dir_all(path).map_err(|error| io_error("create device key directory", error))?;
    set_file_mode(path, 0o700)
}

fn set_file_mode(path: &Path, mode: u32) -> Result<(), PlatformError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| io_error("set device key permissions", error))?;
    }
    Ok(())
}

fn c_string(value: &str) -> Result<CString, PlatformError> {
    CString::new(value).map_err(|_| failure("keychain identifier contains NUL"))
}

fn native_error(operation: &str, status: i32, error: &[c_char]) -> String {
    let detail = mac_ffi::err_string(error);
    if detail.is_empty() {
        format!("{operation} failed (status {status})")
    } else {
        format!("{operation} failed (status {status}): {detail}")
    }
}

fn unix_micros() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn io_error(operation: &str, error: io::Error) -> PlatformError {
    failure(format!("{operation}: {error}"))
}

fn failure(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed(message.into())
}
