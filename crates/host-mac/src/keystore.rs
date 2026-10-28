//! macOS Keychain storage with an opt-in file backend for development builds.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::platform::{KeyStore, PlatformError, Result};

const MAX_KEYCHAIN_VALUE: usize = 16 << 20;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn rs_keychain_load(
        account: *const std::ffi::c_char,
        out: *mut u8,
        capacity: usize,
        length: *mut usize,
    ) -> i32;
    fn rs_keychain_store(account: *const std::ffi::c_char, data: *const u8, length: usize) -> i32;
    fn rs_keychain_delete(account: *const std::ffi::c_char) -> i32;
}

/// The backend selected for a `MacPlatform` instance.
pub enum MacKeyStore {
    Keychain,
    File(FileKeyStore),
}

impl MacKeyStore {
    pub fn from_environment() -> Self {
        if std::env::var("RS_KEYSTORE").ok().as_deref() == Some("file") {
            Self::File(FileKeyStore::from_home())
        } else {
            Self::Keychain
        }
    }
}

impl KeyStore for MacKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Keychain => keychain_load(name),
            Self::File(store) => store.load(name),
        }
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        match self {
            Self::Keychain => keychain_store(name, value),
            Self::File(store) => store.store(name, value),
        }
    }

    fn delete(&self, name: &str) -> Result<()> {
        match self {
            Self::Keychain => keychain_delete(name),
            Self::File(store) => store.delete(name),
        }
    }
}

/// File backend used by development probes and for builds whose signing
/// identity changes frequently. Files are named after the identity key name.
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    pub fn from_home() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            dir: home
                .join("Library")
                .join("Application Support")
                .join("Remote Host")
                .join("keys"),
        }
    }

    /// Construct a file store rooted at an explicit directory. This is useful
    /// for isolated tests and does not alter the process-wide environment.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(name))
    }

    fn prepare_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(PlatformError::Io)?;
        tighten_dir(&self.dir)
    }
}

impl KeyStore for FileKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_for(name)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PlatformError::Io(error)),
        };
        tighten_file(&path)?;
        Ok(Some(bytes))
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        let path = self.path_for(name)?;
        self.prepare_dir()?;
        let temp = self.temp_path(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(PlatformError::Io)?;
        if let Err(error) = file.write_all(value).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temp);
            return Err(PlatformError::Io(error));
        }
        drop(file);
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(PlatformError::Io(error));
        }
        tighten_file(&path)?;
        sync_dir(&self.dir)
    }

    fn delete(&self, name: &str) -> Result<()> {
        let path = self.path_for(name)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlatformError::Io(error)),
        }
    }
}

impl FileKeyStore {
    fn temp_path(&self, name: &str) -> PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64);
        self.dir.join(format!(
            ".{name}.tmp.{}.{}",
            std::process::id(),
            counter ^ timestamp
        ))
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(PlatformError::Platform(
            "keystore name must be a single path component".into(),
        ));
    }
    Ok(())
}

fn keychain_account(name: &str) -> Result<std::ffi::CString> {
    validate_name(name)?;
    std::ffi::CString::new(name)
        .map_err(|_| PlatformError::Platform("keystore name contains an embedded NUL".into()))
}

fn keychain_load(name: &str) -> Result<Option<Vec<u8>>> {
    let account = keychain_account(name)?;
    let mut length = 0usize;
    // SAFETY: null output with zero capacity is a size query; account is a
    // valid NUL-terminated string for the duration of the call.
    let status =
        unsafe { rs_keychain_load(account.as_ptr(), std::ptr::null_mut(), 0, &mut length) };
    if status == 0 {
        return Ok(None);
    }
    if status != 1 && status != 2 {
        return Err(PlatformError::Platform(format!(
            "Keychain load failed ({status})"
        )));
    }
    if length > MAX_KEYCHAIN_VALUE {
        return Err(PlatformError::Platform(
            "Keychain value exceeds the supported size".into(),
        ));
    }
    let mut value = vec![0u8; length];
    // SAFETY: the buffer has the size reported by the preceding query.
    let status = unsafe {
        rs_keychain_load(
            account.as_ptr(),
            value.as_mut_ptr(),
            value.len(),
            &mut length,
        )
    };
    if status != 1 || length > value.len() {
        return Err(PlatformError::Platform(format!(
            "Keychain load failed ({status})"
        )));
    }
    value.truncate(length);
    Ok(Some(value))
}

fn keychain_store(name: &str, value: &[u8]) -> Result<()> {
    let account = keychain_account(name)?;
    // SAFETY: the shim copies the value before returning.
    let status = unsafe { rs_keychain_store(account.as_ptr(), value.as_ptr(), value.len()) };
    if status == 0 {
        Ok(())
    } else {
        Err(PlatformError::Platform(format!(
            "Keychain store failed ({status})"
        )))
    }
}

fn keychain_delete(name: &str) -> Result<()> {
    let account = keychain_account(name)?;
    // SAFETY: account is a valid NUL-terminated string for the duration call.
    let status = unsafe { rs_keychain_delete(account.as_ptr()) };
    if status == 0 {
        Ok(())
    } else {
        Err(PlatformError::Platform(format!(
            "Keychain delete failed ({status})"
        )))
    }
}

#[cfg(unix)]
fn tighten_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(PlatformError::Io)?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(PlatformError::Io)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn tighten_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(PlatformError::Io)?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(PlatformError::Io)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(PlatformError::Io)?;
    }
    Ok(())
}
