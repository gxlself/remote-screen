//! Host-key pinning. The App supplies a Rust-managed SQLite store in A16.

use std::{collections::HashMap, sync::Mutex};

/// The complete SSH wire key is pinned, not only its displayed fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyEntry {
    pub algorithm: String,
    pub key: Vec<u8>,
    pub fingerprint: String,
}

pub trait HostKeyStore: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> Option<HostKeyEntry>;
    fn save(&self, host: &str, port: u16, entry: HostKeyEntry);
}

#[derive(Default, Debug)]
pub struct MemoryHostKeyStore(Mutex<HashMap<(String, u16), HostKeyEntry>>);

impl HostKeyStore for MemoryHostKeyStore {
    fn lookup(&self, host: &str, port: u16) -> Option<HostKeyEntry> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(host.to_owned(), port))
            .cloned()
    }

    fn save(&self, host: &str, port: u16, entry: HostKeyEntry) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((host.to_owned(), port), entry);
    }
}
