//! The process-wide transfer table shared by computer and SFTP transfers.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use super::dto_core::CoreEvent;
use super::dto_files::{FileSourceRef, FileTransfer, FileTransferState};
use super::error::CoreError;
use super::runtime::Core;

const STATE_FILE: &str = "transfers.json";
const STATE_TEMP_FILE: &str = "transfers.json.tmp";
const STATE_BAD_FILE: &str = "transfers.json.bad";
const MAX_TRANSFERS: usize = 200;
const EVENT_INTERVAL_US: u64 = 250_000;
const SPEED_WINDOW_US: u64 = 2_000_000;
const PERSIST_INTERVAL_US: u64 = 5_000_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Computer,
    Server,
}

impl SourceKind {
    pub fn of(src: &FileSourceRef) -> Self {
        match src {
            FileSourceRef::Computer { .. } => Self::Computer,
            FileSourceRef::Server { .. } => Self::Server,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Computer => "computer",
            Self::Server => "server",
        }
    }
}

#[async_trait]
pub trait TransferDriver: Send + Sync {
    fn pause(&self, core: &Arc<Core>, id: &str);
    fn cancel(&self, core: &Arc<Core>, id: &str);
    async fn resume(&self, core: &Arc<Core>, id: &str) -> Result<(), CoreError>;
}

struct Tracked {
    transfer: FileTransfer,
    samples: VecDeque<(u64, u64)>,
    last_event_us: u64,
    last_persist_us: u64,
}

impl Tracked {
    fn new(transfer: FileTransfer, now_us: u64, emitted_now: bool) -> Self {
        let mut samples = VecDeque::new();
        if transfer.state == FileTransferState::Running {
            samples.push_back((now_us, transfer.done));
        }
        Self {
            transfer,
            samples,
            last_event_us: if emitted_now { now_us } else { 0 },
            last_persist_us: now_us,
        }
    }
}

#[derive(Default)]
struct Inner {
    loaded: bool,
    entries: BTreeMap<String, Tracked>,
    drivers: HashMap<SourceKind, Arc<dyn TransferDriver>>,
}

#[derive(Default)]
pub struct Transfers {
    inner: Mutex<Inner>,
    persist_lock: Mutex<()>,
}

impl Transfers {
    pub fn register_driver(&self, kind: SourceKind, driver: Arc<dyn TransferDriver>) {
        lock(&self.inner).drivers.insert(kind, driver);
    }

    pub fn create(&self, core: &Arc<Core>, transfer: FileTransfer) {
        let now_us = core.now_us();
        let mut transfer = transfer;
        transfer.updated_at = now_us;
        let event = {
            let mut inner = lock(&self.inner);
            ensure_loaded(&mut inner, core);
            let last_event_us = inner
                .entries
                .get(&transfer.transfer_id)
                .map_or(now_us, |entry| entry.last_event_us);
            let last_persist_us = inner
                .entries
                .get(&transfer.transfer_id)
                .map_or(now_us, |entry| entry.last_persist_us);
            let mut tracked = Tracked::new(transfer.clone(), now_us, false);
            tracked.last_event_us = last_event_us;
            tracked.last_persist_us = last_persist_us;
            inner.entries.insert(transfer.transfer_id.clone(), tracked);
            prune(&mut inner.entries);
            CoreEvent::FileTransfer {
                transfer: transfer.clone(),
            }
        };
        tracing::debug!(
            transfer_id = %transfer.transfer_id,
            state = state_label(&transfer.state),
            size = transfer.size,
            "transfer created"
        );
        core.events().emit(&event);
        self.persist(core);
    }

    pub fn update(
        &self,
        core: &Arc<Core>,
        id: &str,
        update: impl FnOnce(&mut FileTransfer),
    ) -> Option<FileTransfer> {
        let now_us = core.now_us();
        let (transfer, event, persist) = {
            let mut inner = lock(&self.inner);
            ensure_loaded(&mut inner, core);
            let entry = inner.entries.get_mut(id)?;
            let old_state = entry.transfer.state.clone();
            let old_fail = entry.transfer.fail.clone();
            update(&mut entry.transfer);
            entry.transfer.updated_at = now_us;
            let state_changed = entry.transfer.state != old_state;
            let fail_changed = entry.transfer.fail != old_fail;
            if entry.transfer.state != FileTransferState::Running {
                entry.transfer.bytes_per_sec = 0;
                entry.samples.clear();
            }
            let immediate = state_changed || fail_changed;
            let should_emit = immediate
                || entry.transfer.done == entry.transfer.size
                || now_us.saturating_sub(entry.last_event_us) >= EVENT_INTERVAL_US;
            let event = should_emit.then(|| {
                entry.last_event_us = now_us;
                CoreEvent::FileTransfer {
                    transfer: entry.transfer.clone(),
                }
            });
            let persist = immediate
                || entry.transfer.done == entry.transfer.size
                || now_us.saturating_sub(entry.last_persist_us) >= PERSIST_INTERVAL_US;
            if persist {
                entry.last_persist_us = now_us;
            }
            (entry.transfer.clone(), event, persist)
        };
        if let Some(event) = event {
            core.events().emit(&event);
        }
        if persist {
            self.persist(core);
        }
        Some(transfer)
    }

    pub fn progress(&self, core: &Arc<Core>, id: &str, done: u64) {
        let now_us = core.now_us();
        let (event, persist) = {
            let mut inner = lock(&self.inner);
            ensure_loaded(&mut inner, core);
            let Some(entry) = inner.entries.get_mut(id) else {
                return;
            };
            entry.transfer.done = done;
            entry.transfer.updated_at = now_us;
            if entry.transfer.state == FileTransferState::Running {
                update_speed(entry, now_us, done);
            } else {
                entry.transfer.bytes_per_sec = 0;
                entry.samples.clear();
            }
            let should_emit = done == entry.transfer.size
                || now_us.saturating_sub(entry.last_event_us) >= EVENT_INTERVAL_US;
            let event = should_emit.then(|| {
                entry.last_event_us = now_us;
                CoreEvent::FileTransfer {
                    transfer: entry.transfer.clone(),
                }
            });
            let persist = now_us.saturating_sub(entry.last_persist_us) >= PERSIST_INTERVAL_US;
            if persist {
                entry.last_persist_us = now_us;
            }
            (event, persist)
        };
        if let Some(event) = event {
            core.events().emit(&event);
        }
        if persist {
            self.persist(core);
        }
    }

    pub fn get(&self, core: &Arc<Core>, id: &str) -> Option<FileTransfer> {
        self.with_loaded(core, |inner| {
            inner.entries.get(id).map(|entry| entry.transfer.clone())
        })
    }

    pub fn list(&self, core: &Arc<Core>) -> Vec<FileTransfer> {
        self.with_loaded(core, |inner| {
            let mut transfers: Vec<_> = inner
                .entries
                .values()
                .map(|entry| entry.transfer.clone())
                .collect();
            transfers.sort_by(|a, b| {
                b.started_at
                    .cmp(&a.started_at)
                    .then_with(|| b.transfer_id.cmp(&a.transfer_id))
            });
            transfers
        })
    }

    fn with_loaded<T>(&self, core: &Arc<Core>, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut inner = lock(&self.inner);
        ensure_loaded(&mut inner, core);
        f(&mut inner)
    }

    /// The table lock only takes a snapshot. The serialized file write happens
    /// after it is released so drivers never wait on disk I/O.
    fn persist(&self, core: &Arc<Core>) {
        let _write = lock(&self.persist_lock);
        let (transfers, removed) = {
            let mut inner = lock(&self.inner);
            let before = inner.entries.len();
            prune(&mut inner.entries);
            let after = inner.entries.len();
            let mut transfers: Vec<_> = inner
                .entries
                .values()
                .map(|entry| entry.transfer.clone())
                .collect();
            transfers.sort_by(|a, b| {
                a.started_at
                    .cmp(&b.started_at)
                    .then_with(|| a.transfer_id.cmp(&b.transfer_id))
            });
            (transfers, before.saturating_sub(after))
        };
        let bytes = match serde_json::to_vec(&transfers) {
            Ok(bytes) => bytes,
            Err(_) => {
                tracing::warn!("could not serialize transfer state");
                return;
            }
        };
        let directory = core.data_dir();
        let target = directory.join(STATE_FILE);
        let temporary = directory.join(STATE_TEMP_FILE);
        let result =
            std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, &target));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
            tracing::warn!("could not persist transfer state");
        } else if removed > 0 {
            tracing::debug!(removed, "pruned old transfer records");
        }
    }
}

pub fn pause(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    let now_us = core.now_us();
    let transfers = core.state::<Transfers>();
    let (event, driver) = {
        let mut inner = lock(&transfers.inner);
        ensure_loaded(&mut inner, core);
        let Some(entry) = inner.entries.get(id) else {
            return Err(not_found(id));
        };
        match entry.transfer.state {
            FileTransferState::Paused => return Ok(()),
            FileTransferState::Queued
            | FileTransferState::Running
            | FileTransferState::Interrupted => {}
            ref state => return Err(not_active(state)),
        }
        let kind = SourceKind::of(&entry.transfer.source);
        let driver = inner
            .drivers
            .get(&kind)
            .cloned()
            .ok_or_else(|| no_driver(kind))?;
        let Some(entry) = inner.entries.get_mut(id) else {
            return Err(not_found(id));
        };
        entry.transfer.state = FileTransferState::Paused;
        entry.transfer.bytes_per_sec = 0;
        entry.transfer.updated_at = now_us;
        entry.samples.clear();
        entry.last_event_us = now_us;
        let event = CoreEvent::FileTransfer {
            transfer: entry.transfer.clone(),
        };
        (event, driver)
    };
    core.events().emit(&event);
    transfers.persist(core);
    driver.pause(core, id);
    Ok(())
}

pub fn list(core: &Arc<Core>) -> Vec<FileTransfer> {
    core.state::<Transfers>().list(core)
}

pub async fn resume(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    let driver = {
        let transfers = core.state::<Transfers>();
        let mut inner = lock(&transfers.inner);
        ensure_loaded(&mut inner, core);
        let Some(entry) = inner.entries.get(id) else {
            return Err(not_found(id));
        };
        match entry.transfer.state {
            FileTransferState::Running | FileTransferState::Queued => return Ok(()),
            FileTransferState::Paused
            | FileTransferState::Interrupted
            | FileTransferState::Failed => {}
            ref state => return Err(not_resumable(state)),
        }
        let kind = SourceKind::of(&entry.transfer.source);
        inner
            .drivers
            .get(&kind)
            .cloned()
            .ok_or_else(|| no_driver(kind))?
    };
    driver.resume(core, id).await
}

pub fn cancel(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    let driver = {
        let transfers = core.state::<Transfers>();
        let mut inner = lock(&transfers.inner);
        ensure_loaded(&mut inner, core);
        let Some(entry) = inner.entries.get(id) else {
            return Err(not_found(id));
        };
        if is_ended(&entry.transfer.state) {
            return Ok(());
        }
        let kind = SourceKind::of(&entry.transfer.source);
        inner
            .drivers
            .get(&kind)
            .cloned()
            .ok_or_else(|| no_driver(kind))?
    };
    driver.cancel(core, id);

    let now_us = core.now_us();
    let transfers = core.state::<Transfers>();
    let event = {
        let mut inner = lock(&transfers.inner);
        let Some(entry) = inner.entries.get_mut(id) else {
            return Ok(());
        };
        entry.transfer.state = FileTransferState::Cancelled;
        entry.transfer.fail = None;
        entry.transfer.bytes_per_sec = 0;
        entry.transfer.updated_at = now_us;
        entry.samples.clear();
        entry.last_event_us = now_us;
        CoreEvent::FileTransfer {
            transfer: entry.transfer.clone(),
        }
    };
    core.events().emit(&event);
    transfers.persist(core);
    Ok(())
}

pub fn remove(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    let transfers = core.state::<Transfers>();
    let terminal = {
        let mut inner = lock(&transfers.inner);
        ensure_loaded(&mut inner, core);
        let Some(entry) = inner.entries.get(id) else {
            return Err(not_found(id));
        };
        is_ended(&entry.transfer.state)
    };
    if !terminal {
        cancel(core, id)?;
    }
    let removed = lock(&transfers.inner).entries.remove(id);
    if removed.is_some() {
        core.events().emit(&CoreEvent::FileTransferRemoved {
            transfer_id: id.to_owned(),
        });
        transfers.persist(core);
    }
    Ok(())
}

fn ensure_loaded(inner: &mut Inner, core: &Arc<Core>) {
    if inner.loaded {
        return;
    }
    inner.loaded = true;
    let now_us = core.now_us();
    let path = core.data_dir().join(STATE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            tracing::warn!("could not read transfer state; starting empty");
            return;
        }
    };
    let loaded = match serde_json::from_slice::<Vec<FileTransfer>>(&bytes) {
        Ok(loaded) => loaded,
        Err(_) => {
            tracing::warn!("transfer state is invalid; starting empty");
            if std::fs::rename(&path, core.data_dir().join(STATE_BAD_FILE)).is_err() {
                tracing::warn!("could not preserve invalid transfer state");
            }
            return;
        }
    };
    for mut transfer in loaded {
        if matches!(
            transfer.state,
            FileTransferState::Queued | FileTransferState::Running | FileTransferState::Verifying
        ) {
            transfer.state = FileTransferState::Interrupted;
            transfer.bytes_per_sec = 0;
        }
        inner.entries.insert(
            transfer.transfer_id.clone(),
            Tracked::new(transfer, now_us, false),
        );
    }
    prune(&mut inner.entries);
}

fn update_speed(entry: &mut Tracked, now_us: u64, done: u64) {
    if entry.samples.back().is_some_and(|(time, _)| now_us < *time) {
        entry.samples.clear();
    }
    if let Some(last) = entry.samples.back_mut() {
        if last.0 == now_us {
            last.1 = done;
        } else {
            entry.samples.push_back((now_us, done));
        }
    } else {
        entry.samples.push_back((now_us, done));
    }
    let cutoff = now_us.saturating_sub(SPEED_WINDOW_US);
    while entry.samples.len() > 1
        && entry
            .samples
            .front()
            .is_some_and(|(time, _)| *time < cutoff)
    {
        entry.samples.pop_front();
    }
    let speed = match (entry.samples.front(), entry.samples.back()) {
        (Some((start_time, start_done)), Some((end_time, end_done))) if start_time != end_time => {
            let elapsed = end_time.saturating_sub(*start_time);
            if elapsed == 0 {
                0
            } else {
                let delta = end_done.saturating_sub(*start_done) as u128;
                (delta * 1_000_000 / elapsed as u128).min(u64::MAX as u128) as u64
            }
        }
        _ => 0,
    };
    entry.transfer.bytes_per_sec = speed;
}

fn prune(entries: &mut BTreeMap<String, Tracked>) {
    while entries.len() > MAX_TRANSFERS {
        let oldest = entries
            .iter()
            .filter(|(_, entry)| is_ended(&entry.transfer.state))
            .min_by(|(left_id, left), (right_id, right)| {
                left.transfer
                    .started_at
                    .cmp(&right.transfer.started_at)
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(id, _)| id.clone());
        let Some(id) = oldest else {
            break;
        };
        entries.remove(&id);
    }
}

fn is_ended(state: &FileTransferState) -> bool {
    matches!(
        state,
        FileTransferState::Completed | FileTransferState::Failed | FileTransferState::Cancelled
    )
}

fn state_label(state: &FileTransferState) -> &'static str {
    match state {
        FileTransferState::Queued => "queued",
        FileTransferState::Running => "running",
        FileTransferState::Paused => "paused",
        FileTransferState::Interrupted => "interrupted",
        FileTransferState::Verifying => "verifying",
        FileTransferState::Completed => "completed",
        FileTransferState::Failed => "failed",
        FileTransferState::Cancelled => "cancelled",
    }
}

fn not_found(id: &str) -> CoreError {
    CoreError::Http {
        status: 404,
        hub_code: None,
        message: format!("transfers.notFound: {id}"),
    }
}

fn not_active(state: &FileTransferState) -> CoreError {
    CoreError::Http {
        status: 409,
        hub_code: None,
        message: format!("transfers.notActive: {}", state_label(state)),
    }
}

fn not_resumable(state: &FileTransferState) -> CoreError {
    CoreError::Http {
        status: 409,
        hub_code: None,
        message: format!("transfers.notResumable: {}", state_label(state)),
    }
}

fn no_driver(kind: SourceKind) -> CoreError {
    CoreError::unsupported(format!("transfers.noDriver: {}", kind.label()))
}
