//! 未结束会话的落盘记录，供重启后补写 `session_end`（05「审计事件口径」）。

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::audit::AuditPath;

pub const JOURNAL_FILE: &str = "session_journal.json";
/// 活动会话的存活时间每隔这么久写一次。
pub const TOUCH_INTERVAL_US: u64 = 60_000_000;
/// 本次运行中同时记录的会话上限；超出时丢 since_us 最小的一条并 warn。
pub const MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenSession {
    pub session_id: String,
    #[serde(default)]
    pub user: Option<String>,
    pub device_id: String,
    #[serde(default)]
    pub path: Option<AuditPath>,
    /// "controller" | "viewer"
    pub role: String,
    pub since_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    #[serde(flatten)]
    pub session: OpenSession,
    /// 进入保留期的时刻；活动中为 None。
    #[serde(default)]
    pub left_at_us: Option<u64>,
    /// 最后一次确认会话还活着的时刻。
    pub alive_at_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredEnd {
    pub session: OpenSession,
    /// `left_at_us.unwrap_or(alive_at_us).max(since_us)`
    pub end_at_us: u64,
    /// `(end_at_us - since_us) / 1_000_000`，向下取整；溢出取 `i64::MAX`
    pub duration_s: i64,
    /// `left_at_us.is_some()`
    pub lingering: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct JournalFile {
    version: u32,
    sessions: Vec<JournalEntry>,
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The in-memory journal for sessions that are active or lingering.
pub struct SessionJournal {
    path: Option<PathBuf>,
    entries: Vec<JournalEntry>,
    leftovers: Vec<JournalEntry>,
    last_touch_us: u64,
}

impl SessionJournal {
    /// Opens the journal without creating its directory or file.
    pub fn open(state_dir: &Path, now_us: u64) -> Self {
        let path = state_dir.join(JOURNAL_FILE);
        let leftovers = match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<JournalFile>(&contents) {
                Ok(file) if file.version == 1 => retain_oldest_entries(file.sessions),
                Ok(_) => {
                    tracing::warn!(path = ?path, "ignoring session journal with unknown version");
                    Vec::new()
                }
                Err(error) => {
                    tracing::warn!(path = ?path, error = %error, "ignoring invalid session journal");
                    Vec::new()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                tracing::warn!(path = ?path, error = %error, "failed to read session journal");
                Vec::new()
            }
        };

        Self {
            path: Some(path),
            entries: Vec::new(),
            leftovers,
            last_touch_us: now_us,
        }
    }

    /// Creates an inert journal that never reads or writes a file.
    pub fn disabled() -> Self {
        Self {
            path: None,
            entries: Vec::new(),
            leftovers: Vec::new(),
            last_touch_us: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    pub fn leftovers(&self) -> Vec<RecoveredEnd> {
        self.leftovers
            .iter()
            .map(|entry| {
                let lingering = entry.left_at_us.is_some();
                let end_at_us = entry
                    .left_at_us
                    .unwrap_or(entry.alive_at_us)
                    .max(entry.session.since_us);
                let duration_s = i64::try_from((end_at_us - entry.session.since_us) / 1_000_000)
                    .map_or(i64::MAX, |value| value);
                RecoveredEnd {
                    session: entry.session.clone(),
                    end_at_us,
                    duration_s,
                    lingering,
                }
            })
            .collect()
    }

    pub fn clear_leftovers(&mut self) {
        if !self.is_enabled() {
            return;
        }
        self.leftovers.clear();
        self.rewrite();
    }

    pub fn start(&mut self, session: OpenSession, now_us: u64) {
        if !self.is_enabled() {
            return;
        }
        let session_id = session.session_id.clone();
        let entry = JournalEntry {
            session,
            left_at_us: None,
            alive_at_us: now_us,
        };
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|existing| existing.session.session_id == session_id)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
            self.trim_entries();
        }
        self.rewrite();
    }

    pub fn set_role(&mut self, session_id: &str, role: &str) {
        if !self.is_enabled() {
            return;
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.session.session_id == session_id)
        else {
            return;
        };
        entry.session.role = role.to_owned();
        self.rewrite();
    }

    pub fn left(&mut self, session_id: &str, at_us: u64) {
        if !self.is_enabled() {
            return;
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.session.session_id == session_id)
        else {
            return;
        };
        entry.left_at_us = Some(at_us);
        self.rewrite();
    }

    pub fn resumed(&mut self, session_id: &str, now_us: u64) {
        if !self.is_enabled() {
            return;
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.session.session_id == session_id)
        else {
            return;
        };
        entry.left_at_us = None;
        entry.alive_at_us = now_us;
        self.rewrite();
    }

    pub fn end(&mut self, session_id: &str) {
        if !self.is_enabled() {
            return;
        }
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.session.session_id == session_id)
        else {
            return;
        };
        self.entries.remove(index);
        self.rewrite();
    }

    pub fn touch_due(&self, now_us: u64) -> bool {
        self.is_enabled()
            && (now_us < self.last_touch_us || now_us - self.last_touch_us >= TOUCH_INTERVAL_US)
    }

    pub fn touch(&mut self, now_us: u64, live: &HashSet<String>) -> bool {
        if !self.touch_due(now_us) {
            return false;
        }
        self.last_touch_us = now_us;
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.left_at_us.is_none() && live.contains(&entry.session.session_id) {
                entry.alive_at_us = now_us;
                changed = true;
            }
        }
        if changed {
            self.rewrite();
        }
        changed
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    fn trim_entries(&mut self) {
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.session.since_us)
            .map(|(index, _)| index)
        {
            self.entries.remove(index);
            tracing::warn!(
                max_entries = MAX_ENTRIES,
                "session journal entry limit reached"
            );
        }
    }

    fn rewrite(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let Some(parent) = path.parent() else {
            tracing::warn!(path = ?path, "failed to determine session journal directory");
            return;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            tracing::warn!(path = ?path, error = %error, "failed to create session journal directory");
            return;
        }

        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{JOURNAL_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let result = self.write_temp_and_rename(path, &temp);
        if let Err(error) = result {
            tracing::warn!(path = ?path, error = %error, "failed to write session journal");
            let _ = fs::remove_file(temp);
        }
    }

    fn write_temp_and_rename(&self, path: &Path, temp: &Path) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(temp)?;
        let mut writer = BufWriter::new(file);
        let mut sessions = Vec::with_capacity(self.leftovers.len() + self.entries.len());
        sessions.extend(self.leftovers.iter().cloned());
        sessions.extend(self.entries.iter().cloned());
        serde_json::to_writer(
            &mut writer,
            &JournalFile {
                version: 1,
                sessions,
            },
        )
        .map_err(io::Error::other)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        drop(writer);
        fs::rename(temp, path)
    }
}

fn retain_oldest_entries(mut entries: Vec<JournalEntry>) -> Vec<JournalEntry> {
    let mut unique = Vec::with_capacity(entries.len().min(MAX_ENTRIES));
    for entry in entries.drain(..) {
        if unique
            .iter()
            .all(|existing: &JournalEntry| existing.session.session_id != entry.session.session_id)
        {
            unique.push(entry);
        }
    }
    if unique.len() <= MAX_ENTRIES {
        return unique;
    }

    let mut indices: Vec<usize> = (0..unique.len()).collect();
    indices.sort_by(|left, right| {
        unique[*right]
            .session
            .since_us
            .cmp(&unique[*left].session.since_us)
            .then_with(|| left.cmp(right))
    });
    indices.truncate(MAX_ENTRIES);
    indices.sort_unstable();
    indices
        .into_iter()
        .map(|index| unique[index].clone())
        .collect()
}
