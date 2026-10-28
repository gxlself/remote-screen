//! Synchronous, read-only file browsing with canonical root and deny checks.

use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rsp_core::pb::{self, file_entry::Kind};

const MAX_NAME_BYTES: usize = 1024;
const MAX_CHILD_COUNT: i32 = 10_000;

/// Canonical browsing roots and forbidden subtrees.
#[derive(Debug, Clone)]
pub struct BrowsePolicy {
    pub roots: Vec<PathBuf>,
    pub deny: Vec<PathBuf>,
    pub show_hidden: bool,
}

impl BrowsePolicy {
    /// Build a policy, using the user's home when no configured root exists.
    /// `home` should be an existing user home directory.
    pub fn new(roots: &[String], home: &Path, extra_deny: &[PathBuf]) -> Self {
        let mut canonical_roots: Vec<_> = roots
            .iter()
            .filter_map(|root| fs::canonicalize(expand_home(root, home)).ok())
            .collect();
        let invalid_count = roots.len() - canonical_roots.len();
        if invalid_count != 0 {
            tracing::warn!(invalid_count, "unavailable browsing roots ignored");
        }
        if canonical_roots.is_empty() {
            canonical_roots.push(fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf()));
        }

        let deny = ["Library", ".ssh", ".gnupg", ".Trash"]
            .iter()
            .map(|name| home.join(name))
            .chain(extra_deny.iter().cloned())
            .filter_map(|path| fs::canonicalize(path).ok())
            .collect();
        Self {
            roots: canonical_roots,
            deny,
            show_hidden: false,
        }
    }

    fn denies(&self, path: &Path) -> bool {
        self.deny.iter().any(|deny| path.starts_with(deny))
    }

    fn allows(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root)) && !self.denies(path)
    }
}

/// Expand only `~` and the `~/` prefix, leaving other paths unchanged.
pub fn expand_home(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(suffix) = path.strip_prefix("~/") {
        home.join(suffix)
    } else {
        PathBuf::from(path)
    }
}

/// File-browsing failures without user paths in their messages.
#[derive(Debug, thiserror::Error)]
pub enum BrowseError {
    #[error("path not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("path outside browsing roots or in a forbidden subtree")]
    OutsideRoot,
    #[error("path is not a directory")]
    NotADir,
    #[error("path is not a regular file")]
    NotAFile,
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl BrowseError {
    pub fn to_pb(&self) -> pb::FileError {
        match self {
            Self::NotFound => pb::FileError::NotFound,
            Self::PermissionDenied => pb::FileError::PermissionDenied,
            Self::OutsideRoot => pb::FileError::OutsideRoot,
            Self::NotADir => pb::FileError::NotADir,
            Self::NotAFile => pb::FileError::NotAFile,
            Self::Io(error) => match error.kind() {
                io::ErrorKind::NotFound => pb::FileError::NotFound,
                io::ErrorKind::PermissionDenied => pb::FileError::PermissionDenied,
                io::ErrorKind::NotADirectory => pb::FileError::NotADir,
                io::ErrorKind::IsADirectory => pb::FileError::NotAFile,
                io::ErrorKind::StorageFull => pb::FileError::NoSpace,
                _ => pb::FileError::Io,
            },
        }
    }
}

fn map_io(error: io::Error) -> BrowseError {
    match error.kind() {
        io::ErrorKind::NotFound => BrowseError::NotFound,
        io::ErrorKind::PermissionDenied => BrowseError::PermissionDenied,
        io::ErrorKind::NotADirectory => BrowseError::NotADir,
        io::ErrorKind::IsADirectory => BrowseError::NotAFile,
        _ => BrowseError::Io(error),
    }
}

/// A canonical directory and its navigable parent (`None` at any root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDir {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
}

fn resolve(p: &BrowsePolicy, path: &Path) -> Result<PathBuf, BrowseError> {
    if path.as_os_str().is_empty() {
        return Err(BrowseError::NotFound);
    }
    if !path.is_absolute() {
        return Err(BrowseError::OutsideRoot);
    }
    let canonical = fs::canonicalize(path).map_err(map_io)?;
    if !p.allows(&canonical) {
        return Err(BrowseError::OutsideRoot);
    }
    Ok(canonical)
}

pub fn resolve_dir(p: &BrowsePolicy, path: &str) -> Result<ResolvedDir, BrowseError> {
    let path = if path.is_empty() {
        p.roots.first().ok_or(BrowseError::NotFound)?.as_path()
    } else {
        Path::new(path)
    };
    let path = resolve(p, path)?;
    if !fs::metadata(&path).map_err(map_io)?.is_dir() {
        return Err(BrowseError::NotADir);
    }
    let parent = if p.roots.contains(&path) {
        None
    } else {
        path.parent().map(Path::to_path_buf)
    };
    Ok(ResolvedDir { path, parent })
}

pub fn resolve_file(p: &BrowsePolicy, path: &str) -> Result<PathBuf, BrowseError> {
    let path = resolve(p, Path::new(path))?;
    if !fs::metadata(&path).map_err(map_io)?.is_file() {
        return Err(BrowseError::NotAFile);
    }
    Ok(path)
}

pub fn resolve_dest_dir(p: &BrowsePolicy, path: &str) -> Result<PathBuf, BrowseError> {
    // Unlike directory browsing, an empty destination does not select a root.
    if path.is_empty() {
        return Err(BrowseError::NotFound);
    }
    let dir = resolve_dir(p, path)?;
    if fs::metadata(&dir.path)
        .map_err(map_io)?
        .permissions()
        .readonly()
    {
        return Err(BrowseError::PermissionDenied);
    }
    Ok(dir.path)
}

/// A shared time budget for counting children after the listing is sorted.
#[derive(Debug, Clone)]
pub struct ListOptions {
    pub count_budget: Duration,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            count_budget: Duration::from_millis(200),
        }
    }
}

/// List a directory previously returned by [`resolve_dir`].
/// Child symlinks are checked against the policy before inspecting targets.
pub fn list_dir(
    p: &BrowsePolicy,
    dir: &Path,
    opts: &ListOptions,
) -> Result<Vec<pb::FileEntry>, BrowseError> {
    list_dir_with_canonicalizer(p, dir, opts, |path| fs::canonicalize(path))
}

fn list_dir_with_canonicalizer(
    p: &BrowsePolicy,
    dir: &Path,
    opts: &ListOptions,
    mut canonicalize: impl FnMut(&Path) -> io::Result<PathBuf>,
) -> Result<Vec<pb::FileEntry>, BrowseError> {
    let mut entries = Vec::new();
    let mut invalid_utf8_count = 0_usize;
    for entry in fs::read_dir(dir).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                invalid_utf8_count += 1;
                continue;
            }
        };
        if name.len() > MAX_NAME_BYTES || (!p.show_hidden && name.starts_with('.')) {
            continue;
        }

        let mut path = entry.path();
        let mut metadata = fs::symlink_metadata(&path).map_err(map_io)?;
        let needs_canonicalization = metadata.is_symlink();
        #[cfg(windows)]
        let needs_canonicalization = {
            use std::os::windows::fs::MetadataExt;
            // FILE_ATTRIBUTE_REPARSE_POINT also covers junctions.
            needs_canonicalization || metadata.file_attributes() & 0x400 != 0
        };
        // The parent is already canonical; only links need another path walk.
        let canonical = if needs_canonicalization {
            canonicalize(&path).ok()
        } else {
            None
        };
        if p.denies(&path) || canonical.as_ref().is_some_and(|path| p.denies(path)) {
            continue;
        }
        if metadata.is_symlink() {
            if let Some(target) = canonical.filter(|target| p.allows(target)) {
                if let Ok(target_metadata) = fs::metadata(&target) {
                    metadata = target_metadata;
                    path = target;
                }
            }
        }
        let kind = metadata_kind(&metadata);
        let file_entry = pb::FileEntry {
            name,
            kind: kind.into(),
            size: if kind == Kind::File {
                metadata.len()
            } else {
                0
            },
            mtime_us: mtime_us(&metadata),
            child_count: -1,
        };
        entries.push((file_entry, path));
    }
    if invalid_utf8_count != 0 {
        tracing::debug!(invalid_utf8_count, "non-UTF-8 browsing entries skipped");
    }

    entries.sort_by_cached_key(|(entry, _)| {
        (
            entry.kind != Kind::Dir as i32,
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    let started = Instant::now();
    for (entry, path) in &mut entries {
        if started.elapsed() >= opts.count_budget {
            break;
        }
        if entry.kind == Kind::Dir as i32 {
            entry.child_count = count_children(path, started, opts.count_budget);
        }
    }
    Ok(entries.into_iter().map(|(entry, _)| entry).collect())
}

fn metadata_kind(metadata: &Metadata) -> Kind {
    if metadata.is_file() {
        Kind::File
    } else if metadata.is_dir() {
        Kind::Dir
    } else if metadata.is_symlink() {
        Kind::Symlink
    } else {
        Kind::Other
    }
}

fn mtime_us(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_micros()).ok())
        .unwrap_or(0)
}

fn count_children(path: &Path, started: Instant, budget: Duration) -> i32 {
    let Ok(mut children) = fs::read_dir(path) else {
        return -1;
    };
    for count in 0..MAX_CHILD_COUNT {
        if started.elapsed() >= budget {
            return -1;
        }
        let child = children.next();
        if started.elapsed() >= budget {
            return -1;
        }
        match child {
            Some(Ok(_)) => {}
            Some(Err(_)) => return -1,
            None => return count,
        }
    }
    MAX_CHILD_COUNT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_children_do_not_repeat_canonical_path_walks() {
        let root = std::env::temp_dir().join(format!(
            "rs-browse-canonical-{}-{}",
            std::process::id(),
            rsp_core::clock::unix_micros()
        ));
        fs::create_dir_all(root.join("directory")).expect("test directory is created");
        fs::write(root.join("file"), b"contents").expect("test file is created");
        let root = fs::canonicalize(root).expect("test root is canonical");
        let policy = BrowsePolicy {
            roots: vec![root.clone()],
            deny: Vec::new(),
            show_hidden: false,
        };
        let mut calls = 0;
        let result = list_dir_with_canonicalizer(&policy, &root, &ListOptions::default(), |_| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::TimedOut))
        });
        fs::remove_dir_all(root).expect("test directory is removed");
        let entries = result.expect("regular children list without canonicalizing them again");
        assert_eq!(calls, 0, "ordinary entries must not walk every ancestor");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "directory");
        assert_eq!(entries[0].kind(), Kind::Dir);
        assert_eq!(entries[1].name, "file");
        assert_eq!(entries[1].kind(), Kind::File);
        assert_eq!(entries[1].size, 8);
    }
}
