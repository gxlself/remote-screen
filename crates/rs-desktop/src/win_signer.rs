//! ：Windows 设备密钥（CNG + TPM，退回软件 KSP；开发用文件）。见 04 · 电脑端。

use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use crate::platform::PlatformError;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
    PublicKey,
};
use rsp_transport::identity::{ExternalSigner, SignError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{NTE_BAD_KEYSET, NTE_EXISTS},
        Security::Cryptography::{
            NCryptCreatePersistedKey, NCryptDeleteKey, NCryptExportKey, NCryptFinalizeKey,
            NCryptFreeObject, NCryptOpenKey, NCryptOpenStorageProvider, NCryptSetProperty,
            NCryptSignHash, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
            BCRYPT_ECDSA_PUBLIC_P256_MAGIC, CERT_KEY_SPEC, NCRYPT_EXPORT_POLICY_PROPERTY,
            NCRYPT_FLAGS, NCRYPT_KEY_HANDLE, NCRYPT_PROV_HANDLE, NCRYPT_SILENT_FLAG,
        },
    },
};
use zeroize::Zeroizing;

pub const KEY_NAME_PREFIX: &str = "RemoteScreen-";
pub const KEYSTORE_ENV: &str = "RS_DESKTOP_KEYSTORE";
pub const TPM_PROVIDER: &str = "Microsoft Platform Crypto Provider";
pub const SOFTWARE_PROVIDER: &str = "Microsoft Software Key Storage Provider";

const METADATA_FILE: &str = "device_key.json";
const PRIVATE_KEY_FILE: &str = "device_key.p8";
const METADATA_TMP: &str = "device_key.json.tmp";
const PRIVATE_KEY_TMP: &str = "device_key.p8.tmp";
const ECC_PUBLIC_BLOB_SIZE: usize = 8 + 32 + 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Tpm,
    SoftwareKsp,
    File,
}

impl Backend {
    fn provider(self) -> Option<&'static str> {
        match self {
            Self::Tpm => Some(TPM_PROVIDER),
            Self::SoftwareKsp => Some(SOFTWARE_PROVIDER),
            Self::File => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Metadata {
    backend: Backend,
    #[serde(rename = "keyName", skip_serializing_if = "Option::is_none")]
    key_name: Option<String>,
    #[serde(rename = "createdAtUs")]
    created_at_us: u64,
    #[serde(rename = "spkiSha256")]
    spki_sha256: String,
}

#[derive(Debug, thiserror::Error)]
enum KeyError {
    #[error("{operation}: {hresult:#010x}")]
    Cng {
        operation: &'static str,
        hresult: u32,
    },
    #[error(transparent)]
    Other(#[from] PlatformError),
}

impl KeyError {
    fn cng(operation: &'static str, error: windows::core::Error) -> Self {
        Self::Cng {
            operation,
            hresult: error.code().0 as u32,
        }
    }

    fn is(&self, code: windows::core::HRESULT) -> bool {
        matches!(self, Self::Cng { hresult, .. } if *hresult == code.0 as u32)
    }
}

impl From<KeyError> for PlatformError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Other(error) => error,
            error => Self::Failed(error.to_string()),
        }
    }
}

struct NcryptKey(NCRYPT_KEY_HANDLE);

// SAFETY: After construction the handle is only used inside WinSigner's
// mutex. NCrypt process handles may move between threads with serialized use.
unsafe impl Send for NcryptKey {}
// SAFETY: All shared handle operations are serialized by WinSigner's mutex.
unsafe impl Sync for NcryptKey {}

impl NcryptKey {
    fn delete(&mut self) -> Result<(), KeyError> {
        // SAFETY: This wrapper exclusively owns a live key handle. Successful
        // deletion consumes that handle; resetting it prevents a second free.
        unsafe { NCryptDeleteKey(self.0, 0) }
            .map_err(|error| KeyError::cng("NCryptDeleteKey", error))?;
        self.0 = NCRYPT_KEY_HANDLE::default();
        Ok(())
    }
}

impl Drop for NcryptKey {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: The handle is owned by this wrapper and is no longer
            // accessible by any other thread when dropped.
            let _ = unsafe { NCryptFreeObject(self.0.into()) };
        }
    }
}

struct NcryptProvider(NCRYPT_PROV_HANDLE);

// SAFETY: The provider is kept with its key and all use is serialized by the
// WinSigner mutex. Moving the opaque CNG handle between worker threads is
// valid for the lifetime of the provider.
unsafe impl Send for NcryptProvider {}
// SAFETY: See the Send impl; no concurrent access escapes the mutex.
unsafe impl Sync for NcryptProvider {}

impl Drop for NcryptProvider {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: This wrapper owns the provider handle. CngKey declares
            // its key first, so the key is freed before its provider.
            let _ = unsafe { NCryptFreeObject(self.0.into()) };
        }
    }
}

struct CngKey {
    key: NcryptKey,
    _provider: NcryptProvider,
    // New keys are deleted on every error path until metadata is committed.
    delete_on_drop: bool,
}

impl Drop for CngKey {
    fn drop(&mut self) {
        if self.delete_on_drop {
            let _ = self.key.delete();
        }
    }
}

enum Key {
    Cng(CngKey),
    File(SigningKey),
}

pub struct WinSigner {
    backend: Backend,
    key_name: Option<String>,
    spki: Vec<u8>,
    key: Mutex<Key>,
    last_sign_us: AtomicU64,
}

impl WinSigner {
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// File 后端为 None。
    pub fn key_name(&self) -> Option<&str> {
        self.key_name.as_deref()
    }

    /// 最近一次签名耗时（微秒），没签过为 0。包含等待签名锁的时间。
    pub fn last_sign_us(&self) -> u64 {
        self.last_sign_us.load(Ordering::Relaxed)
    }

    fn from_key(
        backend: Backend,
        key_name: Option<String>,
        key: Key,
    ) -> Result<Self, PlatformError> {
        let spki = match &key {
            Key::Cng(cng) => export_spki(&cng.key)?,
            Key::File(key) => key
                .verifying_key()
                .to_public_key_der()
                .map_err(|_| PlatformError::Failed("encode device SPKI: invalid key".into()))?
                .as_bytes()
                .to_vec(),
        };
        Ok(Self {
            backend,
            key_name,
            spki,
            key: Mutex::new(key),
            last_sign_us: AtomicU64::new(0),
        })
    }

    fn sign_inner(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let key = self
            .key
            .lock()
            .map_err(|_| SignError("device signer mutex poisoned".into()))?;
        let signature: Signature = match &*key {
            Key::File(key) => key
                .try_sign(msg)
                .map_err(|_| SignError("ECDSA-P256-SHA256 signing failed".into()))?,
            Key::Cng(cng) => {
                let hash = Sha256::digest(msg);
                let mut raw = [0u8; 64];
                let mut length = 0;
                // SAFETY: The mutex protects a live key, and hash/raw/length
                // are valid for this synchronous call. ECDSA needs no padding.
                unsafe {
                    NCryptSignHash(
                        cng.key.0,
                        None,
                        &hash,
                        Some(&mut raw),
                        &mut length,
                        NCRYPT_SILENT_FLAG,
                    )
                }
                .map_err(|error| SignError(KeyError::cng("NCryptSignHash", error).to_string()))?;
                if length != 64 {
                    return Err(SignError("NCryptSignHash: invalid signature length".into()));
                }
                Signature::from_slice(&raw)
                    .map_err(|_| SignError("NCryptSignHash: invalid ECDSA signature".into()))?
            }
        };
        Ok(signature.to_der().as_bytes().to_vec())
    }
}

impl ExternalSigner for WinSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let started = Instant::now();
        let result = self.sign_inner(msg);
        let elapsed_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        // Keep 0 reserved for the "no signature yet" sentinel even when a
        // very fast software-KSP operation rounds below one microsecond.
        let recorded_us = elapsed_us.max(1);
        self.last_sign_us.store(recorded_us, Ordering::Relaxed);
        let elapsed_ms = recorded_us as f64 / 1_000.0;
        tracing::debug!(backend = ?self.backend, elapsed_ms, "device key signing completed");
        if recorded_us > 500_000 {
            tracing::warn!(backend = ?self.backend, elapsed_ms, "device key signing exceeded 500 ms");
        }
        result
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

pub fn open_or_create(data_dir: &Path) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    let (signer, _) = open_with(data_dir, None)?;
    Ok(signer)
}

/// 测试入口：指定后端偏好。返回 (签名器, 本次是否新建)。
pub fn open_with(
    data_dir: &Path,
    prefer: Option<Backend>,
) -> Result<(Arc<WinSigner>, bool), PlatformError> {
    fs::create_dir_all(data_dir).map_err(|error| io_error("create device directory", error))?;
    let metadata = read_metadata(data_dir)?;
    let force_file = prefer == Some(Backend::File)
        || std::env::var_os(KEYSTORE_ENV).is_some_and(|value| value == "file");
    let (mut signer, created) = if force_file {
        open_file(data_dir)?
    } else if let Some(metadata) = &metadata {
        match open_recorded(data_dir, metadata) {
            Ok(Some(signer)) => (signer, false),
            Ok(None) | Err(_) => {
                tracing::warn!("设备密钥不可用，已重新生成；需要重新登记和配对");
                (create_native(prefer)?, true)
            }
        }
    } else {
        (create_native(prefer)?, true)
    };

    if signer.backend == Backend::File {
        tracing::warn!(
            "device key stored in a plain file (RS_DESKTOP_KEYSTORE=file); development only"
        );
    }
    let hash = spki_hash(&signer.spki);
    let same_key = metadata.as_ref().is_some_and(|metadata| {
        metadata.backend == signer.backend && metadata.key_name == signer.key_name && !created
    });
    let hash_matches = metadata
        .as_ref()
        .is_some_and(|metadata| metadata.spki_sha256 == hash);
    if same_key && !hash_matches {
        tracing::warn!("设备密钥公钥摘要与元数据不一致，已更新元数据");
    }
    if !same_key || !hash_matches {
        let created_at_us = metadata
            .as_ref()
            .filter(|_| same_key)
            .map_or_else(rsp_core::clock::unix_micros, |metadata| {
                metadata.created_at_us
            });
        write_metadata(
            data_dir,
            &Metadata {
                backend: signer.backend,
                key_name: signer.key_name.clone(),
                created_at_us,
                spki_sha256: hash,
            },
        )?;
    }
    // This mutex has not been shared yet. Disarm rollback only after rename.
    if let Key::Cng(cng) = signer
        .key
        .get_mut()
        .map_err(|_| PlatformError::Failed("device signer mutex poisoned".into()))?
    {
        cng.delete_on_drop = false;
    }
    Ok((Arc::new(signer), created))
}

/// 打开提供程序成功即视为 TPM 可用；建密钥失败另行退回。
pub fn tpm_available() -> bool {
    open_provider(TPM_PROVIDER).is_ok()
}

/// 删除记录的 KSP 持久化密钥和两个文件。仅供测试和「重置设备」。
pub fn delete_key(data_dir: &Path) -> Result<(), PlatformError> {
    if let Some(metadata) = read_metadata(data_dir)? {
        if let Some(provider_name) = metadata.backend.provider() {
            let key_name = recorded_key_name(&metadata)?;
            let provider = open_provider(provider_name)?;
            match open_key(&provider, key_name) {
                Ok(mut key) => key.delete()?,
                Err(error) if error.is(NTE_BAD_KEYSET) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    for file in [
        METADATA_FILE,
        PRIVATE_KEY_FILE,
        METADATA_TMP,
        PRIVATE_KEY_TMP,
    ] {
        match fs::remove_file(data_dir.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove device key file", error)),
        }
    }
    Ok(())
}

fn open_recorded(data_dir: &Path, metadata: &Metadata) -> Result<Option<WinSigner>, PlatformError> {
    let Some(provider_name) = metadata.backend.provider() else {
        return load_file(data_dir);
    };
    let key_name = recorded_key_name(metadata)?;
    let opened = open_provider(provider_name).and_then(|provider| {
        let key = open_key(&provider, key_name)?;
        Ok(CngKey {
            key,
            _provider: provider,
            delete_on_drop: false,
        })
    });
    match opened {
        Ok(key) => WinSigner::from_key(metadata.backend, Some(key_name.to_owned()), Key::Cng(key))
            .map(Some),
        Err(KeyError::Cng { hresult, .. }) => {
            tracing::debug!(
                hresult = format_args!("{hresult:#010x}"),
                "recorded device key unavailable"
            );
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn recorded_key_name(metadata: &Metadata) -> Result<&str, PlatformError> {
    let key_name = metadata.key_name.as_deref().unwrap_or_default();
    let valid = key_name
        .strip_prefix(KEY_NAME_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 16
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
    if !valid {
        return Err(PlatformError::Failed(
            "device key metadata: invalid keyName".into(),
        ));
    }
    Ok(key_name)
}

fn create_native(prefer: Option<Backend>) -> Result<WinSigner, PlatformError> {
    if prefer != Some(Backend::SoftwareKsp) {
        match create_cng(Backend::Tpm) {
            Ok(signer) => return Ok(signer),
            Err(KeyError::Cng { hresult, .. }) => tracing::info!(
                hresult = format_args!("{hresult:#010x}"),
                "TPM key creation failed; falling back to software KSP"
            ),
            Err(error) => return Err(error.into()),
        }
    }
    create_cng(Backend::SoftwareKsp).map_err(Into::into)
}

fn create_cng(backend: Backend) -> Result<WinSigner, KeyError> {
    let provider_name = backend
        .provider()
        .ok_or_else(|| PlatformError::Failed("CNG creation requires a native backend".into()))?;
    let provider = open_provider(provider_name)?;
    for _ in 0..3 {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|_| {
            PlatformError::Failed("getrandom::fill: key name generation failed".into())
        })?;
        let key_name = format!("{KEY_NAME_PREFIX}{}", hex(&random));
        let wide_name = wide_string(&key_name);
        let mut key = NcryptKey(NCRYPT_KEY_HANDLE::default());
        // SAFETY: provider is live, wide_name is NUL terminated and key is an
        // output slot. Zero flags select a per-user, non-overwriting key.
        let created = unsafe {
            NCryptCreatePersistedKey(
                provider.0,
                &mut key.0,
                BCRYPT_ECDSA_P256_ALGORITHM,
                PCWSTR(wide_name.as_ptr()),
                CERT_KEY_SPEC(0),
                NCRYPT_FLAGS(0),
            )
        };
        if let Err(error) = created {
            if error.code() == NTE_EXISTS {
                continue;
            }
            return Err(KeyError::cng("NCryptCreatePersistedKey", error));
        }
        let cng = CngKey {
            key,
            _provider: provider,
            delete_on_drop: true,
        };
        if backend == Backend::SoftwareKsp {
            // SAFETY: cng owns a live key and the property buffer holds one
            // DWORD. A zero export policy denies all private key exports.
            unsafe {
                NCryptSetProperty(
                    cng.key.0.into(),
                    NCRYPT_EXPORT_POLICY_PROPERTY,
                    &0u32.to_le_bytes(),
                    NCRYPT_FLAGS(0),
                )
            }
            .map_err(|error| KeyError::cng("NCryptSetProperty", error))?;
        }
        // SAFETY: cng exclusively owns this unfinalized key. Silent mode
        // prevents a provider from asking for PINs or other interaction.
        unsafe { NCryptFinalizeKey(cng.key.0, NCRYPT_SILENT_FLAG) }
            .map_err(|error| KeyError::cng("NCryptFinalizeKey", error))?;
        let spki = export_spki(&cng.key)?;
        return Ok(WinSigner {
            backend,
            key_name: Some(key_name),
            spki,
            key: Mutex::new(Key::Cng(cng)),
            last_sign_us: AtomicU64::new(0),
        });
    }
    Err(KeyError::Cng {
        operation: "NCryptCreatePersistedKey",
        hresult: NTE_EXISTS.0 as u32,
    })
}

fn open_provider(provider_name: &str) -> Result<NcryptProvider, KeyError> {
    let wide_name = wide_string(provider_name);
    let mut provider = NcryptProvider(NCRYPT_PROV_HANDLE::default());
    // SAFETY: wide_name is NUL terminated and the provider output slot lives
    // throughout this synchronous call.
    unsafe { NCryptOpenStorageProvider(&mut provider.0, PCWSTR(wide_name.as_ptr()), 0) }
        .map_err(|error| KeyError::cng("NCryptOpenStorageProvider", error))?;
    Ok(provider)
}

fn open_key(provider: &NcryptProvider, key_name: &str) -> Result<NcryptKey, KeyError> {
    let wide_name = wide_string(key_name);
    let mut key = NcryptKey(NCRYPT_KEY_HANDLE::default());
    // SAFETY: provider is live, wide_name is NUL terminated, and key is an
    // owned output slot. No machine flag is used for this per-user key.
    unsafe {
        NCryptOpenKey(
            provider.0,
            &mut key.0,
            PCWSTR(wide_name.as_ptr()),
            CERT_KEY_SPEC(0),
            NCRYPT_SILENT_FLAG,
        )
    }
    .map_err(|error| KeyError::cng("NCryptOpenKey", error))?;
    Ok(key)
}

fn export_spki(key: &NcryptKey) -> Result<Vec<u8>, KeyError> {
    let mut size = 0;
    // SAFETY: key is live; None is the documented size query and size is a
    // writable output slot.
    unsafe {
        NCryptExportKey(
            key.0,
            None,
            BCRYPT_ECCPUBLIC_BLOB,
            None,
            None,
            &mut size,
            NCRYPT_SILENT_FLAG,
        )
    }
    .map_err(|error| KeyError::cng("NCryptExportKey", error))?;
    if size as usize != ECC_PUBLIC_BLOB_SIZE {
        return Err(
            PlatformError::Failed("NCryptExportKey: invalid P-256 blob size".into()).into(),
        );
    }
    let mut blob = [0u8; ECC_PUBLIC_BLOB_SIZE];
    // SAFETY: key is live and blob/size are writable buffers/slots with the
    // lengths supplied by the generated windows bindings.
    unsafe {
        NCryptExportKey(
            key.0,
            None,
            BCRYPT_ECCPUBLIC_BLOB,
            None,
            Some(&mut blob),
            &mut size,
            NCRYPT_SILENT_FLAG,
        )
    }
    .map_err(|error| KeyError::cng("NCryptExportKey", error))?;
    if size as usize != blob.len() {
        return Err(PlatformError::Failed("NCryptExportKey: invalid output length".into()).into());
    }
    ecc_public_blob_to_spki(&blob).map_err(Into::into)
}

fn ecc_public_blob_to_spki(blob: &[u8]) -> Result<Vec<u8>, PlatformError> {
    if blob.len() != ECC_PUBLIC_BLOB_SIZE {
        return Err(PlatformError::Failed(
            "ECC public blob: invalid length".into(),
        ));
    }
    let magic = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    let key_size = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
    if magic != BCRYPT_ECDSA_PUBLIC_P256_MAGIC || key_size != 32 {
        return Err(PlatformError::Failed(
            "ECC public blob: invalid magic or key size".into(),
        ));
    }
    let mut sec1 = [0u8; 65];
    sec1[0] = 0x04;
    sec1[1..].copy_from_slice(&blob[8..]);
    PublicKey::from_sec1_bytes(&sec1)
        .map_err(|_| PlatformError::Failed("ECC public blob: invalid P-256 point".into()))?
        .to_public_key_der()
        .map(|der| der.as_bytes().to_vec())
        .map_err(|_| PlatformError::Failed("ECC public blob: SPKI encoding failed".into()))
}

fn load_file(data_dir: &Path) -> Result<Option<WinSigner>, PlatformError> {
    let bytes = match fs::read(data_dir.join(PRIVATE_KEY_FILE)) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read device key file", error)),
    };
    let key = match SigningKey::from_pkcs8_der(&bytes) {
        Ok(key) => key,
        Err(_) => return Ok(None),
    };
    WinSigner::from_key(Backend::File, None, Key::File(key)).map(Some)
}

fn open_file(data_dir: &Path) -> Result<(WinSigner, bool), PlatformError> {
    if let Some(signer) = load_file(data_dir)? {
        return Ok((signer, false));
    }
    let key = loop {
        let mut scalar = Zeroizing::new([0u8; 32]);
        getrandom::fill(scalar.as_mut()).map_err(|_| {
            PlatformError::Failed("getrandom::fill: device key generation failed".into())
        })?;
        if let Ok(key) = SigningKey::from_slice(scalar.as_ref()) {
            break key;
        }
    };
    // SecretDocument zeroizes its allocation on drop; the serialized bytes
    // passed to the filesystem are also held in a Zeroizing buffer.
    let der = key
        .to_pkcs8_der()
        .map_err(|_| PlatformError::Failed("encode device PKCS#8 failed".into()))?;
    let bytes = Zeroizing::new(der.as_bytes().to_vec());
    write_atomic(data_dir, PRIVATE_KEY_FILE, PRIVATE_KEY_TMP, &bytes)?;
    Ok((
        WinSigner::from_key(Backend::File, None, Key::File(key))?,
        true,
    ))
}

fn read_metadata(data_dir: &Path) -> Result<Option<Metadata>, PlatformError> {
    match fs::read(data_dir.join(METADATA_FILE)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) => {
                tracing::warn!(error = %error, "device key metadata is invalid; regenerating");
                Ok(None)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("read device key metadata", error)),
    }
}

fn write_metadata(data_dir: &Path, metadata: &Metadata) -> Result<(), PlatformError> {
    let bytes = serde_json::to_vec(metadata)
        .map_err(|_| PlatformError::Failed("encode device key metadata failed".into()))?;
    write_atomic(data_dir, METADATA_FILE, METADATA_TMP, &bytes)
}

fn write_atomic(
    data_dir: &Path,
    file: &str,
    temporary: &str,
    bytes: &[u8],
) -> Result<(), PlatformError> {
    let temporary = data_dir.join(temporary);
    let target = data_dir.join(file);
    let result =
        fs::write(&temporary, bytes).and_then(|()| match fs::rename(&temporary, &target) {
            Ok(()) => Ok(()),
            // Windows std::fs::rename cannot replace an existing file. The
            // temporary file is complete before this branch, so removing the old
            // target still avoids exposing a partial JSON/PKCS#8 file.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(&target).and_then(|()| fs::rename(&temporary, &target))
            }
            Err(error) => Err(error),
        });
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|error| io_error("write device key file", error))
}

fn spki_hash(spki: &[u8]) -> String {
    hex(&Sha256::digest(spki))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[(byte >> 4) as usize] as char,
                DIGITS[(byte & 0x0f) as usize] as char,
            ]
        })
        .collect()
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn io_error(operation: &str, error: std::io::Error) -> PlatformError {
    PlatformError::Failed(format!("{operation}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_blob() -> (Vec<u8>, Vec<u8>) {
        let key = SigningKey::from_slice(&[1u8; 32]).expect("valid test scalar");
        let mut blob = Vec::new();
        blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
        blob.extend_from_slice(&32u32.to_le_bytes());
        blob.extend_from_slice(&key.verifying_key().to_sec1_point(false).as_bytes()[1..]);
        let spki = key
            .verifying_key()
            .to_public_key_der()
            .expect("SPKI")
            .as_bytes()
            .to_vec();
        (blob, spki)
    }

    #[test]
    fn ecc_blob_converts_to_spki() {
        let (blob, expected) = public_blob();
        assert_eq!(
            ecc_public_blob_to_spki(&blob).expect("valid public blob"),
            expected
        );
    }

    #[test]
    fn ecc_blob_rejects_malformed_input() {
        let (blob, _) = public_blob();
        for length in 0..blob.len() {
            assert!(matches!(
                ecc_public_blob_to_spki(&blob[..length]),
                Err(PlatformError::Failed(_))
            ));
        }
        let mut extended = blob.clone();
        extended.push(0);
        assert!(ecc_public_blob_to_spki(&extended).is_err());
        for offset in [0, 4] {
            let mut wrong = blob.clone();
            wrong[offset] ^= 1;
            assert!(ecc_public_blob_to_spki(&wrong).is_err());
        }
        let mut invalid_point = blob;
        invalid_point[8..].fill(0);
        assert!(ecc_public_blob_to_spki(&invalid_point).is_err());
    }
}
