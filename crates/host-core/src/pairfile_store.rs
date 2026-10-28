//! 配对文件存储 `pairfiles.json`。见 04「其他配对方式」、06「配对」。

use std::collections::HashSet;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use data_encoding::BASE64URL_NOPAD;
use rsp_core::trust::Permission;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;
#[cfg(unix)]
use tracing::warn;
use zeroize::Zeroizing;

use crate::pairing::{OsRandom, RandomSource};

const VERSION: u32 = 1;

pub const PAIRFILES_FILE: &str = "pairfiles.json";
pub const MAX_ACTIVE_FILES: usize = 50;
pub const PAIRFILE_LIFETIME_US: u64 = 24 * 60 * 60 * 1_000_000;
pub const PAIRFILE_MAX_FAILURES: u32 = 5;
pub const TERMINAL_RETENTION_US: u64 = 7 * 24 * 60 * 60 * 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairFileState {
    Active,
    Used,
    Failed,
    Revoked,
    Expired,
}

/// Newly created pairing file material. The secret is returned only here and
/// is dropped immediately after the caller writes it into the exported file.
#[derive(Clone, PartialEq, Eq)]
pub struct NewPairFile {
    pub file_id: [u8; 16],
    pub secret: Zeroizing<[u8; 32]>,
    pub permission: Permission,
    pub created_at_us: u64,
    pub expires_at_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairFileSummary {
    pub file_id: String,
    pub label: Option<String>,
    pub permission: Permission,
    pub created_at_us: u64,
    pub expires_at_us: u64,
    pub state: PairFileState,
    pub used_by_device_id: Option<String>,
    pub used_at_us: Option<u64>,
}

/// The secret and permission needed when a pairing session starts.
#[derive(Clone, PartialEq, Eq)]
pub struct PairFileGrant {
    pub secret: Zeroizing<[u8; 32]>,
    pub permission: Permission,
}

impl fmt::Debug for NewPairFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewPairFile")
            .field("file_id", &self.file_id)
            .field("secret", &"<redacted>")
            .field("permission", &self.permission)
            .field("created_at_us", &self.created_at_us)
            .field("expires_at_us", &self.expires_at_us)
            .finish()
    }
}

impl fmt::Debug for PairFileGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairFileGrant")
            .field("secret", &"<redacted>")
            .field("permission", &self.permission)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum PairFileError {
    #[error("too many active pairing files")]
    TooMany,
    #[error("pairing file not found")]
    NotFound,
    #[error("pairing file is not usable")]
    NotUsable,
    #[error("invalid label")]
    InvalidLabel,
    #[error("pairing file store is corrupt")]
    Corrupt,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("random number generation failed")]
    Random,
}

pub struct PairFileStore {
    path: PathBuf,
    doc: Mutex<Doc>,
    rng: Arc<dyn RandomSource>,
}

impl fmt::Debug for PairFileStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairFileStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default)]
struct Doc {
    files: Vec<Record>,
}

#[derive(Clone)]
struct Record {
    file_id: [u8; 16],
    secret: Zeroizing<[u8; 32]>,
    label: Option<String>,
    permission: Permission,
    created_at_us: u64,
    expires_at_us: u64,
    state: PairFileState,
    failures: u32,
    used_by: Option<String>,
    used_at_us: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct PairFilesJson {
    v: u32,
    files: Vec<PairFileJson>,
}

#[derive(Serialize, Deserialize)]
struct PairFileJson {
    file_id: String,
    secret: String,
    label: Option<String>,
    permission: String,
    created_at_us: u64,
    expires_at_us: u64,
    state: PairFileState,
    failures: u32,
    used_by: Option<String>,
    used_at_us: Option<u64>,
}

impl PairFileStore {
    pub fn open(state_dir: &Path) -> Result<Self, PairFileError> {
        fs::create_dir_all(state_dir)?;
        tighten_dir(state_dir)?;
        let path = state_dir.join(PAIRFILES_FILE);
        let doc = if path.exists() {
            tighten_file(&path)?;
            read_doc(&path)?
        } else {
            Doc::default()
        };
        Ok(Self {
            path,
            doc: Mutex::new(doc),
            rng: Arc::new(OsRandom),
        })
    }

    pub fn with_rng(self, rng: Arc<dyn RandomSource>) -> Self {
        Self { rng, ..self }
    }

    pub fn create(
        &self,
        now_us: u64,
        permission: Permission,
        label: Option<&str>,
    ) -> Result<NewPairFile, PairFileError> {
        let label = normalize_label(label)?;
        let mut guard = self.lock();
        let mut next = guard.clone();
        normalize_expired(&mut next, now_us);
        next.files
            .retain(|record| !terminal_expired(record, now_us));
        if next
            .files
            .iter()
            .filter(|record| record.state == PairFileState::Active)
            .count()
            >= MAX_ACTIVE_FILES
        {
            return Err(PairFileError::TooMany);
        }

        let mut file_id = [0u8; 16];
        self.rng
            .fill(&mut file_id)
            .map_err(|_| PairFileError::Random)?;
        let mut secret = Zeroizing::new([0u8; 32]);
        self.rng
            .fill(&mut secret[..])
            .map_err(|_| PairFileError::Random)?;
        let created_at_us = now_us;
        let expires_at_us = now_us.saturating_add(PAIRFILE_LIFETIME_US);
        let record = Record {
            file_id,
            secret: Zeroizing::new(*secret),
            label: label.clone(),
            permission,
            created_at_us,
            expires_at_us,
            state: PairFileState::Active,
            failures: 0,
            used_by: None,
            used_at_us: None,
        };
        next.files.push(record);
        self.persist(&next)?;
        *guard = next;
        Ok(NewPairFile {
            file_id,
            secret,
            permission,
            created_at_us,
            expires_at_us,
        })
    }

    pub fn list(&self, now_us: u64) -> Vec<PairFileSummary> {
        let guard = self.lock();
        let mut files = guard
            .files
            .iter()
            .map(|record| PairFileSummary {
                file_id: encode_file_id(&record.file_id),
                label: record.label.clone(),
                permission: record.permission,
                created_at_us: record.created_at_us,
                expires_at_us: record.expires_at_us,
                state: effective_state(record, now_us),
                used_by_device_id: record.used_by.clone(),
                used_at_us: record.used_at_us,
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| {
            right
                .created_at_us
                .cmp(&left.created_at_us)
                .then_with(|| left.file_id.cmp(&right.file_id))
        });
        files
    }

    pub fn revoke(&self, file_id: &str) -> Result<(), PairFileError> {
        let file_id = decode_file_id(file_id).ok_or(PairFileError::NotFound)?;
        let mut guard = self.lock();
        let Some(index) = guard
            .files
            .iter()
            .position(|record| record.file_id == file_id)
        else {
            return Err(PairFileError::NotFound);
        };
        if guard.files[index].state != PairFileState::Active {
            return Ok(());
        }
        let mut next = guard.clone();
        next.files[index].state = PairFileState::Revoked;
        self.persist(&next)?;
        *guard = next;
        Ok(())
    }

    pub fn begin(&self, now_us: u64, file_id: &[u8]) -> Result<PairFileGrant, PairFileError> {
        let file_id = <[u8; 16]>::try_from(file_id).map_err(|_| PairFileError::NotUsable)?;
        let guard = self.lock();
        let record = guard
            .files
            .iter()
            .find(|record| record.file_id == file_id)
            .ok_or(PairFileError::NotUsable)?;
        if effective_state(record, now_us) != PairFileState::Active {
            return Err(PairFileError::NotUsable);
        }
        Ok(PairFileGrant {
            secret: record.secret.clone(),
            permission: record.permission,
        })
    }

    pub fn fail(&self, now_us: u64, file_id: &[u8]) -> u32 {
        let Ok(file_id) = <[u8; 16]>::try_from(file_id) else {
            return 0;
        };
        let mut guard = self.lock();
        let mut next = guard.clone();
        normalize_expired(&mut next, now_us);
        let Some(record) = next
            .files
            .iter_mut()
            .find(|record| record.file_id == file_id)
        else {
            return 0;
        };
        if record.state != PairFileState::Active {
            return 0;
        }
        record.failures = record.failures.saturating_add(1);
        let remaining = PAIRFILE_MAX_FAILURES.saturating_sub(record.failures);
        if remaining == 0 {
            record.state = PairFileState::Failed;
        }
        if let Err(persist_error) = self.persist(&next) {
            error!(error = %persist_error, "failed to persist pairing file failure count");
        }
        *guard = next;
        remaining
    }

    pub fn consume(
        &self,
        now_us: u64,
        file_id: &[u8],
        device_id: &str,
    ) -> Result<(), PairFileError> {
        let file_id = <[u8; 16]>::try_from(file_id).map_err(|_| PairFileError::NotUsable)?;
        let mut guard = self.lock();
        let mut next = guard.clone();
        normalize_expired(&mut next, now_us);
        let Some(record) = next
            .files
            .iter_mut()
            .find(|record| record.file_id == file_id)
        else {
            return Err(PairFileError::NotUsable);
        };
        if record.state != PairFileState::Active {
            return Err(PairFileError::NotUsable);
        }
        record.state = PairFileState::Used;
        record.used_by = Some(device_id.to_owned());
        record.used_at_us = Some(now_us);
        self.persist(&next)?;
        *guard = next;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Doc> {
        self.doc.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, doc: &Doc) -> Result<(), PairFileError> {
        let files = doc
            .files
            .iter()
            .map(|record| PairFileJson {
                file_id: encode_file_id(&record.file_id),
                secret: BASE64URL_NOPAD.encode(&record.secret[..]),
                label: record.label.clone(),
                permission: permission_name(record.permission).to_owned(),
                created_at_us: record.created_at_us,
                expires_at_us: record.expires_at_us,
                state: record.state,
                failures: record.failures,
                used_by: record.used_by.clone(),
                used_at_us: record.used_at_us,
            })
            .collect();
        write_json(&self.path, &PairFilesJson { v: VERSION, files })
    }
}

fn read_doc(path: &Path) -> Result<Doc, PairFileError> {
    let bytes = fs::read(path)?;
    let file: PairFilesJson = serde_json::from_slice(&bytes).map_err(|_| PairFileError::Corrupt)?;
    if file.v != VERSION {
        return Err(PairFileError::Corrupt);
    }
    let mut files = Vec::with_capacity(file.files.len());
    let mut file_ids = HashSet::with_capacity(file.files.len());
    for entry in file.files {
        let file_id = decode_file_id(&entry.file_id).ok_or(PairFileError::Corrupt)?;
        if !file_ids.insert(file_id) {
            return Err(PairFileError::Corrupt);
        }
        let secret = BASE64URL_NOPAD
            .decode(entry.secret.as_bytes())
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or(PairFileError::Corrupt)?;
        let permission = parse_permission(&entry.permission).ok_or(PairFileError::Corrupt)?;
        let label = normalize_label(entry.label.as_deref()).map_err(|_| PairFileError::Corrupt)?;
        if entry.failures > PAIRFILE_MAX_FAILURES {
            return Err(PairFileError::Corrupt);
        }
        files.push(Record {
            file_id,
            secret: Zeroizing::new(secret),
            label,
            permission,
            created_at_us: entry.created_at_us,
            expires_at_us: entry.expires_at_us,
            state: entry.state,
            failures: entry.failures,
            used_by: entry.used_by,
            used_at_us: entry.used_at_us,
        });
    }
    Ok(Doc { files })
}

fn normalize_label(label: Option<&str>) -> Result<Option<String>, PairFileError> {
    let Some(label) = label else {
        return Ok(None);
    };
    let label = label.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.chars().count() > 64 || label.chars().any(char::is_control) {
        return Err(PairFileError::InvalidLabel);
    }
    Ok(Some(label.to_owned()))
}

fn normalize_expired(doc: &mut Doc, now_us: u64) {
    for record in &mut doc.files {
        if record.state == PairFileState::Active && now_us >= record.expires_at_us {
            record.state = PairFileState::Expired;
        }
    }
}

fn effective_state(record: &Record, now_us: u64) -> PairFileState {
    if record.state == PairFileState::Active && now_us >= record.expires_at_us {
        PairFileState::Expired
    } else {
        record.state
    }
}

fn terminal_expired(record: &Record, now_us: u64) -> bool {
    if record.state == PairFileState::Active {
        return false;
    }
    let terminal_at = record
        .used_at_us
        .unwrap_or(0)
        .max(record.expires_at_us)
        .max(record.created_at_us);
    now_us.saturating_sub(terminal_at) > TERMINAL_RETENTION_US
}

fn encode_file_id(file_id: &[u8; 16]) -> String {
    BASE64URL_NOPAD.encode(file_id)
}

fn decode_file_id(value: &str) -> Option<[u8; 16]> {
    BASE64URL_NOPAD
        .decode(value.as_bytes())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
}

fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::ViewOnly => "view_only",
        Permission::Control => "control",
        Permission::Unattended => "unattended",
    }
}

fn parse_permission(value: &str) -> Option<Permission> {
    match value {
        "view_only" => Some(Permission::ViewOnly),
        "control" => Some(Permission::Control),
        "unattended" => Some(Permission::Unattended),
        _ => None,
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), PairFileError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| PairFileError::Corrupt)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut file = options.open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    tighten_file(&tmp)?;
    fs::rename(&tmp, path)?;
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
}

#[cfg(unix)]
fn tighten_dir(path: &Path) -> Result<(), PairFileError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o700 {
        warn!(path = %path.display(), "tightening pairing file directory permissions");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_dir(_path: &Path) -> Result<(), PairFileError> {
    Ok(())
}

#[cfg(unix)]
fn tighten_file(path: &Path) -> Result<(), PairFileError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        warn!(path = %path.display(), "tightening pairing file permissions");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_file(_path: &Path) -> Result<(), PairFileError> {
    Ok(())
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn sync_dir(path: &Path) -> Result<(), PairFileError> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}
