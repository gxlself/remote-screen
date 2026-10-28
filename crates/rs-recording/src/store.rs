//! 录制目录 `<dataDir>/recordings/`：id 与路径、空间检查、列表缓存、`.part` 恢复、保留期与删除。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use data_encoding::HEXLOWER;

use crate::crypto::{SecretWrapper, WrapError};
use crate::file::{self, FileError, RecordingReader, RecoverKind, HEADER_FIXED_LEN, PART_SUFFIX};
use crate::record::{self, Body, Record, RecordingEnd, RecordingMeta};

pub const GIB: u64 = 1 << 30;
pub const MIB: u64 = 1 << 20;

const EXTENSION: &str = ".rsrec";
const DAY_US: u64 = 86_400_000_000;
const REASON_RECOVERED: &str = "recovered";
const REASON_DAMAGED: &str = "damaged";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    pub max_total_bytes: u64,
    /// None = 永久保留。
    pub retention_days: Option<u32>,
    pub min_free_to_start: u64,
    pub min_free_while_recording: u64,
    pub max_duration_us: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_total_bytes: 4096 * MIB,
            retention_days: Some(30),
            min_free_to_start: GIB,
            min_free_while_recording: 500 * MIB,
            max_duration_us: 2 * 3600 * 1_000_000,
        }
    }
}

impl StoreConfig {
    /// 设置项 `recording.maxTotalMb` 和 `recording.retentionDays`，其余取默认值。
    pub fn from_settings(max_total_mb: u64, retention_days: Option<u32>) -> Self {
        Self {
            max_total_bytes: max_total_mb.saturating_mul(MIB),
            retention_days,
            ..Self::default()
        }
    }
}

pub trait SpaceProbe: Send + Sync {
    /// dir 所在文件系统的可用字节数；查不到返回 None（按「空间足够」处理）。
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefusal {
    StorageFull,
    DiskLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    StorageFull,
    DiskLow,
    MaxDuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredState {
    Complete,
    Recovered,
    Damaged,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredRecording {
    pub id: String,
    pub meta: RecordingMeta,
    pub end: RecordingEnd,
    pub bytes: u64,
    pub state: StoredState,
    pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoverReport {
    Finished { id: String, kind: RecoverKind },
    RemovedEmpty { id: String },
    Skipped { id: String, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("recording not found")]
    NotFound,
    #[error("invalid recording id")]
    InvalidId,
    #[error("recording is in use")]
    Busy,
    #[error("recording key unavailable")]
    KeyUnavailable,
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    File(#[from] FileError),
    #[error("random source failed")]
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

struct Entry {
    id: String,
    path: PathBuf,
    part: bool,
    stamp: Stamp,
}

#[derive(Clone)]
struct Loaded {
    recording: StoredRecording,
    /// 保留期的起算时刻；None = 永不自动删除（version 不认识、文件头读不出）。
    retain_from: Option<u64>,
}

struct Cached {
    stamp: Stamp,
    loaded: Loaded,
}

enum ReadOutcome {
    Cache(Loaded),
    /// 读文件时出了 IO 错误：照样列出，但下次重读。
    NoCache(Loaded),
    Gone,
    KeyUnavailable,
}

pub struct RecordingStore {
    dir: PathBuf,
    probe: Arc<dyn SpaceProbe>,
    cache: Mutex<HashMap<PathBuf, Cached>>,
}

impl RecordingStore {
    /// dir 就是 recordings 目录本身；不存在就 create_dir_all。
    pub fn open(dir: PathBuf, probe: Arc<dyn SpaceProbe>) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            probe,
            cache: Mutex::default(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 16 字节随机数的小写十六进制（32 个字符）。
    pub fn new_id() -> Result<String, StoreError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| StoreError::Random)?;
        Ok(HEXLOWER.encode(&bytes))
    }

    /// 不是 32 位小写十六进制就返回 false。所有接受 id 的方法都先检查它，防止路径穿越。
    pub fn valid_id(id: &str) -> bool {
        id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    /// `<dir>/<id>.rsrec.part`
    pub fn part_path(&self, id: &str) -> PathBuf {
        debug_assert!(Self::valid_id(id), "invalid recording id");
        self.dir.join(format!("{id}{EXTENSION}{PART_SUFFIX}"))
    }

    /// `<dir>/<id>.rsrec`
    pub fn final_path(&self, id: &str) -> PathBuf {
        debug_assert!(Self::valid_id(id), "invalid recording id");
        self.dir.join(format!("{id}{EXTENSION}"))
    }

    /// 所有 <id>.rsrec 和 <id>.rsrec.part 的字节数之和。
    pub fn used_bytes(&self) -> u64 {
        match self.scan() {
            Ok(entries) => entries
                .iter()
                .fold(0, |used, entry| used.saturating_add(entry.stamp.len)),
            Err(error) => {
                tracing::warn!(%error, "recording directory scan failed");
                0
            }
        }
    }

    pub fn free_bytes(&self) -> Option<u64> {
        self.probe.free_bytes(&self.dir)
    }

    /// used >= max_total_bytes → StorageFull；free < min_free_to_start → DiskLow。两者都满足时报 StorageFull。
    pub fn check_can_start(&self, cfg: &StoreConfig) -> Result<(), StartRefusal> {
        if self.used_bytes() >= cfg.max_total_bytes {
            return Err(StartRefusal::StorageFull);
        }
        if self
            .free_bytes()
            .is_some_and(|free| free < cfg.min_free_to_start)
        {
            return Err(StartRefusal::DiskLow);
        }
        Ok(())
    }

    /// 纯函数：录制中按节奏调用。
    /// duration >= max → MaxDuration；used_others + recorded >= max_total → StorageFull；free < min_free_while_recording → DiskLow。
    pub fn should_stop(
        cfg: &StoreConfig,
        used_others: u64,
        recorded: u64,
        duration_us: u64,
        free: Option<u64>,
    ) -> Option<StopReason> {
        if duration_us >= cfg.max_duration_us {
            Some(StopReason::MaxDuration)
        } else if used_others.saturating_add(recorded) >= cfg.max_total_bytes {
            Some(StopReason::StorageFull)
        } else if free.is_some_and(|free| free < cfg.min_free_while_recording) {
            Some(StopReason::DiskLow)
        } else {
            None
        }
    }

    /// 只列 .rsrec（不含 .part），按 meta.started_at_us 从新到旧。
    pub fn list(&self, wrapper: &dyn SecretWrapper) -> Result<Vec<StoredRecording>, StoreError> {
        let entries: Vec<Entry> = self
            .scan()?
            .into_iter()
            .filter(|entry| !entry.part)
            .collect();
        let live: HashSet<&Path> = entries.iter().map(|entry| entry.path.as_path()).collect();
        self.cache().retain(|path, _| live.contains(path.as_path()));
        let mut recordings = Vec::with_capacity(entries.len());
        for entry in &entries {
            if let Some(loaded) = self.load(entry, wrapper)? {
                recordings.push(loaded.recording);
            }
        }
        recordings.sort_by(|a, b| {
            b.meta
                .started_at_us
                .cmp(&a.meta.started_at_us)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(recordings)
    }

    pub fn get(
        &self,
        id: &str,
        wrapper: &dyn SecretWrapper,
    ) -> Result<StoredRecording, StoreError> {
        if !Self::valid_id(id) {
            return Err(StoreError::InvalidId);
        }
        let path = self.final_path(id);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Err(StoreError::NotFound),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cache().remove(&path);
                return Err(StoreError::NotFound);
            }
            Err(error) => return Err(error.into()),
        };
        let entry = Entry {
            id: id.to_owned(),
            path,
            part: false,
            stamp: Stamp::of(&metadata),
        };
        self.load(&entry, wrapper)?
            .map(|loaded| loaded.recording)
            .ok_or(StoreError::NotFound)
    }

    /// 恢复所有不在 active 里的 .part。
    pub fn recover_all(
        &self,
        wrapper: &dyn SecretWrapper,
        active: &HashSet<String>,
        now_us: u64,
    ) -> Vec<RecoverReport> {
        let entries = match self.scan() {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(%error, "recording directory scan failed");
                return Vec::new();
            }
        };
        entries
            .iter()
            .filter(|entry| entry.part && !active.contains(&entry.id))
            .map(|entry| self.recover(entry, wrapper, now_us))
            .collect()
    }

    /// 删除 started_at + days * 86_400_000_000 < now_us 的 .rsrec（不碰 .part 和 active）。返回删掉的 id。
    /// started_at 取 meta.started_at_us；Meta 读不出时取文件头的 created_at_us；version 不认识的文件不删。
    pub fn apply_retention(
        &self,
        cfg: &StoreConfig,
        wrapper: &dyn SecretWrapper,
        now_us: u64,
        active: &HashSet<String>,
    ) -> Vec<String> {
        let Some(days) = cfg.retention_days else {
            return Vec::new();
        };
        let keep_us = u64::from(days).saturating_mul(DAY_US);
        let entries = match self.scan() {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(%error, "recording directory scan failed");
                return Vec::new();
            }
        };
        let mut removed = Vec::new();
        for entry in entries
            .iter()
            .filter(|entry| !entry.part && !active.contains(&entry.id))
        {
            // 主密钥拿不到时 Meta 读不出，也不知道文件好坏：这次先不删。
            let retain_from = match self.load(entry, wrapper) {
                Ok(Some(loaded)) => loaded.retain_from,
                Ok(None) | Err(_) => None,
            };
            let expired = retain_from.is_some_and(|from| from.saturating_add(keep_us) < now_us);
            if !expired {
                continue;
            }
            match fs::remove_file(&entry.path) {
                Ok(()) => {
                    self.cache().remove(&entry.path);
                    removed.push(entry.id.clone());
                }
                Err(error) => {
                    tracing::warn!(id = %entry.id, %error, "expired recording was not deleted");
                }
            }
        }
        removed
    }

    /// 删除 .rsrec 或 .rsrec.part；id 在 active 里 → Busy；不存在 → NotFound。
    pub fn delete(&self, id: &str, active: &HashSet<String>) -> Result<(), StoreError> {
        if !Self::valid_id(id) {
            return Err(StoreError::InvalidId);
        }
        if active.contains(id) {
            return Err(StoreError::Busy);
        }
        let final_path = self.final_path(id);
        self.cache().remove(&final_path);
        let mut deleted = false;
        for path in [final_path, self.part_path(id)] {
            match fs::remove_file(&path) {
                Ok(()) => deleted = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if deleted {
            Ok(())
        } else {
            Err(StoreError::NotFound)
        }
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<PathBuf, Cached>> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 目录里名字是 `<id>.rsrec` 或 `<id>.rsrec.part` 的普通文件，按 id 排序；其他一律忽略。
    fn scan(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for dir_entry in fs::read_dir(&self.dir)? {
            let dir_entry = dir_entry?;
            let Some((id, part)) = dir_entry.file_name().to_str().and_then(parse_name) else {
                continue;
            };
            let metadata = match dir_entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.is_file() {
                entries.push(Entry {
                    id,
                    path: dir_entry.path(),
                    part,
                    stamp: Stamp::of(&metadata),
                });
            }
        }
        entries.sort_by(|a, b| (&a.id, a.part).cmp(&(&b.id, b.part)));
        Ok(entries)
    }

    /// `(路径, 长度, mtime)` 都没变就用缓存，不重新解密。
    fn load(
        &self,
        entry: &Entry,
        wrapper: &dyn SecretWrapper,
    ) -> Result<Option<Loaded>, StoreError> {
        let cached = self
            .cache()
            .get(&entry.path)
            .filter(|cached| cached.stamp == entry.stamp)
            .map(|cached| cached.loaded.clone());
        if cached.is_some() {
            return Ok(cached);
        }
        match read_recording(entry, wrapper) {
            ReadOutcome::Cache(loaded) => {
                self.cache().insert(
                    entry.path.clone(),
                    Cached {
                        stamp: entry.stamp,
                        loaded: loaded.clone(),
                    },
                );
                Ok(Some(loaded))
            }
            ReadOutcome::NoCache(loaded) => {
                self.cache().remove(&entry.path);
                Ok(Some(loaded))
            }
            ReadOutcome::Gone => {
                self.cache().remove(&entry.path);
                Ok(None)
            }
            ReadOutcome::KeyUnavailable => Err(StoreError::KeyUnavailable),
        }
    }

    fn recover(&self, entry: &Entry, wrapper: &dyn SecretWrapper, now_us: u64) -> RecoverReport {
        let id = entry.id.clone();
        // 创建时崩溃，连文件头都没写全。
        if entry.stamp.len < HEADER_FIXED_LEN as u64 {
            return remove_empty(id, &entry.path);
        }
        let tally = RefCell::new(RecordingEnd::default());
        let outcome = file::recover_part(
            &entry.path,
            wrapper,
            &mut |_, plaintext| tally_chunk(&mut tally.borrow_mut(), plaintext),
            &mut |kind| recovered_end(&tally.borrow(), kind, now_us),
        );
        self.cache().remove(&self.final_path(&id));
        let frames = tally.into_inner().frames;
        match outcome {
            // 一个关键帧都没录到：没有画面的录制无法回放。
            Ok(outcome) if frames == 0 => remove_empty(id, &outcome.path),
            Ok(outcome) => RecoverReport::Finished {
                id,
                kind: outcome.kind,
            },
            Err(error) => {
                tracing::warn!(%id, %error, "recording recovery skipped");
                RecoverReport::Skipped {
                    id,
                    reason: error.to_string(),
                }
            }
        }
    }
}

fn parse_name(name: &str) -> Option<(String, bool)> {
    let (stem, part) = match name.strip_suffix(PART_SUFFIX) {
        Some(stem) => (stem, true),
        None => (name, false),
    };
    let id = stem.strip_suffix(EXTENSION)?;
    RecordingStore::valid_id(id).then(|| (id.to_owned(), part))
}

fn read_recording(entry: &Entry, wrapper: &dyn SecretWrapper) -> ReadOutcome {
    let mut reader = match RecordingReader::open(&entry.path, wrapper) {
        Ok(reader) => reader,
        Err(error) => return damaged(entry, error, None, None),
    };
    let created_at_us = reader.header().created_at_us;
    let meta = match read_meta(&mut reader) {
        Ok(meta) => meta,
        Err(error) => return damaged(entry, error, Some(created_at_us), None),
    };
    let end = match read_end(&mut reader) {
        Ok(end) => end,
        Err(error) => return damaged(entry, error, Some(created_at_us), Some(meta)),
    };
    let state = match end.reason.as_str() {
        REASON_RECOVERED => StoredState::Recovered,
        REASON_DAMAGED => StoredState::Damaged,
        _ => StoredState::Complete,
    };
    let has_audio = meta.audio && end.audio_packets > 0;
    ReadOutcome::Cache(Loaded {
        retain_from: Some(meta.started_at_us),
        recording: StoredRecording {
            id: entry.id.clone(),
            meta,
            end,
            bytes: entry.stamp.len,
            state,
            has_audio,
        },
    })
}

fn read_meta(reader: &mut RecordingReader) -> Result<RecordingMeta, FileError> {
    if reader.chunks().is_empty() {
        return Err(FileError::Corrupt("missing metadata"));
    }
    let plaintext = reader.read_chunk(0)?;
    record::iter(&plaintext)
        .flatten()
        .find_map(|record| match record.body {
            Body::Meta(meta) => Some(meta),
            _ => None,
        })
        .ok_or(FileError::Corrupt("missing metadata"))
}

fn read_end(reader: &mut RecordingReader) -> Result<RecordingEnd, FileError> {
    let plaintext = reader.read_final()?;
    record::iter(&plaintext)
        .flatten()
        .find_map(|record| match record.body {
            Body::End(end) => Some(end),
            _ => None,
        })
        .ok_or(FileError::Corrupt("missing end record"))
}

/// 单份文件读不出来：照样列出，state = Damaged。Meta 读出来了就保留，否则用占位值。
fn damaged(
    entry: &Entry,
    error: FileError,
    created_at_us: Option<u64>,
    meta: Option<RecordingMeta>,
) -> ReadOutcome {
    match &error {
        FileError::Wrap(WrapError::Unavailable) => return ReadOutcome::KeyUnavailable,
        FileError::Io(source) if source.kind() == io::ErrorKind::NotFound => {
            return ReadOutcome::Gone
        }
        _ => {}
    }
    tracing::warn!(id = %entry.id, %error, "recording is damaged");
    let created_at_us = created_at_us.or_else(|| {
        file::read_header(&entry.path)
            .ok()
            .map(|header| header.created_at_us)
    });
    // 以后的版本可能读得了，不按保留期删除。
    let retain_from = match error {
        FileError::UnsupportedVersion(_) => None,
        _ => meta
            .as_ref()
            .map(|meta| meta.started_at_us)
            .or(created_at_us),
    };
    let meta = meta.unwrap_or_else(|| RecordingMeta {
        host_id: String::new(),
        host_name: String::new(),
        role: String::new(),
        display_id: None,
        started_at_us: created_at_us.unwrap_or_else(|| modified_us(entry.stamp)),
        app_version: String::new(),
        audio: false,
    });
    let loaded = Loaded {
        retain_from,
        recording: StoredRecording {
            id: entry.id.clone(),
            meta,
            end: RecordingEnd {
                reason: REASON_DAMAGED.to_owned(),
                ..RecordingEnd::default()
            },
            bytes: entry.stamp.len,
            state: StoredState::Damaged,
            has_audio: false,
        },
    };
    if matches!(error, FileError::Io(_)) {
        ReadOutcome::NoCache(loaded)
    } else {
        ReadOutcome::Cache(loaded)
    }
}

/// 连文件头都读不出时，列表里的开始时间退回到文件的 mtime。
fn modified_us(stamp: Stamp) -> u64 {
    stamp
        .modified
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
        })
}

fn tally_chunk(end: &mut RecordingEnd, plaintext: &[u8]) {
    end.plain_bytes += plaintext.len() as u64;
    for record in record::iter(plaintext).flatten() {
        end.duration_us = end.duration_us.max(record.t_us);
        match record.body {
            Body::Config(config) => {
                end.width = config.width;
                end.height = config.height;
                end.codec = config.codec;
            }
            Body::Video(video) => {
                end.frames += 1;
                end.keyframes += u64::from(video.keyframe);
            }
            Body::Audio(_) => end.audio_packets += 1,
            Body::Gap { .. } => end.gaps += 1,
            _ => {}
        }
    }
}

/// 补写的 FINAL 块：明文是一条 End，`t_start_us` 和 `duration_us` 都取最大的 `t_us`。
fn recovered_end(tally: &RecordingEnd, kind: RecoverKind, now_us: u64) -> (u64, Vec<u8>) {
    let reason = match kind {
        RecoverKind::Damaged => REASON_DAMAGED,
        RecoverKind::Recovered | RecoverKind::Complete => REASON_RECOVERED,
    };
    let record = Record {
        t_us: tally.duration_us,
        body: Body::End(RecordingEnd {
            ended_at_us: now_us,
            reason: reason.to_owned(),
            ..tally.clone()
        }),
    };
    let mut plaintext = Vec::new();
    record.encode_into(&mut plaintext);
    (record.t_us, plaintext)
}

fn remove_empty(id: String, path: &Path) -> RecoverReport {
    match fs::remove_file(path) {
        Ok(()) => RecoverReport::RemovedEmpty { id },
        Err(error) => RecoverReport::Skipped {
            id,
            reason: error.to_string(),
        },
    }
}
