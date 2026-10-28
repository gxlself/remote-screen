//! Resolution of the directories belonging to the user currently using the host.
//!
//! On Windows the service runs as SYSTEM, so a platform resolver must inspect
//! the console user and return that user's home and Downloads directories; the
//! Windows implementation is supplied in CI.  The resolver's `current`
//! method is called from session tasks and must use cached state rather than
//! performing a slow operation or waiting for a network or UI operation.

use std::path::{Path, PathBuf};

/// The account that owns the directories returned by a [`UserDirsSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirsOwner {
    /// The current console user (or the agent's user on Mac/Linux).
    User,
    /// No user is available; Downloads is a restricted service fallback.
    NoUser,
}

/// Directories and ownership information for the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirs {
    /// The directory used for received files.
    pub downloads: PathBuf,
    /// The browsing root and `~` expansion directory. `None` disables browsing.
    pub home: Option<PathBuf>,
    /// Who owns the resolved directories.
    pub owner: DirsOwner,
}

/// Resolves the current user's directories for file operations.
pub trait UserDirsSource: Send + Sync + 'static {
    /// Return a cached snapshot quickly; this must not perform slow I/O.
    fn current(&self) -> UserDirs;

    /// Check whether the service may write to a directory on behalf of the user.
    /// Agent processes already run as the user, so the default is always true.
    fn user_can_write(&self, dir: &Path) -> bool {
        let _ = dir;
        true
    }
}

/// Default directory resolver preserving the agent's historical behavior.
pub struct EnvDirs {
    downloads: PathBuf,
}

impl EnvDirs {
    pub fn new(downloads: PathBuf) -> Self {
        Self { downloads }
    }
}

impl UserDirsSource for EnvDirs {
    fn current(&self) -> UserDirs {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        UserDirs {
            downloads: self.downloads.clone(),
            home,
            owner: DirsOwner::User,
        }
    }
}
