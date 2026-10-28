//! 录制写盘（M3 V5，07「会话录制」）。`Ingress` 在会话泵和网络线程上把事件换算成记录，
//! 非阻塞地送进通道；写盘线程 `rs-recording` 分块、加密写入、定时 fsync、每秒发
//! `recording.state` 并检查停止条件，最后收尾改名。
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
        Arc, Mutex, MutexGuard, Weak,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use data_encoding::HEXLOWER;
use rs_recording::{
    chunk::ChunkBuilder,
    crypto::{SecretWrapper, WrapError},
    file::{finalize_path, FileError, RecordingWriter},
    record::{
        AudioRec, Body, ConfigRec, CursorShapeRec, GapReason, Record, RecordingEnd, RecordingMeta,
        VideoRec,
    },
    store::{RecordingStore, SpaceProbe, StartRefusal, StopReason, StoreConfig},
    timeline::MediaClock,
};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::pb;

use super::{
    dto::{RecordingRunState, RecordingStateEvent, RecordingStatus, RecordingStatusState},
    ffi_wrap, notice,
    slot::RecorderSlot,
};
use crate::rscore::{
    api_sessions::settings_snapshot,
    dto_core::{id_str, CoreEvent, Role, SessionPhase, SessionSummary},
    error::CoreError,
    runtime::Core,
    tables::SessionEntry,
};

/// fsync 的间隔：崩溃时最多丢这么久。
pub const SYNC_EVERY: Duration = Duration::from_secs(5);
/// `recording.state` 事件和停止条件检查的间隔。
pub const STATE_EVERY: Duration = Duration::from_secs(1);
/// 周期关键帧：回放的跳转点。
pub const KEYFRAME_EVERY: Duration = Duration::from_secs(30);
/// 通道里排队的字节上限。超出时视频跳到下一个关键帧，声音直接丢。
pub const QUEUE_BUDGET: usize = 32 << 20;

const CHANNEL_CAPACITY: usize = 1024;
const POLL_EVERY: Duration = Duration::from_millis(250);
/// 同一个 Ingress 因背压请求关键帧的最小间隔。
const KEYFRAME_RETRY_US: u64 = 2_000_000;
/// 通道满时暂存的光标消息和 Gap 最多这么多条，超出丢最旧的。
const MAX_PENDING: usize = 16;

const REASON_USER: &str = "user";
pub(crate) const REASON_SESSION_ENDED: &str = "sessionEnded";
const REASON_ERROR: &str = "error";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn system_now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn now_us(core: &Weak<Core>) -> u64 {
    core.upgrade()
        .map_or_else(system_now_us, |core| core.now_us())
}

fn http(status: u16, code: &str) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: code.to_owned(),
    }
}

fn io_error(error: impl Display) -> CoreError {
    CoreError::internal(format!("recording.io: {error}"))
}

/// Default = 生产值。测试用 start_recording_with 注入。
pub struct RecorderTuning {
    pub sync_every: Duration,
    pub state_every: Duration,
    pub keyframe_every: Duration,
    pub queue_budget: usize,
    /// None = 取自 Settings。
    pub store_config: Option<StoreConfig>,
    /// None = StatvfsProbe。
    pub probe: Option<Arc<dyn SpaceProbe>>,
    /// 测试钩子：写够 N 块后线程直接返回，不收尾。
    pub crash_after_chunks: Option<u32>,
}

impl Default for RecorderTuning {
    fn default() -> Self {
        Self {
            sync_every: SYNC_EVERY,
            state_every: STATE_EVERY,
            keyframe_every: KEYFRAME_EVERY,
            queue_budget: QUEUE_BUDGET,
            store_config: None,
            probe: None,
            crash_after_chunks: None,
        }
    }
}

/// `f_bavail * f_frsize`；查不到返回 None。
pub struct StatvfsProbe;

impl SpaceProbe for StatvfsProbe {
    #[cfg(unix)]
    // statvfs 的字段宽度随平台不同（Linux 是 u64，Apple 有 u32）。
    #[allow(clippy::unnecessary_cast)]
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};

        let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: path 以 NUL 结尾，stat 指向一块可写的 statvfs。
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: statvfs 返回 0 时已经填满 stat。
        let stat = unsafe { stat.assume_init() };
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }

    #[cfg(not(unix))]
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        None
    }
}

fn recordings_dir(core: &Core) -> PathBuf {
    core.data_dir().join("recordings")
}

/// `<dataDir>/recordings`，可用空间用 StatvfsProbe 查。
pub fn store(core: &Arc<Core>) -> Result<RecordingStore, CoreError> {
    RecordingStore::open(recordings_dir(core), Arc::new(StatvfsProbe)).map_err(io_error)
}

/// 设置项 `recording.maxTotalMb`、`recording.retentionDays`（C12），其余取默认值。
pub fn store_config(core: &Arc<Core>) -> StoreConfig {
    let settings = settings_snapshot(core);
    StoreConfig::from_settings(
        settings.recording_max_total_mb,
        settings.recording_retention_days,
    )
}

/// 正在录的份（core.state::<ActiveRecordings>）。 用它在列表里补上 state = recording 的条目。
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveRecording {
    pub recording_id: String,
    pub session_id: u64,
    pub meta: RecordingMeta,
    pub bytes: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
pub struct ActiveRecordings(Mutex<HashMap<String, ActiveRecording>>);

impl ActiveRecordings {
    fn insert(&self, recording: ActiveRecording) {
        lock(&self.0).insert(recording.recording_id.clone(), recording);
    }

    fn update(&self, recording_id: &str, f: impl FnOnce(&mut ActiveRecording)) {
        if let Some(recording) = lock(&self.0).get_mut(recording_id) {
            f(recording);
        }
    }

    fn remove(&self, recording_id: &str) {
        lock(&self.0).remove(recording_id);
    }
}

/// 按开始时间从新到旧，和 RecordingStore::list 一致。
pub fn active_recordings(core: &Core) -> Vec<ActiveRecording> {
    let mut recordings: Vec<_> = lock(&core.state::<ActiveRecordings>().0)
        .values()
        .cloned()
        .collect();
    recordings.sort_by(|a, b| {
        b.meta
            .started_at_us
            .cmp(&a.meta.started_at_us)
            .then_with(|| a.recording_id.cmp(&b.recording_id))
    });
    recordings
}

pub fn active_ids(core: &Core) -> HashSet<String> {
    lock(&core.state::<ActiveRecordings>().0)
        .keys()
        .cloned()
        .collect()
}

/// 阻塞函数，调用方放在 spawn_blocking 里。
pub fn start_recording(
    core: &Arc<Core>,
    entry: &Arc<SessionEntry>,
) -> Result<RecordingStatus, CoreError> {
    start_recording_with(core, entry, RecorderTuning::default())
}

pub fn start_recording_with(
    core: &Arc<Core>,
    entry: &Arc<SessionEntry>,
    tuning: RecorderTuning,
) -> Result<RecordingStatus, CoreError> {
    let wrapper =
        ffi_wrap::wrapper().ok_or_else(|| CoreError::unsupported("recording.unsupported"))?;
    let summary = entry.summary();
    if summary.phase != SessionPhase::Streaming {
        return Err(http(409, "recording.notStreaming"));
    }
    if !entry.recorder.claim() {
        return Err(http(409, "recording.alreadyRecording"));
    }
    begin(core, entry, tuning, wrapper.as_ref(), summary).inspect_err(|_| entry.recorder.release())
}

fn begin(
    core: &Arc<Core>,
    entry: &Arc<SessionEntry>,
    tuning: RecorderTuning,
    wrapper: &dyn SecretWrapper,
    summary: SessionSummary,
) -> Result<RecordingStatus, CoreError> {
    let cfg = tuning.store_config.unwrap_or_else(|| store_config(core));
    let probe = tuning.probe.unwrap_or_else(|| Arc::new(StatvfsProbe));
    let store = RecordingStore::open(recordings_dir(core), probe.clone()).map_err(io_error)?;
    let active = active_ids(core);
    let now = core.now_us();
    store.recover_all(wrapper, &active, now);
    store.apply_retention(&cfg, wrapper, now, &active);
    store
        .check_can_start(&cfg)
        .map_err(|refusal| match refusal {
            StartRefusal::StorageFull => http(507, "recording.storageFull"),
            StartRefusal::DiskLow => http(507, "recording.diskLow"),
        })?;

    let recording_id = RecordingStore::new_id().map_err(io_error)?;
    let file_id = HEXLOWER
        .decode(recording_id.as_bytes())
        .ok()
        .and_then(|bytes| <[u8; 16]>::try_from(bytes).ok())
        .ok_or_else(|| io_error("invalid recording id"))?;
    let part = store.part_path(&recording_id);
    let file =
        RecordingWriter::create(&part, wrapper, file_id, now).map_err(|error| match error {
            FileError::Wrap(WrapError::Unavailable) => {
                CoreError::internal("recording.keyUnavailable")
            }
            error => io_error(error),
        })?;
    let used_others = store.used_bytes().saturating_sub(file.bytes_written());
    let meta = RecordingMeta {
        host_id: summary.host_id,
        host_name: summary.host_name,
        role: match summary.role {
            Some(Role::Controller) => "controller",
            _ => "viewer",
        }
        .to_owned(),
        display_id: summary.display_id,
        started_at_us: core.now_us(),
        app_version: core.config().app_version,
        audio: entry.opts.audio != Some(false),
    };

    let control = Arc::new(Control::new(
        recording_id.clone(),
        entry.id,
        Arc::downgrade(entry),
        file.bytes_written(),
    ));
    let actives = core.state::<ActiveRecordings>();
    actives.insert(ActiveRecording {
        recording_id: recording_id.clone(),
        session_id: entry.id,
        meta: meta.clone(),
        bytes: file.bytes_written(),
        duration_ms: 0,
        width: 0,
        height: 0,
    });
    let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
    let queued = Arc::new(AtomicUsize::new(0));
    let writer = Writer {
        link: Link {
            core: Arc::downgrade(core),
            entry: Arc::downgrade(entry),
            slot: entry.recorder.share(),
            control: control.clone(),
            actives: actives.clone(),
        },
        rx,
        queued: queued.clone(),
        builder: ChunkBuilder::new(meta),
        file,
        dir: store.dir().to_path_buf(),
        cfg,
        probe,
        used_others,
        sync_every: tuning.sync_every,
        state_every: tuning.state_every,
        keyframe_every: tuning.keyframe_every,
        crash_after_chunks: tuning.crash_after_chunks,
    };
    let weak_core = Arc::downgrade(core);
    let ingress = Ingress::new(
        tx,
        queued,
        tuning.queue_budget,
        keyframe_requester(entry),
        Box::new(move || now_us(&weak_core)),
    );
    entry.recorder.install(ingress, control.clone());
    let spawned = std::thread::Builder::new()
        .name("rs-recording".into())
        .spawn(move || writer.run());
    if let Err(error) = spawned {
        drop(entry.recorder.uninstall());
        actives.remove(&recording_id);
        if let Err(error) = std::fs::remove_file(&part) {
            tracing::warn!(%error, "could not remove unstarted recording");
        }
        return Err(io_error(error));
    }
    request_keyframe(entry);
    notice::sync(entry, true);
    Ok(control.status())
}

/// 请求停止；收尾在写盘线程完成，结果以 `recording.state` 事件给出。没在录 → Ok（幂等）。
pub fn stop_recording(entry: &SessionEntry) -> Result<(), CoreError> {
    if let Some(control) = entry.recorder.control() {
        control.request_stop(REASON_USER);
    }
    Ok(())
}

pub fn status(entry: &SessionEntry) -> RecordingStatus {
    entry.recorder.control().map_or_else(
        || RecordingStatus {
            session_id: id_str(entry.id),
            recording_id: None,
            state: RecordingStatusState::Idle,
            bytes: 0,
            duration_ms: 0,
        },
        |control| control.status(),
    )
}

fn request_keyframe(entry: &SessionEntry) {
    if let Some(handle) = entry.handle() {
        handle.request_keyframe_for(pb::keyframe_request::Reason::DecoderReset);
    }
}

fn keyframe_requester(entry: &Arc<SessionEntry>) -> Box<dyn Fn() + Send> {
    let entry = Arc::downgrade(entry);
    Box::new(move || {
        if let Some(entry) = entry.upgrade() {
            request_keyframe(&entry);
        }
    })
}

/// 一份录制的控制面。停止请求不经过数据通道，通道满了也能停下；统计供 status 读。
pub(crate) struct Control {
    recording_id: String,
    session_id: u64,
    entry: Weak<SessionEntry>,
    stop: AtomicBool,
    reason: Mutex<Option<String>>,
    bytes: AtomicU64,
    duration_ms: AtomicU64,
}

impl Control {
    pub(crate) fn new(
        recording_id: String,
        session_id: u64,
        entry: Weak<SessionEntry>,
        bytes: u64,
    ) -> Self {
        Self {
            recording_id,
            session_id,
            entry,
            stop: AtomicBool::new(false),
            reason: Mutex::new(None),
            bytes: AtomicU64::new(bytes),
            duration_ms: AtomicU64::new(0),
        }
    }

    /// 第一次请求的原因生效。
    pub(crate) fn request_stop(&self, reason: &str) {
        lock(&self.reason).get_or_insert_with(|| reason.to_owned());
        self.stop.store(true, Ordering::Release);
    }

    fn stop_reason(&self) -> Option<String> {
        if !self.stop.load(Ordering::Acquire) {
            return None;
        }
        lock(&self.reason).clone()
    }

    /// 录制告知 Host（C5）。
    pub(crate) fn notice(&self, recording: bool) {
        if let Some(entry) = self.entry.upgrade() {
            notice::sync(&entry, recording);
        }
    }

    pub(crate) fn status(&self) -> RecordingStatus {
        RecordingStatus {
            session_id: id_str(self.session_id),
            recording_id: Some(self.recording_id.clone()),
            state: if self.stop.load(Ordering::Acquire) {
                RecordingStatusState::Stopping
            } else {
                RecordingStatusState::Recording
            },
            bytes: self.bytes.load(Ordering::Relaxed),
            duration_ms: self.duration_ms.load(Ordering::Relaxed),
        }
    }
}

/// 把会话事件换算成记录，非阻塞地送进写盘通道。只在 RecorderSlot 的锁里调用。
pub(crate) struct Ingress {
    clock: MediaClock,
    tx: SyncSender<Record>,
    /// 通道里排队的字节：发送时加，写盘线程取出时减。
    queued: Arc<AtomicUsize>,
    budget: usize,
    /// 背压丢过视频：之后的增量帧一律丢弃，直到下一个关键帧。
    skipping: bool,
    skipped_since_us: u64,
    dropped_video: u64,
    dropped_audio: u64,
    last_keyframe_request_us: Option<u64>,
    pending_config: Option<Record>,
    pending_cursor_pos: Option<Record>,
    /// 光标消息和 Gap，按到达顺序。
    pending: VecDeque<Record>,
    request_keyframe: Box<dyn Fn() + Send>,
    now_us: Box<dyn Fn() -> u64 + Send>,
}

impl Ingress {
    pub(crate) fn new(
        tx: SyncSender<Record>,
        queued: Arc<AtomicUsize>,
        budget: usize,
        request_keyframe: Box<dyn Fn() + Send>,
        now_us: Box<dyn Fn() -> u64 + Send>,
    ) -> Self {
        Self {
            clock: MediaClock::new(),
            tx,
            queued,
            budget,
            skipping: false,
            skipped_since_us: 0,
            dropped_video: 0,
            dropped_audio: 0,
            last_keyframe_request_us: None,
            pending_config: None,
            pending_cursor_pos: None,
            pending: VecDeque::new(),
            request_keyframe,
            now_us,
        }
    }

    pub(crate) fn frame(&mut self, received: &ReceivedFrame) {
        let real_us = received.received_us;
        let t_us = self.map(real_us);
        let frame = &received.frame;
        if self.skipping && !frame.keyframe {
            self.drop_video(real_us);
            return;
        }
        let record = Record {
            t_us,
            body: Body::Video(VideoRec {
                frame_id: frame.frame_id,
                epoch: frame.epoch,
                display_id: frame.display_id,
                flags: frame.flags,
                keyframe: frame.keyframe,
                capture_ts_us: frame.capture_ts_us,
                capture_local_us: received.capture_local_us,
                data: frame.data.clone(),
            }),
        };
        if !self.flush_pending() || !self.fits(&record) {
            self.drop_video(real_us);
            return;
        }
        if self.skipping {
            // 跳过结束：先记下跳过的时长，再写这个关键帧。
            let gap = Record {
                t_us,
                body: Body::Gap {
                    reason: GapReason::Backpressure,
                    real_us: real_us.saturating_sub(self.skipped_since_us),
                },
            };
            if self.send(gap).is_some() {
                self.drop_video(real_us);
                return;
            }
            self.skipped_since_us = real_us;
        }
        match self.send(record) {
            None => self.skipping = false,
            Some(_) => self.drop_video(real_us),
        }
    }

    pub(crate) fn config(&mut self, config: &pb::StreamConfig) {
        let t_us = self.map((self.now_us)());
        self.send_state(Record {
            t_us,
            body: Body::Config(ConfigRec {
                epoch: config.epoch,
                display_id: config.display_id,
                codec: config.codec as u8,
                width: config.width,
                height: config.height,
                fps: config.fps,
                bitrate_kbps: config.bitrate_kbps,
                cursor_x: config.cursor_x,
                cursor_y: config.cursor_y,
                cursor_visible: config.cursor_visible,
            }),
        });
    }

    pub(crate) fn cursor(&mut self, cursor: &CursorEvent) {
        let t_us = self.map((self.now_us)());
        self.send_state(Record {
            t_us,
            body: Body::CursorPos {
                x: cursor.x,
                y: cursor.y,
                visible: cursor.visible,
            },
        });
    }

    pub(crate) fn cursor_msg(&mut self, message: &pb::CursorMsg) {
        let body = match &message.msg {
            Some(pb::cursor_msg::Msg::Shape(shape)) => Body::CursorShape(CursorShapeRec {
                id: shape.id,
                width: shape.width,
                height: shape.height,
                hot_x: shape.hot_x,
                hot_y: shape.hot_y,
                scale: shape.scale,
                png: shape.png.clone(),
            }),
            Some(pb::cursor_msg::Msg::Use(id)) => Body::CursorUse(*id),
            Some(pb::cursor_msg::Msg::Hidden(hidden)) => Body::CursorHidden(*hidden),
            None => return,
        };
        let t_us = self.map((self.now_us)());
        self.send_state(Record { t_us, body });
    }

    pub(crate) fn audio(&mut self, seq: u16, capture_ts_us: u64, payload: &[u8]) {
        let t_us = self.map((self.now_us)());
        let record = Record {
            t_us,
            body: Body::Audio(AudioRec {
                seq,
                capture_ts_us,
                payload: payload.to_vec(),
            }),
        };
        if !self.flush_pending() || !self.fits(&record) || self.send(record).is_some() {
            self.dropped_audio += 1;
        }
    }

    /// 下一个空档记为 Paused（会话进后台）。
    pub(crate) fn mark_paused(&mut self) {
        self.clock.mark_paused();
    }

    /// （视频, 声音）因背压丢掉的条数。
    pub(crate) fn dropped(&self) -> (u64, u64) {
        (self.dropped_video, self.dropped_audio)
    }

    /// 还没送出的状态记录；收尾时由写盘线程直接写入。
    pub(crate) fn take_pending(&mut self) -> Vec<Record> {
        self.pending_config
            .take()
            .into_iter()
            .chain(self.pending_cursor_pos.take())
            .chain(self.pending.drain(..))
            .collect()
    }

    fn map(&mut self, real_us: u64) -> u64 {
        let (t_us, gap) = self.clock.map(real_us);
        if let Some(gap) = gap {
            self.send_state(Record {
                t_us,
                body: Body::Gap {
                    reason: gap.reason,
                    real_us: gap.real_us,
                },
            });
        }
        t_us
    }

    fn fits(&self, record: &Record) -> bool {
        self.queued
            .load(Ordering::Acquire)
            .saturating_add(record.encoded_len())
            <= self.budget
    }

    fn drop_video(&mut self, real_us: u64) {
        self.dropped_video += 1;
        if !self.skipping {
            self.skipping = true;
            self.skipped_since_us = real_us;
        }
        let due = self
            .last_keyframe_request_us
            .is_none_or(|last| real_us.saturating_sub(last) >= KEYFRAME_RETRY_US);
        if due {
            self.last_keyframe_request_us = Some(real_us);
            (self.request_keyframe)();
        }
    }

    /// 非阻塞地送一条记录；送不进去（通道满，或写盘线程已经不在）时把它还回来。
    fn send(&mut self, record: Record) -> Option<Record> {
        let bytes = record.encoded_len();
        // 先加后发：写盘线程取出后才减，计数不会倒过来。
        self.queued.fetch_add(bytes, Ordering::AcqRel);
        match self.tx.try_send(record) {
            Ok(()) => None,
            Err(TrySendError::Full(record) | TrySendError::Disconnected(record)) => {
                self.queued.fetch_sub(bytes, Ordering::AcqRel);
                Some(record)
            }
        }
    }

    /// 状态类记录不按预算丢弃；通道满时暂存，下次发送任何消息之前先补发。
    fn send_state(&mut self, record: Record) {
        let record = if self.flush_pending() {
            match self.send(record) {
                None => return,
                Some(record) => record,
            }
        } else {
            record
        };
        match record.body {
            Body::Config(_) => self.pending_config = Some(record),
            Body::CursorPos { .. } => self.pending_cursor_pos = Some(record),
            _ => {
                if self.pending.len() == MAX_PENDING {
                    self.pending.pop_front();
                }
                self.pending.push_back(record);
            }
        }
    }

    /// 补发暂存的状态记录；通道又满了返回 false。
    fn flush_pending(&mut self) -> bool {
        if let Some(record) = self.pending_config.take() {
            if let Some(record) = self.send(record) {
                self.pending_config = Some(record);
                return false;
            }
        }
        if let Some(record) = self.pending_cursor_pos.take() {
            if let Some(record) = self.send(record) {
                self.pending_cursor_pos = Some(record);
                return false;
            }
        }
        while let Some(record) = self.pending.pop_front() {
            if let Some(record) = self.send(record) {
                self.pending.push_front(record);
                return false;
            }
        }
        true
    }
}

/// 写盘线程和会话、Core 的联系。SessionEntry 和 Core 只留 Weak，避免循环引用让 Core 无法释放。
struct Link {
    core: Weak<Core>,
    entry: Weak<SessionEntry>,
    slot: RecorderSlot,
    control: Arc<Control>,
    actives: Arc<ActiveRecordings>,
}

impl Link {
    fn emit(&self, state: RecordingRunState, bytes: u64, duration_ms: u64, reason: Option<String>) {
        if let Some(core) = self.core.upgrade() {
            core.events()
                .emit(&CoreEvent::RecordingState(RecordingStateEvent {
                    session_id: id_str(self.control.session_id),
                    recording_id: self.control.recording_id.clone(),
                    state,
                    bytes,
                    duration_ms,
                    reason,
                }));
        }
    }

    /// 收尾的最后一步：注销登记、告知 Host、放开录制位，再发结束事件。
    /// 放开在发事件之前，收到 stopped 之后 status 就是 idle，也可以马上再开始录。
    fn conclude(&self, state: RecordingRunState, reason: &str, bytes: u64, duration_ms: u64) {
        self.actives.remove(&self.control.recording_id);
        self.control.notice(false);
        self.slot.release();
        self.emit(state, bytes, duration_ms, Some(reason.to_owned()));
    }
}

struct Writer {
    link: Link,
    rx: Receiver<Record>,
    queued: Arc<AtomicUsize>,
    builder: ChunkBuilder,
    file: RecordingWriter,
    dir: PathBuf,
    cfg: StoreConfig,
    probe: Arc<dyn SpaceProbe>,
    used_others: u64,
    sync_every: Duration,
    state_every: Duration,
    keyframe_every: Duration,
    crash_after_chunks: Option<u32>,
}

impl Writer {
    fn run(mut self) {
        let started = Instant::now();
        let (mut synced, mut reported, mut keyed) = (started, started, started);
        let ended = loop {
            match self.rx.recv_timeout(POLL_EVERY) {
                Ok(record) => {
                    if let Err(error) = self.take(record) {
                        break Err(error);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // Ingress 已经不在挂点上了：没有人还能再送消息。
                Err(RecvTimeoutError::Disconnected) => break Ok(REASON_SESSION_ENDED.to_owned()),
            }
            if self
                .crash_after_chunks
                .is_some_and(|chunks| self.file.chunks_written() >= u64::from(chunks))
            {
                return self.crash();
            }
            if let Some(reason) = self.link.control.stop_reason() {
                break Ok(reason);
            }
            // 会话泵收到 Ended 时只关 VideoTap、不经过 finish_open，这里一并按会话结束处理。
            if self
                .link
                .entry
                .upgrade()
                .is_none_or(|entry| entry.video.is_closed())
            {
                break Ok(REASON_SESSION_ENDED.to_owned());
            }
            let now = Instant::now();
            if now.duration_since(synced) >= self.sync_every {
                synced = now;
                if let Err(error) = self.file.sync() {
                    break Err(error);
                }
            }
            if now.duration_since(reported) >= self.state_every {
                reported = now;
                if let Some(reason) = self.report() {
                    break Ok(reason.to_owned());
                }
            }
            if now.duration_since(keyed) >= self.keyframe_every {
                keyed = now;
                if let Some(entry) = self.link.entry.upgrade() {
                    request_keyframe(&entry);
                }
            }
        };
        match ended {
            Ok(reason) => self.finish(reason),
            Err(error) => self.fail(&error),
        }
    }

    fn take(&mut self, record: Record) -> Result<(), FileError> {
        self.queued
            .fetch_sub(record.encoded_len(), Ordering::AcqRel);
        self.append(record)
    }

    fn append(&mut self, record: Record) -> Result<(), FileError> {
        for chunk in self.builder.push(record) {
            self.file
                .append(chunk.t_start_us, chunk.keyframe_start, &chunk.plaintext)?;
        }
        Ok(())
    }

    /// 更新统计、发 recording.state，并检查停止条件；该停时返回 End reason。
    fn report(&self) -> Option<&'static str> {
        let bytes = self.file.bytes_written();
        let stats = self.builder.stats();
        let free = self.probe.free_bytes(&self.dir);
        let stop =
            RecordingStore::should_stop(&self.cfg, self.used_others, bytes, stats.last_t_us, free);
        if let Some(stop) = stop {
            return Some(match stop {
                StopReason::MaxDuration => "maxDuration",
                StopReason::StorageFull => "storageFull",
                StopReason::DiskLow => "diskLow",
            });
        }
        let duration_ms = stats.last_t_us / 1000;
        let control = &self.link.control;
        control.bytes.store(bytes, Ordering::Relaxed);
        control.duration_ms.store(duration_ms, Ordering::Relaxed);
        self.link.actives.update(&control.recording_id, |active| {
            active.bytes = bytes;
            active.duration_ms = duration_ms;
            active.width = stats.width;
            active.height = stats.height;
        });
        self.link
            .emit(RecordingRunState::Recording, bytes, duration_ms, None);
        None
    }

    fn finish(mut self, reason: String) {
        // 卸下 Ingress 之后不会再有新消息；读完通道里剩下的，再补上它暂存的状态记录。
        let ingress = self.link.slot.uninstall();
        if let Err(error) = self.drain(ingress) {
            return self.fail(&error);
        }
        let Writer {
            link,
            builder,
            file,
            ..
        } = self;
        let stats = builder.stats().clone();
        let duration_ms = stats.last_t_us / 1000;
        if stats.frames == 0 {
            // 一个关键帧都没录到：没有画面的录制无法回放，和恢复 .part 时一样直接删掉。
            let part = file.path().to_path_buf();
            drop(file);
            if let Err(error) = std::fs::remove_file(&part) {
                tracing::warn!(%error, "could not remove empty recording");
            }
            link.conclude(RecordingRunState::Stopped, &reason, 0, 0);
            return;
        }
        let bytes = file.bytes_written();
        let end = RecordingEnd {
            ended_at_us: now_us(&link.core),
            reason: reason.clone(),
            ..RecordingEnd::default()
        };
        match close_file(builder, file, end) {
            Ok(bytes) => link.conclude(RecordingRunState::Stopped, &reason, bytes, duration_ms),
            Err(error) => {
                tracing::warn!(
                    recording_id = %link.control.recording_id,
                    %error,
                    "recording finish failed; keeping .part for recovery"
                );
                link.conclude(RecordingRunState::Failed, REASON_ERROR, bytes, duration_ms);
            }
        }
    }

    fn drain(&mut self, ingress: Option<Ingress>) -> Result<(), FileError> {
        while let Ok(record) = self.rx.try_recv() {
            self.take(record)?;
        }
        let Some(mut ingress) = ingress else {
            return Ok(());
        };
        let (video, audio) = ingress.dropped();
        if video + audio > 0 {
            tracing::info!(
                recording_id = %self.link.control.recording_id,
                video,
                audio,
                "recording dropped media under backpressure"
            );
        }
        // 暂存的记录比通道里的都新：暂存之后的发送一定先把它们补发了。
        for record in ingress.take_pending() {
            self.append(record)?;
        }
        Ok(())
    }

    /// 写入出错：不再写，`.part` 原样保留，下次列表时恢复。
    fn fail(self, error: &FileError) {
        tracing::warn!(
            recording_id = %self.link.control.recording_id,
            %error,
            "recording write failed; keeping .part for recovery"
        );
        drop(self.link.slot.uninstall());
        let duration_ms = self.builder.stats().last_t_us / 1000;
        self.link.conclude(
            RecordingRunState::Failed,
            REASON_ERROR,
            self.file.bytes_written(),
            duration_ms,
        );
    }

    /// 测试钩子：模拟进程崩溃，文件不收尾、不发事件。只清掉内存里的登记，同一进程里的后续步骤就像重启之后一样。
    fn crash(self) {
        tracing::warn!(
            recording_id = %self.link.control.recording_id,
            "recording crash hook: leaving .part unfinished"
        );
        drop(self.link.slot.uninstall());
        self.link.actives.remove(&self.link.control.recording_id);
        self.link.slot.release();
    }
}

/// 写最后一个普通块、FINAL 块和文件尾，再把 `.part` 改名。返回文件字节数。
fn close_file(
    builder: ChunkBuilder,
    mut file: RecordingWriter,
    end: RecordingEnd,
) -> Result<u64, FileError> {
    let (pending, final_chunk) = builder.finish(end);
    if let Some(chunk) = pending {
        file.append(chunk.t_start_us, chunk.keyframe_start, &chunk.plaintext)?;
    }
    let part = file.path().to_path_buf();
    let bytes = file.finish(final_chunk.t_start_us, &final_chunk.plaintext)?;
    finalize_path(&part)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use rsp_core::video::Frame;

    use super::*;

    struct Harness {
        ingress: Ingress,
        rx: Receiver<Record>,
        queued: Arc<AtomicUsize>,
        now: Arc<AtomicU64>,
        keyframes: Arc<AtomicUsize>,
    }

    fn harness(budget: usize) -> Harness {
        let (tx, rx) = mpsc::sync_channel(4);
        let queued = Arc::new(AtomicUsize::new(0));
        let now = Arc::new(AtomicU64::new(1_000_000));
        let keyframes = Arc::new(AtomicUsize::new(0));
        let (clock, requests) = (now.clone(), keyframes.clone());
        Harness {
            ingress: Ingress::new(
                tx,
                queued.clone(),
                budget,
                Box::new(move || {
                    requests.fetch_add(1, Ordering::Relaxed);
                }),
                Box::new(move || clock.load(Ordering::Relaxed)),
            ),
            rx,
            queued,
            now,
            keyframes,
        }
    }

    impl Harness {
        /// 像写盘线程那样取出通道里的全部记录。
        fn drain(&self) -> Vec<Record> {
            self.rx
                .try_iter()
                .inspect(|record| {
                    self.queued
                        .fetch_sub(record.encoded_len(), Ordering::AcqRel);
                })
                .collect()
        }

        fn keyframes(&self) -> usize {
            self.keyframes.load(Ordering::Relaxed)
        }

        fn at(&mut self, now_us: u64) -> &mut Self {
            self.now.store(now_us, Ordering::Relaxed);
            self
        }
    }

    fn frame(frame_id: u32, keyframe: bool, received_us: u64, len: usize) -> ReceivedFrame {
        ReceivedFrame {
            frame: Frame {
                frame_id,
                epoch: 1,
                display_id: 1,
                flags: 0,
                keyframe,
                capture_ts_us: received_us - 500,
                data: vec![0x55; len],
            },
            received_us,
            capture_local_us: Some(42),
        }
    }

    fn config(epoch: u32) -> pb::StreamConfig {
        pb::StreamConfig {
            epoch,
            codec: pb::Codec::Hevc as i32,
            width: 1920,
            height: 1080,
            ..Default::default()
        }
    }

    const POS: CursorEvent = CursorEvent {
        x: 0.1,
        y: 0.2,
        visible: true,
    };

    #[test]
    fn keyframe_and_deltas_take_media_time_from_received_us() {
        let mut h = harness(QUEUE_BUDGET);
        for (i, delta) in [0, 16_667, 33_334, 50_001].into_iter().enumerate() {
            h.ingress
                .frame(&frame(i as u32, i == 0, 10_000_000 + delta, 100));
        }
        let records = h.drain();
        assert_eq!(
            records.iter().map(|record| record.t_us).collect::<Vec<_>>(),
            [0, 16_667, 33_334, 50_001]
        );
        for (i, record) in records.iter().enumerate() {
            let Body::Video(video) = &record.body else {
                panic!("expected video, got {:?}", record.body);
            };
            assert_eq!(video.frame_id, i as u32);
            assert_eq!(video.keyframe, i == 0);
            assert_eq!(video.capture_local_us, Some(42));
            assert_eq!(video.data.len(), 100);
        }
        assert_eq!(h.queued.load(Ordering::Acquire), 0);
        assert_eq!(h.keyframes(), 0);
    }

    #[test]
    fn backpressure_drops_deltas_until_a_keyframe_after_a_gap() {
        let mut h = harness(10 * 1024);
        let t0 = 1_000_000;
        let delta = 6 * 1024;
        h.ingress.frame(&frame(1, false, t0, delta));
        h.ingress.frame(&frame(2, false, t0 + 16_000, delta));
        h.ingress.frame(&frame(3, false, t0 + 33_000, delta));
        assert_eq!(h.keyframes(), 1);
        // 1 秒内再丢：不再请求。
        h.ingress.frame(&frame(4, false, t0 + 1_000_000, delta));
        assert_eq!(h.keyframes(), 1);
        let sent = h.drain();
        assert_eq!(sent.len(), 1);
        assert!(matches!(&sent[0].body, Body::Video(video) if video.frame_id == 1));
        // 有空间了也还在跳过，直到关键帧。
        h.ingress.frame(&frame(5, false, t0 + 1_100_000, delta));
        h.ingress.frame(&frame(6, true, t0 + 1_200_000, 2048));
        h.ingress.frame(&frame(7, false, t0 + 1_216_000, 100));
        let sent = h.drain();
        assert_eq!(
            sent[0],
            Record {
                t_us: 1_200_000,
                body: Body::Gap {
                    reason: GapReason::Backpressure,
                    real_us: 1_200_000 - 16_000,
                },
            }
        );
        assert!(
            matches!(&sent[1].body, Body::Video(video) if video.frame_id == 6 && video.keyframe)
        );
        assert!(matches!(&sent[2].body, Body::Video(video) if video.frame_id == 7));
        assert_eq!(sent.len(), 3);
        assert_eq!(h.keyframes(), 1);
        assert_eq!(h.ingress.dropped(), (4, 0));
    }

    #[test]
    fn audio_over_budget_is_dropped_and_counted() {
        let mut h = harness(100);
        h.ingress.audio(1, 10, &[1; 200]);
        h.ingress.audio(2, 30, &[1; 20]);
        let sent = h.drain();
        assert_eq!(sent.len(), 1);
        assert!(matches!(&sent[0].body, Body::Audio(audio) if audio.seq == 2));
        assert_eq!(h.ingress.dropped(), (0, 1));
        assert_eq!(h.keyframes(), 0);
    }

    #[test]
    fn state_records_wait_for_room_and_keep_the_latest() {
        let mut h = harness(QUEUE_BUDGET);
        for i in 0..4 {
            h.ingress
                .frame(&frame(i, i == 0, 1_000_000 + u64::from(i) * 1000, 10));
        }
        h.ingress.config(&config(1));
        h.ingress.config(&config(2));
        h.ingress.cursor(&POS);
        assert_eq!(h.drain().len(), 4);
        h.ingress.cursor_msg(&pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Use(7)),
        });
        let sent: Vec<_> = h.drain().into_iter().map(|record| record.body).collect();
        let [Body::Config(config), pos, Body::CursorUse(7)] = sent.as_slice() else {
            panic!("unexpected records {sent:?}");
        };
        assert_eq!(config.epoch, 2);
        assert_eq!(config.codec, pb::Codec::Hevc as u8);
        assert_eq!(
            *pos,
            Body::CursorPos {
                x: 0.1,
                y: 0.2,
                visible: true
            }
        );
    }

    #[test]
    fn cursor_messages_wait_in_a_bounded_queue() {
        let mut h = harness(QUEUE_BUDGET);
        for i in 0..4 {
            h.ingress.frame(&frame(i, i == 0, 1_000_000, 10));
        }
        for id in 0..20 {
            h.ingress.cursor_msg(&pb::CursorMsg {
                msg: Some(pb::cursor_msg::Msg::Use(id)),
            });
        }
        assert_eq!(h.drain().len(), 4);
        let mut kept = Vec::new();
        loop {
            h.ingress.flush_pending();
            let sent = h.drain();
            if sent.is_empty() {
                break;
            }
            kept.extend(sent.into_iter().map(|record| record.body));
        }
        assert_eq!(kept, (4..20).map(Body::CursorUse).collect::<Vec<_>>());
    }

    #[test]
    fn silences_compress_into_clock_and_paused_gaps() {
        let mut h = harness(QUEUE_BUDGET);
        h.at(1_000_000).ingress.cursor(&POS);
        h.at(6_000_000).ingress.cursor(&POS);
        let sent = h.drain();
        assert_eq!(sent[0].t_us, 0);
        assert_eq!(
            sent[1],
            Record {
                t_us: 100_000,
                body: Body::Gap {
                    reason: GapReason::Clock,
                    real_us: 5_000_000,
                },
            }
        );
        assert_eq!(sent[2].t_us, 100_000);
        assert_eq!(sent.len(), 3);

        h.ingress.mark_paused();
        h.at(11_000_000).ingress.cursor(&POS);
        let sent = h.drain();
        assert_eq!(
            sent[0].body,
            Body::Gap {
                reason: GapReason::Paused,
                real_us: 5_000_000,
            }
        );
        assert_eq!(sent[1].t_us, 200_000);
    }

    #[test]
    fn statvfs_probe_reports_free_space() {
        assert!(StatvfsProbe
            .free_bytes(&std::env::temp_dir())
            .is_some_and(|free| free > 0));
        assert_eq!(
            StatvfsProbe.free_bytes(Path::new("/nonexistent/rs-recording-probe")),
            None
        );
    }
}
