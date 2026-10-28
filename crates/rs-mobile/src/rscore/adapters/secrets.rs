//! Credential storage and a non-secret local index for built-in adapters.

use std::{
    collections::HashMap,
    ffi::{c_void, CString},
    io::Cursor,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
};

use serde::{Deserialize, Deserializer, Serialize};
use tracing::{error, warn};
use zeroize::{Zeroize, Zeroizing};

use super::{
    driver::Credentials,
    dto::SavedCredentialInfo,
    ffi_secrets::{RsSecretDelCb, RsSecretGetCb, RsSecretPutCb},
};
use crate::rscore::runtime::Core;

pub const MAX_KEY_BYTES: usize = 300;
pub const MAX_VALUE_BYTES: usize = 4096;
/// Non-secret kv entries: `adapter.saved.<store_key>` -> {"user":…,"domain":…}.
/// Absent fields are omitted.
pub const SAVED_INDEX_PREFIX: &str = "adapter.saved.";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("no secret store registered")]
    NotRegistered,
    #[error("key too long or contains NUL")]
    BadKey,
    #[error("value too large")]
    TooLarge,
    #[error("native store failed: {0}")]
    Native(i32),
    #[error("stored value is malformed")]
    Malformed,
}

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` means the key does not exist.
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    fn put(&self, key: &str, value: &[u8]) -> Result<(), SecretError>;
    /// Deleting a missing key also succeeds.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// In-memory storage for tests and desktop use. Counts calls passing validation.
#[derive(Default)]
pub struct MemSecretStore {
    values: Mutex<HashMap<String, Vec<u8>>>,
    gets: AtomicUsize,
    puts: AtomicUsize,
    deletes: AtomicUsize,
}

impl MemSecretStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Return a copy for inspection; the caller owns this unprotected buffer.
    pub fn raw(&self, key: &str) -> Option<Vec<u8>> {
        validate_key(key).ok()?;
        self.values
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.values
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get_count(&self) -> usize {
        self.gets.load(Ordering::Relaxed)
    }

    pub fn put_count(&self) -> usize {
        self.puts.load(Ordering::Relaxed)
    }

    pub fn delete_count(&self) -> usize {
        self.deletes.load(Ordering::Relaxed)
    }
}

impl SecretStore for MemSecretStore {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        validate_key(key)?;
        self.gets.fetch_add(1, Ordering::Relaxed);
        Ok(self.raw(key).map(Zeroizing::new))
    }

    fn put(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
        validate_key(key)?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(SecretError::TooLarge);
        }
        self.puts.fetch_add(1, Ordering::Relaxed);
        let previous = self
            .values
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(key.to_owned(), value.to_vec());
        if let Some(mut previous) = previous {
            previous.zeroize();
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        validate_key(key)?;
        self.deletes.fetch_add(1, Ordering::Relaxed);
        let previous = self
            .values
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(key);
        if let Some(mut previous) = previous {
            previous.zeroize();
        }
        Ok(())
    }
}

impl Drop for MemSecretStore {
    fn drop(&mut self) {
        for value in self
            .values
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .values_mut()
        {
            value.zeroize();
        }
    }
}

static REGISTERED: RwLock<Option<Arc<dyn SecretStore>>> = RwLock::new(None);

pub fn registered() -> Option<Arc<dyn SecretStore>> {
    REGISTERED
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Replace the process-wide store. `None` clears it for tests only.
pub fn set_registered(store: Option<Arc<dyn SecretStore>>) {
    let previous = {
        let mut current = REGISTERED
            .write()
            .unwrap_or_else(|error| error.into_inner());
        std::mem::replace(&mut *current, store)
    };
    // Drop outside the lock: a store's destructor may itself use the registry.
    drop(previous);
}

pub fn can_save() -> bool {
    registered().is_some()
}

fn validate_key(key: &str) -> Result<(), SecretError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES || key.contains('\0') {
        Err(SecretError::BadKey)
    } else {
        Ok(())
    }
}

fn call_store<T>(
    key: &str,
    operation: &'static str,
    f: impl FnOnce() -> Result<T, SecretError>,
) -> Result<T, SecretError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        error!(key, operation, "secret store panicked");
        Err(SecretError::Native(-1))
    })
}

#[derive(Serialize)]
struct EncodedCredentials<'a> {
    v: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<&'a str>,
    password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<&'a str>,
}

#[derive(Deserialize)]
struct DecodedCredentials {
    v: u8,
    user: Option<String>,
    #[serde(deserialize_with = "deserialize_password")]
    password: Zeroizing<String>,
    domain: Option<String>,
}

fn deserialize_password<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[derive(Serialize, Deserialize)]
struct SavedIndex {
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
}

/// Save after authentication succeeds, then best-effort update the local index.
pub fn save(core: &Core, key: &str, creds: &Credentials) -> Result<(), SecretError> {
    validate_key(key)?;
    let store = registered().ok_or(SecretError::NotRegistered)?;
    // Serialize directly into a bounded, zeroizing buffer. This also avoids
    // reallocations leaving copies of a password behind when JSON needs escapes.
    let mut value = Zeroizing::new(vec![0; MAX_VALUE_BYTES]);
    let mut writer = Cursor::new(value.as_mut_slice());
    serde_json::to_writer(
        &mut writer,
        &EncodedCredentials {
            v: 1,
            user: creds.user.as_deref(),
            password: creds.password.as_str(),
            domain: creds.domain.as_deref(),
        },
    )
    // Serializing these strings can only fail when the bounded writer is full.
    .map_err(|_| SecretError::TooLarge)?;
    let len = writer.position() as usize;
    value.truncate(len);
    call_store(key, "put", || store.put(key, &value))?;

    let index = SavedIndex {
        user: creds.user.clone(),
        domain: creds.domain.clone(),
    };
    match serde_json::to_string(&index) {
        Ok(index) => {
            if core
                .store()
                .kv_set(&format!("{SAVED_INDEX_PREFIX}{key}"), &index)
                .is_err()
            {
                warn!(key, "failed to write saved credential index");
            }
        }
        Err(_) => warn!(key, "failed to encode saved credential index"),
    }
    Ok(())
}

/// Read credentials without deleting malformed or unsupported stored values.
pub fn load(key: &str) -> Result<Option<Credentials>, SecretError> {
    validate_key(key)?;
    let store = registered().ok_or(SecretError::NotRegistered)?;
    let Some(value) = call_store(key, "get", || store.get(key))? else {
        return Ok(None);
    };
    if value.len() > MAX_VALUE_BYTES {
        return Err(SecretError::TooLarge);
    }
    match serde_json::from_slice::<DecodedCredentials>(&value) {
        Ok(value) if value.v == 1 => Ok(Some(Credentials {
            user: value.user,
            password: value.password,
            domain: value.domain,
        })),
        _ => {
            warn!(key, "stored credentials are malformed");
            Err(SecretError::Malformed)
        }
    }
}

/// Delete the native value, then best-effort remove its local index.
/// Without a registered store, only the index is removed.
pub fn forget(core: &Core, key: &str) -> Result<(), SecretError> {
    validate_key(key)?;
    if let Some(store) = registered() {
        call_store(key, "delete", || store.delete(key))?;
    }
    if core
        .store()
        .kv_delete(&format!("{SAVED_INDEX_PREFIX}{key}"))
        .is_err()
    {
        warn!(key, "failed to remove saved credential index");
    }
    Ok(())
}

/// Inspect only the non-secret index; never unlock or read the native store.
/// Invalid keys and unreadable indices are reported as not saved.
pub fn saved_info(core: &Core, key: &str) -> SavedCredentialInfo {
    if validate_key(key).is_err() || !can_save() {
        return SavedCredentialInfo::default();
    }
    let index = match core.store().kv_get(&format!("{SAVED_INDEX_PREFIX}{key}")) {
        Ok(Some(index)) => index,
        Ok(None) => return SavedCredentialInfo::default(),
        Err(_) => {
            warn!(key, "failed to read saved credential index");
            return SavedCredentialInfo::default();
        }
    };
    match serde_json::from_str::<SavedIndex>(&index) {
        Ok(index) => SavedCredentialInfo {
            saved: true,
            user: index.user,
            domain: index.domain,
        },
        Err(_) => {
            warn!(key, "saved credential index is malformed");
            SavedCredentialInfo::default()
        }
    }
}

pub(crate) struct CallbackStore {
    pub(crate) ctx: *mut c_void,
    pub(crate) get: RsSecretGetCb,
    pub(crate) put: RsSecretPutCb,
    pub(crate) del: RsSecretDelCb,
}

// SAFETY: C4.2 registration requires ctx/callbacks to stay valid for the process
// lifetime, or until replacement and all old calls return, and to support
// concurrent calls on arbitrary threads.
unsafe impl Send for CallbackStore {}
// SAFETY: The same C4.2 lifetime and concurrent-call requirements apply; this
// adapter only shares immutable callback pointers and the native-owned ctx.
unsafe impl Sync for CallbackStore {}

impl SecretStore for CallbackStore {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        validate_key(key)?;
        let c_key = CString::new(key).map_err(|_| SecretError::BadKey)?;
        call_store(key, "get", || {
            let mut value = Zeroizing::new(vec![0; MAX_VALUE_BYTES]);
            let mut len = value.len();
            let rc = (self.get)(self.ctx, c_key.as_ptr(), value.as_mut_ptr(), &mut len);
            match rc {
                0 if len <= MAX_VALUE_BYTES => {
                    value.truncate(len);
                    Ok(Some(value))
                }
                1 => Ok(None),
                _ => Err(SecretError::Native(rc)),
            }
        })
    }

    fn put(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
        validate_key(key)?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(SecretError::TooLarge);
        }
        let c_key = CString::new(key).map_err(|_| SecretError::BadKey)?;
        call_store(key, "put", || {
            let rc = (self.put)(self.ctx, c_key.as_ptr(), value.as_ptr(), value.len());
            if rc == 0 {
                Ok(())
            } else {
                Err(SecretError::Native(rc))
            }
        })
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        validate_key(key)?;
        let c_key = CString::new(key).map_err(|_| SecretError::BadKey)?;
        call_store(key, "delete", || {
            let rc = (self.del)(self.ctx, c_key.as_ptr());
            if rc == 0 {
                Ok(())
            } else {
                Err(SecretError::Native(rc))
            }
        })
    }
}
