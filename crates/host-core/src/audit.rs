//! Bounded, durable audit spool. No network access or platform code lives here.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// Connection path recorded with an audit event. Values outside the known
/// set, including ones written by newer versions, read back as `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPath {
    TailnetDirect,
    TailnetDerp,
    Intranet,
    Lan,
    #[serde(other)]
    Unknown,
}

/// One audit event. `ts` is Unix microseconds while queued and in the local
/// history; [`AuditQueue::batch`] reports it in seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub ts: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub server: Option<String>,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<AuditPath>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_s: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<serde_json::Value>,
}

/// A queued event with its deduplication key `(host_id, event, ts_us, seq)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostAuditRecord {
    #[serde(flatten)]
    pub record: AuditEvent,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ts_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seq: Option<u32>,
}

impl Deref for HostAuditRecord {
    type Target = AuditEvent;

    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

impl DerefMut for HostAuditRecord {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.record
    }
}

impl From<AuditEvent> for HostAuditRecord {
    fn from(record: AuditEvent) -> Self {
        Self {
            record,
            ts_us: None,
            seq: None,
        }
    }
}

pub const MAX_EVENTS: usize = 10_000;
/// Maximum number of locally retained audit history records.
pub const HISTORY_MAX: usize = 2_000;
const COMPACT_AFTER: usize = 256;
const SEQ_WINDOW_US: u64 = 60_000_000;

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit queue I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("audit queue JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("audit queue lock was poisoned")]
    Poisoned,
    #[error("audit queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("audit sequence exhausted for this microsecond")]
    SeqExhausted,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: u64,
    event: AuditEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seq: Option<u32>,
}

// A small append-only journal avoids rewriting 10,000 entries on every
// overflow. Replay applies the same bound and counts discarded events.
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Record {
    Push(Entry),
    Ack {
        through: u64,
    },
    State {
        next_id: u64,
        dropped: u64,
        #[serde(default = "default_capacity")]
        max_events: usize,
    },
}

fn default_capacity() -> usize {
    MAX_EVENTS
}

struct State {
    entries: VecDeque<Entry>,
    max_events: usize,
    next_id: u64,
    dropped: u64,
    journal_ops: usize,
    seqs: BTreeMap<u64, u32>,
    max_ts_us_seen: u64,
}

#[derive(Default)]
struct History {
    entries: VecDeque<AuditEvent>,
    file_lines: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            max_events: default_capacity(),
            next_id: 1,
            dropped: 0,
            journal_ops: 0,
            seqs: BTreeMap::new(),
            max_ts_us_seen: 0,
        }
    }
}

impl State {
    fn assign_sequences(&mut self) -> Result<(), AuditError> {
        // Seed all persisted sequences before assigning legacy entries, even
        // when a legacy entry precedes an already-numbered entry in the queue.
        for entry in &self.entries {
            self.max_ts_us_seen = self.max_ts_us_seen.max(entry.event.ts);
            if let Some(seq) = entry.seq {
                let next = seq.checked_add(1).ok_or(AuditError::SeqExhausted)?;
                let counter = self.seqs.entry(entry.event.ts).or_default();
                *counter = (*counter).max(next);
            }
        }
        for entry in &mut self.entries {
            if entry.seq.is_none() {
                let counter = self.seqs.entry(entry.event.ts).or_default();
                let seq = *counter;
                *counter = seq.checked_add(1).ok_or(AuditError::SeqExhausted)?;
                entry.seq = Some(seq);
            }
        }
        Ok(())
    }

    fn prune_sequences(&mut self) {
        let cutoff = self.max_ts_us_seen.saturating_sub(SEQ_WINDOW_US);
        while self
            .seqs
            .first_key_value()
            .is_some_and(|(&ts, _)| ts < cutoff)
        {
            self.seqs.pop_first();
        }
    }

    fn apply(&mut self, record: Record) {
        match record {
            Record::Push(entry) => {
                self.next_id = self.next_id.max(entry.id.saturating_add(1));
                self.entries.push_back(entry);
                self.enforce_capacity();
            }
            Record::Ack { through } => self.entries.retain(|entry| entry.id > through),
            Record::State {
                next_id,
                dropped,
                max_events,
            } => {
                self.next_id = next_id;
                self.dropped = dropped;
                self.max_events = max_events;
            }
        }
        self.journal_ops += 1;
    }

    fn enforce_capacity(&mut self) {
        while self.entries.len() > self.max_events {
            self.entries.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

/// A queue snapshot. Acknowledging it removes only its records, even when
/// concurrent producers filled the queue and evicted some of those records.
#[derive(Clone)]
pub struct AuditBatch {
    pub items: Vec<HostAuditRecord>,
    through: u64,
}

#[derive(Clone)]
pub struct AuditQueue {
    path: Arc<PathBuf>,
    state: Arc<Mutex<State>>,
    history: Arc<Mutex<History>>,
}

impl AuditQueue {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, AuditError> {
        Self::open_with_capacity(dir, MAX_EVENTS)
    }

    /// Opens a durable queue, discarding the oldest records if the new capacity
    /// is smaller than the persisted queue. Discards count toward dropped_count.
    pub fn open_with_capacity(
        dir: impl AsRef<Path>,
        max_events: usize,
    ) -> Result<Self, AuditError> {
        Self::from_path_with_capacity(dir.as_ref().join("audit_queue.jsonl"), max_events)
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, AuditError> {
        Self::from_path_with_capacity(path, MAX_EVENTS)
    }

    fn from_path_with_capacity(
        path: impl Into<PathBuf>,
        max_events: usize,
    ) -> Result<Self, AuditError> {
        if max_events == 0 {
            return Err(AuditError::InvalidCapacity);
        }
        let path = path.into();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let history_path = path.with_extension("history.jsonl");
        let history = load_history(&history_path);
        let mut state = State::default();
        if path.exists() {
            let mut reader = BufReader::new(File::open(&path)?);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 {
                    break;
                }
                match serde_json::from_str::<Record>(&line) {
                    Ok(Record::State { max_events: 0, .. }) => {
                        return Err(AuditError::InvalidCapacity);
                    }
                    Ok(record) => state.apply(record),
                    // Only an unterminated tail can be a torn append. Never
                    // silently discard a complete corrupt audit record.
                    Err(_) if !line.ends_with('\n') => break,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        // Replay with the journal's capacity first: a larger new capacity must
        // not resurrect evicted records, nor a smaller one count past acks as drops.
        state.max_events = max_events;
        state.enforce_capacity();
        state.assign_sequences()?;
        let queue = Self {
            path: Arc::new(path),
            state: Arc::new(Mutex::new(state)),
            history: Arc::new(Mutex::new(history)),
        };
        queue.compact()?;
        Ok(queue)
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Path of the local audit history, independent of the queue.
    pub fn history_path(&self) -> PathBuf {
        self.path.with_extension("history.jsonl")
    }
    pub fn len(&self) -> usize {
        self.state.lock().map(|s| s.entries.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn dropped_count(&self) -> u64 {
        self.state.lock().map(|s| s.dropped).unwrap_or(0)
    }

    pub fn push(&self, event: AuditEvent) -> Result<(), AuditError> {
        let mut state = self.lock()?;
        // Producers supply Unix microseconds; only the batch snapshot uses
        // seconds in AuditEvent.ts. Capacity eviction never recycles a seq.
        let ts_us = event.ts;
        let previous = state.seqs.get(&ts_us).copied();
        let seq = previous.unwrap_or(0);
        let next = seq.checked_add(1).ok_or(AuditError::SeqExhausted)?;
        state.seqs.insert(ts_us, next);
        let history_event = event.clone();
        let record = Record::Push(Entry {
            id: state.next_id,
            event,
            seq: Some(seq),
        });
        if let Err(error) = self.append(&record) {
            match previous {
                Some(next) => {
                    state.seqs.insert(ts_us, next);
                }
                None => {
                    state.seqs.remove(&ts_us);
                }
            }
            return Err(error);
        }
        state.apply(record);
        state.max_ts_us_seen = state.max_ts_us_seen.max(ts_us);
        state.prune_sequences();
        self.append_history(&history_event);
        self.maybe_compact(&mut state)?;
        Ok(())
    }

    /// Returns recent local history, ordered by timestamp (newest first).
    /// Equal timestamps retain insertion order; a page boundary can therefore
    /// split equal timestamps and skip one on the next `before_us` page.
    pub fn history(&self, limit: usize, before_us: Option<u64>) -> Vec<AuditEvent> {
        let history = self
            .history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut entries: Vec<_> = history
            .entries
            .iter()
            .filter(|event| before_us.is_none_or(|before| event.ts < before))
            .cloned()
            .collect();
        entries.sort_by_key(|event| std::cmp::Reverse(event.ts));
        entries.truncate(limit);
        entries
    }

    pub fn batch(&self, limit: usize) -> Result<AuditBatch, AuditError> {
        let state = self.lock()?;
        let entries: Vec<_> = state.entries.iter().take(limit).collect();
        Ok(AuditBatch {
            through: entries.last().map_or(0, |entry| entry.id),
            items: entries
                .into_iter()
                .map(|entry| {
                    let ts_us = entry.event.ts;
                    HostAuditRecord {
                        record: AuditEvent {
                            ts: ts_us / 1_000_000,
                            ..entry.event.clone()
                        },
                        ts_us: Some(ts_us),
                        seq: Some(entry.seq.expect("assigned on push/open")),
                    }
                })
                .collect(),
        })
    }

    pub fn acknowledge(&self, batch: &AuditBatch) -> Result<(), AuditError> {
        if batch.items.is_empty() {
            return Ok(());
        }
        let mut state = self.lock()?;
        let record = Record::Ack {
            through: batch.through,
        };
        self.append(&record)?;
        state.apply(record);
        // Reclaim acknowledged contents promptly; failures retain a replayable ack.
        self.rewrite(&mut state)
    }

    pub fn compact(&self) -> Result<(), AuditError> {
        let mut state = self.lock()?;
        self.rewrite(&mut state)
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, AuditError> {
        self.state.lock().map_err(|_| AuditError::Poisoned)
    }

    fn append_history(&self, event: &AuditEvent) {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        history.entries.push_back(event.clone());
        if history.entries.len() > HISTORY_MAX {
            history.entries.pop_front();
        }

        let mut options = OpenOptions::new();
        options.create(true).append(true);
        private_options(&mut options);
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let mut file = options.open(self.history_path())?;
            serde_json::to_writer(&mut file, event)?;
            file.write_all(b"\n")?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(error = %error, "failed to append audit history");
            return;
        }
        history.file_lines = history.file_lines.saturating_add(1);
        if history.file_lines > 2 * HISTORY_MAX {
            if let Err(error) = rewrite_history(&self.history_path(), &history.entries) {
                tracing::warn!(error = %error, "failed to rewrite audit history");
            } else {
                history.file_lines = history.entries.len();
            }
        }
    }

    fn append(&self, record: &Record) -> Result<(), AuditError> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        private_options(&mut options);
        let mut file = options.open(self.path.as_ref())?;
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        file.sync_data()?;
        Ok(())
    }

    fn maybe_compact(&self, state: &mut State) -> Result<(), AuditError> {
        if state.journal_ops >= COMPACT_AFTER {
            self.rewrite(state)?;
        }
        Ok(())
    }

    fn rewrite(&self, state: &mut State) -> Result<(), AuditError> {
        let temp = self.path.with_extension("jsonl.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        private_options(&mut options);
        let file = options.open(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut writer = BufWriter::new(&file);
        serde_json::to_writer(
            &mut writer,
            &Record::State {
                next_id: state.next_id,
                dropped: state.dropped,
                max_events: state.max_events,
            },
        )?;
        writer.write_all(b"\n")?;
        for entry in &state.entries {
            serde_json::to_writer(&mut writer, &Record::Push(entry.clone()))?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        file.sync_all()?;
        fs::rename(temp, self.path.as_ref())?;
        #[cfg(unix)]
        File::open(
            self.path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        state.journal_ops = 0;
        Ok(())
    }
}

fn private_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn load_history(path: &Path) -> History {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return History::default(),
        Err(error) => {
            tracing::warn!(error = %error, "failed to read audit history");
            return History::default();
        }
    };
    let mut reader = BufReader::new(file);
    let mut history = History::default();
    let mut line = String::new();
    let mut skipped = 0usize;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                history.file_lines = history.file_lines.saturating_add(1);
                match serde_json::from_str::<AuditEvent>(&line) {
                    Ok(event) => {
                        history.entries.push_back(event);
                        if history.entries.len() > HISTORY_MAX {
                            history.entries.pop_front();
                        }
                    }
                    Err(_) => skipped = skipped.saturating_add(1),
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to read audit history");
                return History::default();
            }
        }
    }
    if skipped > 0 {
        tracing::warn!(skipped, "skipped invalid audit history lines");
    }
    if history.file_lines > HISTORY_MAX {
        if let Err(error) = rewrite_history(path, &history.entries) {
            tracing::warn!(error = %error, "failed to rewrite audit history");
        } else {
            history.file_lines = history.entries.len();
        }
    }
    history
}

fn rewrite_history(path: &Path, entries: &VecDeque<AuditEvent>) -> io::Result<()> {
    let temp = path.with_extension("jsonl.tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    private_options(&mut options);
    let file = options.open(&temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let result = (|| -> io::Result<()> {
        let mut writer = BufWriter::new(&file);
        for event in entries {
            serde_json::to_writer(&mut writer, event).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        File::open(
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
