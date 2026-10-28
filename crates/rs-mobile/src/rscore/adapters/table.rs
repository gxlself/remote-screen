//! Adapter session table.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};

use tokio::sync::{mpsc, oneshot, watch};

use super::driver::{Credentials, DriverInput};
use super::dto::{AdapterOpenOpts, AdapterPhase, AdapterSummary, AdapterTarget};
use super::ADAPTER_ID_BASE;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Default)]
struct TableState {
    next: u64,
    by_id: HashMap<u64, Arc<AdapterEntry>>,
    by_key: HashMap<String, u64>,
}

/// Sessions owned by one [`crate::rscore::runtime::Core`].
#[derive(Default)]
pub struct AdapterTable {
    state: Mutex<TableState>,
}

impl AdapterTable {
    /// Allocate `ADAPTER_ID_BASE + 1`, then monotonically increasing IDs.
    pub fn alloc_id(&self) -> u64 {
        let mut state = lock(&self.state);
        state.next = state
            .next
            .checked_add(1)
            .expect("adapter session id exhausted");
        ADAPTER_ID_BASE
            .checked_add(state.next)
            .expect("adapter session id exhausted")
    }

    /// Register a session. If the store key was already present, return the
    /// previous entry so the caller can close it after releasing this lock.
    pub fn insert(&self, entry: Arc<AdapterEntry>) -> Option<Arc<AdapterEntry>> {
        let mut state = lock(&self.state);
        let old = state
            .by_key
            .insert(entry.store_key.clone(), entry.id)
            .and_then(|id| state.by_id.get(&id).cloned());
        state.by_id.insert(entry.id, entry);
        old
    }

    pub fn get(&self, id: u64) -> Option<Arc<AdapterEntry>> {
        lock(&self.state).by_id.get(&id).cloned()
    }

    /// Remove an ID. The reverse key is removed only when it still points at
    /// this ID, because a replacement can already be registered.
    pub fn remove(&self, id: u64) -> Option<Arc<AdapterEntry>> {
        let mut state = lock(&self.state);
        let entry = state.by_id.remove(&id)?;
        if state.by_key.get(&entry.store_key).copied() == Some(id) {
            state.by_key.remove(&entry.store_key);
        }
        Some(entry)
    }

    pub fn list(&self) -> Vec<AdapterSummary> {
        let mut entries: Vec<_> = lock(&self.state).by_id.values().cloned().collect();
        entries.sort_by_key(|entry| entry.id);
        entries.into_iter().map(|entry| entry.summary()).collect()
    }
}

/// Shared state for one adapter session.
pub struct AdapterEntry {
    pub id: u64,
    pub target: AdapterTarget,
    pub store_key: String,

    pub(crate) input: Mutex<Option<mpsc::Sender<DriverInput>>>,
    pub(crate) cancel: Mutex<Option<oneshot::Sender<()>>>,
    pub(crate) closed_by_user: AtomicBool,
    pub(crate) background: watch::Sender<bool>,
    pub(crate) phase: Mutex<AdapterPhase>,
    pub(crate) width: Mutex<Option<u32>>,
    pub(crate) height: Mutex<Option<u32>>,
    pub(crate) credential_waiter: Mutex<Option<oneshot::Sender<Option<Credentials>>>>,
    pub(crate) cert_waiter: Mutex<Option<oneshot::Sender<bool>>>,

    // Credential policy is per session. `saved_attempted` prevents a failed
    // authentication from trying the saved value repeatedly.
    pub(crate) opts: AdapterOpenOpts,
    pub(crate) saved_attempted: AtomicBool,
    pub(crate) used_saved: AtomicBool,
    pub(crate) pending_credentials: Mutex<Option<(Credentials, bool)>>,
}

impl AdapterEntry {
    pub(crate) fn new(
        id: u64,
        target: AdapterTarget,
        store_key: String,
        opts: AdapterOpenOpts,
        input: mpsc::Sender<DriverInput>,
        cancel: oneshot::Sender<()>,
    ) -> Self {
        Self {
            id,
            target,
            store_key,
            input: Mutex::new(Some(input)),
            cancel: Mutex::new(Some(cancel)),
            closed_by_user: AtomicBool::new(false),
            background: watch::channel(false).0,
            phase: Mutex::new(AdapterPhase::Connecting),
            width: Mutex::new(None),
            height: Mutex::new(None),
            credential_waiter: Mutex::new(None),
            cert_waiter: Mutex::new(None),
            opts,
            saved_attempted: AtomicBool::new(false),
            used_saved: AtomicBool::new(false),
            pending_credentials: Mutex::new(None),
        }
    }

    pub fn summary(&self) -> AdapterSummary {
        AdapterSummary {
            session_id: self.id.to_string(),
            target: self.target.clone(),
            phase: *lock(&self.phase),
            width: *lock(&self.width),
            height: *lock(&self.height),
        }
    }

    pub fn protocol(&self) -> super::dto::AdapterProtocol {
        self.target.protocol
    }

    pub(crate) fn set_phase(&self, phase: AdapterPhase) {
        *lock(&self.phase) = phase;
    }

    pub(crate) fn close_waiters(&self) {
        if let Some(waiter) = lock(&self.credential_waiter).take() {
            let _ = waiter.send(None);
        }
        if let Some(waiter) = lock(&self.cert_waiter).take() {
            let _ = waiter.send(false);
        }
    }

    pub(crate) fn request_close(&self) {
        self.closed_by_user.store(true, Ordering::Release);
        lock(&self.input).take();
        if let Some(cancel) = lock(&self.cancel).take() {
            let _ = cancel.send(());
        }
        lock(&self.pending_credentials).take();
        self.close_waiters();
    }
}
