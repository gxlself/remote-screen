//! Recently completed uploads, so FileQuery can answer COMPLETED after a lost receipt.

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{collections::VecDeque, fs, io, path::Path};
use tracing::warn;

pub const COMPLETED_TTL_US: u64 = 24 * 3600 * 1_000_000;
pub const COMPLETED_MAX: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedUpload {
    pub transfer_id: String,
    pub device_id: String,
    pub size: u64,
    pub saved_name: String,
    pub at_us: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CompletedUploads {
    entries: VecDeque<CompletedUpload>,
}

#[derive(Serialize, Deserialize)]
struct PersistedCompletedUploads {
    version: u32,
    entries: VecDeque<CompletedUpload>,
}

impl CompletedUploads {
    pub fn load(path: &Path, now_us: u64) -> Self {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(%error, "could not read completed upload registry");
                return Self::default();
            }
        };
        let persisted = match serde_json::from_slice::<PersistedCompletedUploads>(&bytes) {
            Ok(persisted) if persisted.version == 1 => persisted,
            Ok(_) | Err(_) => {
                let bad = path.with_file_name(
                    path.file_name()
                        .map(|name| format!("{}.bad", name.to_string_lossy()))
                        .unwrap_or_else(|| "completed-uploads.json.bad".to_owned()),
                );
                let rename_error = fs::rename(path, bad).err();
                warn!(
                    ?rename_error,
                    "completed upload registry is invalid; starting empty"
                );
                return Self::default();
            }
        };
        let mut uploads = Self {
            entries: persisted.entries,
        };
        uploads
            .entries
            .retain(|entry| now_us.saturating_sub(entry.at_us) < COMPLETED_TTL_US);
        while uploads.entries.len() > COMPLETED_MAX {
            uploads.entries.pop_front();
        }
        uploads
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let persisted = PersistedCompletedUploads {
            version: 1,
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec(&persisted).map_err(io::Error::other)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_file_name(
            path.file_name()
                .map(|name| format!("{}.tmp", name.to_string_lossy()))
                .unwrap_or_else(|| "completed-uploads.json.tmp".to_owned()),
        );
        fs::write(&temporary, bytes)?;
        #[cfg(unix)]
        {
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn record(&mut self, entry: CompletedUpload) {
        let at_us = entry.at_us;
        self.entries.retain(|existing| {
            existing.transfer_id != entry.transfer_id || existing.device_id != entry.device_id
        });
        self.entries.push_back(entry);
        self.entries
            .retain(|existing| at_us.saturating_sub(existing.at_us) < COMPLETED_TTL_US);
        while self.entries.len() > COMPLETED_MAX {
            self.entries.pop_front();
        }
    }

    pub fn lookup(
        &mut self,
        transfer_id: &str,
        device_id: &str,
        now_us: u64,
    ) -> Option<CompletedUpload> {
        self.entries
            .retain(|entry| now_us.saturating_sub(entry.at_us) < COMPLETED_TTL_US);
        self.entries
            .iter()
            .find(|entry| entry.transfer_id == transfer_id && entry.device_id == device_id)
            .cloned()
    }

    pub fn forget(&mut self, transfer_id: &str, device_id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.transfer_id != transfer_id || entry.device_id != device_id);
        self.entries.len() != before
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(transfer_id: &str, device_id: &str, size: u64, at_us: u64) -> CompletedUpload {
        CompletedUpload {
            transfer_id: transfer_id.to_owned(),
            device_id: device_id.to_owned(),
            size,
            saved_name: "a.txt".to_owned(),
            at_us,
        }
    }

    #[test]
    fn lookup_matches_transfer_and_device() {
        let mut uploads = CompletedUploads::default();
        let recorded = entry("id", "dev-a", 42, 1_000);
        uploads.record(recorded.clone());

        assert_eq!(uploads.lookup("id", "dev-a", 1_000), Some(recorded));
        assert_eq!(uploads.lookup("id", "dev-b", 1_000), None);
        assert_eq!(uploads.lookup("unknown", "dev-a", 1_000), None);
    }

    #[test]
    fn lookup_expires_entries_at_ttl() {
        let mut uploads = CompletedUploads::default();
        uploads.record(entry("id", "dev-a", 42, 1_000));

        assert!(uploads
            .lookup("id", "dev-a", 1_000 + COMPLETED_TTL_US - 1)
            .is_some());
        assert_eq!(
            uploads.lookup("id", "dev-a", 1_000 + COMPLETED_TTL_US),
            None
        );
        assert!(uploads.is_empty());
    }

    #[test]
    fn lookup_does_not_expire_when_clock_moves_back() {
        let mut uploads = CompletedUploads::default();
        uploads.record(entry("id", "dev-a", 42, 5_000));

        assert!(uploads.lookup("id", "dev-a", 1_000).is_some());
    }

    #[test]
    fn record_evicts_oldest_entry_at_capacity() {
        let mut uploads = CompletedUploads::default();
        for index in 0..=COMPLETED_MAX {
            uploads.record(entry(
                &index.to_string(),
                "dev-a",
                index as u64,
                index as u64,
            ));
        }

        assert_eq!(uploads.len(), COMPLETED_MAX);
        assert_eq!(uploads.lookup("0", "dev-a", COMPLETED_MAX as u64), None);
        assert!(uploads
            .lookup(&COMPLETED_MAX.to_string(), "dev-a", COMPLETED_MAX as u64,)
            .is_some());
    }

    #[test]
    fn record_replaces_same_transfer_and_device() {
        let mut uploads = CompletedUploads::default();
        uploads.record(entry("id", "dev-a", 1, 1_000));
        uploads.record(entry("id", "dev-a", 2, 1_001));

        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads.lookup("id", "dev-a", 1_001).unwrap().size, 2);
    }

    #[test]
    fn forget_removes_only_matching_device_entry() {
        let mut uploads = CompletedUploads::default();
        uploads.record(entry("id", "dev-a", 1, 1_000));
        uploads.record(entry("id", "dev-b", 2, 1_000));

        assert!(uploads.forget("id", "dev-a"));
        assert!(!uploads.forget("id", "dev-a"));
        assert!(uploads.lookup("id", "dev-b", 1_000).is_some());
    }

    #[test]
    fn save_and_load_round_trip() {
        let directory = std::path::PathBuf::from(format!(
            "target/completed-uploads-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("completed-uploads.json");
        let mut uploads = CompletedUploads::default();
        uploads.record(entry("id", "dev-a", 42, 1_000));
        uploads.save(&path).unwrap();

        let mut loaded = CompletedUploads::load(&path, 1_000);
        assert_eq!(
            loaded.lookup("id", "dev-a", 1_000),
            uploads.lookup("id", "dev-a", 1_000)
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
