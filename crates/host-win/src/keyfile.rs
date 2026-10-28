use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use host_core::platform::{KeyStore, PlatformError, Result};
use zeroize::Zeroizing;

/// Encrypts and decrypts the payload stored in a key file.
pub trait Protector: Send + Sync {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>>;
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>>;
}

/// Applies the platform ACL to a key directory and file.
pub trait DirSecurer: Send + Sync {
    fn secure_dir(&self, dir: &Path) -> Result<()>;
    fn secure_file(&self, file: &Path) -> Result<()>;
}

/// A no-op securer for tests and non-Windows callers.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSecurer;

impl DirSecurer for NoopSecurer {
    fn secure_dir(&self, _dir: &Path) -> Result<()> {
        Ok(())
    }

    fn secure_file(&self, _file: &Path) -> Result<()> {
        Ok(())
    }
}

/// File format marker for a DPAPI-protected key file.
pub const MAGIC: &[u8; 4] = b"RSK1";
/// Maximum protected blob size accepted by the file format.
pub const MAX_BLOB: usize = 64 * 1024;
/// Additional entropy shared by the DPAPI protect and unprotect calls.
pub const ENTROPY: &[u8] = b"ai.gxlself.remotehost.keystore.v1";
/// ACL applied to key directories.
pub const DIR_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// ACL applied to individual key files.
pub const FILE_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)";

/// A key store whose encryption and ACL operations are supplied by small
/// platform adapters.
pub struct FileKeyStore<P: Protector, S: DirSecurer> {
    dir: PathBuf,
    protector: P,
    securer: S,
    // The required temporary name is shared by writes to the same key.
    write_lock: Mutex<()>,
}

impl<P: Protector, S: DirSecurer> FileKeyStore<P, S> {
    pub fn new(dir: PathBuf, protector: P, securer: S) -> Self {
        Self {
            dir,
            protector,
            securer,
            write_lock: Mutex::new(()),
        }
    }
}

impl<P: Protector, S: DirSecurer> fmt::Debug for FileKeyStore<P, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileKeyStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl<P: Protector, S: DirSecurer> KeyStore for FileKeyStore<P, S> {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        validate_name(name)?;
        let path = self.path_for(name);
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PlatformError::Io(error)),
        };
        // Read one byte past the format limit so oversized files are rejected
        // without allocating an unbounded buffer.
        let mut bytes = Vec::new();
        file.take((MAGIC.len() + 4 + MAX_BLOB + 1) as u64)
            .read_to_end(&mut bytes)?;
        let blob = decode_file(&bytes)?;
        let plain = self.protector.unprotect(blob)?;
        Ok(Some(plain.to_vec()))
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        validate_name(name)?;
        let _write_guard = self
            .write_lock
            .lock()
            .map_err(|_| PlatformError::Platform("keystore lock poisoned".to_owned()))?;
        let path = self.path_for(name);
        let temp = self.temp_path(name);
        let result = (|| {
            fs::create_dir_all(&self.dir)?;
            self.securer.secure_dir(&self.dir)?;
            let blob = self.protector.protect(value)?;
            let bytes = encode_file(&blob)?;
            self.write_temp_and_rename(&temp, &path, &bytes)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let _write_guard = self
            .write_lock
            .lock()
            .map_err(|_| PlatformError::Platform("keystore lock poisoned".to_owned()))?;
        match fs::remove_file(self.path_for(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlatformError::Io(error)),
        }
    }
}

impl<P: Protector, S: DirSecurer> FileKeyStore<P, S> {
    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.dpapi"))
    }

    fn temp_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.dpapi.tmp"))
    }

    fn write_temp_and_rename(&self, temp: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        self.securer.secure_file(temp)?;
        fs::rename(temp, path)?;
        Ok(())
    }
}

/// Validate an application key name before using it as a file-name component.
pub fn validate_name(name: &str) -> Result<()> {
    let valid_length = (1..=64).contains(&name.len());
    let valid_chars = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !valid_length || name.starts_with('.') || !valid_chars {
        return Err(PlatformError::Platform("invalid key name".to_owned()));
    }
    Ok(())
}

/// Encode a protected blob as the on-disk key file format.
pub fn encode_file(blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() > MAX_BLOB {
        return Err(PlatformError::Platform("key too large".to_owned()));
    }
    let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + blob.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(blob.len() as u32).to_le_bytes());
    bytes.extend_from_slice(blob);
    Ok(bytes)
}

/// Decode a protected blob from the on-disk key file format.
pub fn decode_file(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() < MAGIC.len() + 4 || &bytes[..MAGIC.len()] != MAGIC {
        return Err(PlatformError::Platform("corrupt key file".to_owned()));
    }
    let length_start = MAGIC.len();
    let length_end = length_start + 4;
    let length_bytes: [u8; 4] = bytes[length_start..length_end]
        .try_into()
        .map_err(|_| PlatformError::Platform("corrupt key file".to_owned()))?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_BLOB || bytes.len() != length_end + length {
        return Err(PlatformError::Platform("corrupt key file".to_owned()));
    }
    Ok(&bytes[length_end..])
}
