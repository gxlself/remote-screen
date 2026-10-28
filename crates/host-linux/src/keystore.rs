//! Atomic file-backed host key storage for Linux.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::platform::{KeyStore, PlatformError, Result};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A directory-backed implementation of the host key store.
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    /// Construct a store rooted at an explicit directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Construct the key directory below a host state directory.
    pub fn in_state_dir(state_dir: &Path) -> Self {
        Self::new(state_dir.join("keys"))
    }

    /// Return the directory containing the key files.
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
        sync_dir(&self.dir);
        Ok(())
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

/// Flush the containing directory where the platform supports it.  Directory
/// fsync is best effort because some filesystems reject opening directories.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    {
        if let Ok(file) = File::open(path) {
            let _ = file.sync_all();
        }
    }
}
