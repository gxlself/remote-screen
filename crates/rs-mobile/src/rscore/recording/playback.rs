//! 录制回放「假会话」（M3 V5）。id 从 PLAYBACK_ID_BASE 起，和 SessionTable 的 id 不会重叠。
//! 原生用回放 id 调 rs_session_attach（C9 分流到 attach_native）和 RsAudio.start；
//! 驱动任务按录制里的媒体时间，把配置、帧、光标和声音送进这个回放自己的 VideoTap 和 AudioEntry。
use std::{
    collections::VecDeque,
    fmt::Display,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

use rs_recording::{
    crypto::{SecretWrapper, WrapError},
    file::{FileError, RecordingReader},
    record::{self, Body, ConfigRec, CursorShapeRec, Record, RecordingEnd},
    store::RecordingStore,
    timeline::{EpochRemap, Pacer, SeekIndex},
};
use rsp_client::{audio::AudioSink, CursorEvent, ReceivedFrame};
use rsp_core::{pb, video::Frame};
use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

use super::{
    dto::{PlaybackCommand, PlaybackInfo, PlaybackRunState, PlaybackStateEvent},
    ffi_wrap,
};
use crate::rscore::{
    audio::{AudioEntry, AudioTable},
    dto_core::{id_str, CoreEvent},
    error::CoreError,
    ffi_session::attach_on_worker,
    runtime::Core,
    tables::{VideoConsumer, VideoTap},
};

/// 必须小于 RS_ADAPTER_ID_BASE（1 << 62）：iOS 的 SessionAttachment 和 `is_adapter_id` 把 `>= 1 << 62`
/// 的 id 当成适配器会话（原始帧、PCM 播放器），回放 id 落在它下面才会走 rs_session_attach 和 AudioTable。
/// RSP 会话 id 从 1 递增，碰不到这个号段。
pub const PLAYBACK_ID_BASE: u64 = 1 << 61;

/// 播放中 `playback.state` 的间隔。
const STATE_EVERY: Duration = Duration::from_millis(250);
const RECORDING_EXT: &str = ".rsrec";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// 与录制写入器使用同一个目录。
fn recordings_dir(core: &Core) -> PathBuf {
    core.data_dir().join("recordings")
}

fn not_found() -> CoreError {
    CoreError::Http {
        status: 404,
        hub_code: None,
        message: "recording.notFound".to_owned(),
    }
}

fn damaged(detail: impl Display) -> CoreError {
    CoreError::Http {
        status: 422,
        hub_code: None,
        message: format!("recording.damaged: {detail}"),
    }
}

fn open_error(error: FileError) -> CoreError {
    match error {
        FileError::Wrap(WrapError::Unavailable) => CoreError::internal("recording.keyUnavailable"),
        FileError::Io(error) if error.kind() == io::ErrorKind::NotFound => not_found(),
        error => damaged(error),
    }
}

/// 同一时间最多一个回放；id 递增，不复用。
#[derive(Default)]
pub(crate) struct PlaybackTable {
    state: Mutex<TableState>,
}

#[derive(Default)]
struct TableState {
    last: u64,
    current: Option<Arc<PlaybackEntry>>,
}

impl PlaybackTable {
    fn alloc_id(&self) -> u64 {
        let mut state = lock(&self.state);
        state.last = state
            .last
            .checked_add(1)
            .expect("playback id space exhausted");
        PLAYBACK_ID_BASE + state.last
    }

    /// 换上新回放，返回被换下的旧回放；调用方在锁外关掉它。
    fn replace(&self, entry: Arc<PlaybackEntry>) -> Option<Arc<PlaybackEntry>> {
        lock(&self.state).current.replace(entry)
    }

    fn get(&self, id: u64) -> Option<Arc<PlaybackEntry>> {
        lock(&self.state)
            .current
            .as_ref()
            .filter(|entry| entry.id == id)
            .cloned()
    }

    fn remove(&self, id: u64) -> Option<Arc<PlaybackEntry>> {
        let mut state = lock(&self.state);
        if state.current.as_ref().is_some_and(|entry| entry.id == id) {
            state.current.take()
        } else {
            None
        }
    }
}

enum Cmd {
    Control(PlaybackCommand),
    Reattach,
}

struct PlaybackEntry {
    id: u64,
    video: VideoTap,
    commands: mpsc::UnboundedSender<Cmd>,
    task: Mutex<TaskSlot>,
}

#[derive(Default)]
struct TaskSlot {
    closed: bool,
    handle: Option<JoinHandle<()>>,
}

impl PlaybackEntry {
    /// 并发的 open/close 可能在任务启动前就关掉了这个回放。
    fn set_task(&self, handle: JoinHandle<()>) {
        let mut slot = lock(&self.task);
        if slot.closed {
            handle.abort();
        } else {
            slot.handle = Some(handle);
        }
    }

    fn shutdown(&self, core: &Core) {
        let handle = {
            let mut slot = lock(&self.task);
            slot.closed = true;
            slot.handle.take()
        };
        if let Some(handle) = handle {
            handle.abort();
        }
        self.video.close("playback closed");
        core.state::<AudioTable>().remove(self.id);
    }
}

struct Opened {
    reader: RecordingReader,
    first: Vec<Record>,
    index: SeekIndex,
    normal_chunks: usize,
    /// 能读的块数：普通块，FINAL 读得出时再加 1。
    chunks: usize,
    duration_us: u64,
    width: u32,
    height: u32,
    has_audio: bool,
}

impl Opened {
    fn load(path: &Path, wrapper: &dyn SecretWrapper) -> Result<Self, CoreError> {
        let mut reader = RecordingReader::open(path, wrapper).map_err(open_error)?;
        let Some(last) = reader.chunks().last().copied() else {
            return Err(damaged("no chunks"));
        };
        let first = reader.read_chunk(0).map_err(damaged)?;
        let first = record::decode(&first).map_err(damaged)?;
        let meta_audio = first
            .iter()
            .find_map(|record| match &record.body {
                Body::Meta(meta) => Some(meta.audio),
                _ => None,
            })
            .ok_or_else(|| damaged("missing metadata"))?;
        let (width, height) = first
            .iter()
            .find_map(|record| match &record.body {
                Body::Config(config) => Some((config.width, config.height)),
                _ => None,
            })
            .ok_or_else(|| damaged("missing stream config"))?;
        let end = read_end(&mut reader);
        let duration_us = end.as_ref().map_or(last.t_start_us, |end| end.duration_us);
        let index = SeekIndex::from_chunks(
            reader
                .chunks()
                .iter()
                .enumerate()
                .map(|(i, chunk)| (i, chunk.keyframe_start(), chunk.t_start_us)),
            duration_us,
        );
        if index.points().is_empty() {
            return Err(damaged("no keyframe chunk"));
        }
        let normal_chunks = reader.chunks().len();
        Ok(Self {
            first,
            index,
            normal_chunks,
            chunks: normal_chunks + usize::from(end.is_some()),
            duration_us,
            width,
            height,
            has_audio: meta_audio && end.as_ref().is_none_or(|end| end.audio_packets > 0),
            reader,
        })
    }
}

fn read_end(reader: &mut RecordingReader) -> Option<RecordingEnd> {
    let plaintext = reader.read_final().ok()?;
    record::decode(&plaintext)
        .ok()?
        .into_iter()
        .find_map(|record| match record.body {
            Body::End(end) => Some(end),
            _ => None,
        })
}

/// 打开一份录制（只开 `.rsrec`，正在录的 `.part` 不行）。已经有回放时先关掉旧的。
pub async fn open_playback(
    core: &Arc<Core>,
    recording_id: &str,
) -> Result<PlaybackInfo, CoreError> {
    let wrapper =
        ffi_wrap::wrapper().ok_or_else(|| CoreError::unsupported("recording.unsupported"))?;
    if !RecordingStore::valid_id(recording_id) {
        return Err(not_found());
    }
    let path = recordings_dir(core).join(format!("{recording_id}{RECORDING_EXT}"));
    let opened = core
        .handle()
        .spawn_blocking(move || Opened::load(&path, wrapper.as_ref()))
        .await
        .map_err(CoreError::internal)??;

    let table = core.state::<PlaybackTable>();
    let id = table.alloc_id();
    let audio = if opened.has_audio {
        Some(core.state::<AudioTable>().get_or_create(id)?)
    } else {
        None
    };
    let (commands, receiver) = mpsc::unbounded_channel();
    let entry = Arc::new(PlaybackEntry {
        id,
        video: VideoTap::default(),
        commands: commands.clone(),
        task: Mutex::default(),
    });
    // 钩子在 VideoTap 的锁里运行：只发命令，不阻塞。
    entry.video.set_attach_hook(Box::new(move || {
        let _ = commands.send(Cmd::Reattach);
    }));
    let info = PlaybackInfo {
        playback_id: id_str(id),
        recording_id: recording_id.to_owned(),
        duration_ms: opened.duration_us / 1000,
        width: opened.width,
        height: opened.height,
        has_audio: opened.has_audio,
    };
    if let Some(old) = table.replace(entry.clone()) {
        old.shutdown(core);
    }
    let task = DriverTask {
        core: Arc::downgrade(core),
        entry: entry.clone(),
        audio,
        reader: Arc::new(Mutex::new(opened.reader)),
        normal_chunks: opened.normal_chunks,
        chunks: opened.chunks,
        playback_id: info.playback_id.clone(),
        recording_id: info.recording_id.clone(),
        duration_ms: info.duration_ms,
    };
    entry.set_task(core.spawn(task.run(opened.index, opened.first, receiver)));
    Ok(info)
}

/// 不等待结果：命令交给驱动任务，状态以 `playback.state` 事件给出。
pub fn control(
    core: &Arc<Core>,
    playback_id: u64,
    command: PlaybackCommand,
) -> Result<(), CoreError> {
    let entry = core
        .state::<PlaybackTable>()
        .get(playback_id)
        .ok_or_else(not_found)?;
    entry
        .commands
        .send(Cmd::Control(command))
        .map_err(|_| not_found())
}

/// id 不存在时也返回 Ok（幂等）。
pub fn close(core: &Arc<Core>, playback_id: u64) -> Result<(), CoreError> {
    if let Some(entry) = core.state::<PlaybackTable>().remove(playback_id) {
        entry.shutdown(core);
    }
    Ok(())
}

/// rs_session_attach 找不到会话时调用。同步回放缓存的配置（在 rs-core worker 上）；不是回放 id 返回 false。
pub fn attach_native(core: &Core, id: u64, video: Arc<dyn VideoConsumer>) -> bool {
    if id < PLAYBACK_ID_BASE {
        return false;
    }
    let Some(entry) = core.state::<PlaybackTable>().get(id) else {
        return false;
    };
    attach_on_worker(core, move || entry.video.attach(video))
}

/// rs_session_detach 找不到会话时调用。
pub fn detach_native(core: &Core, id: u64) {
    if let Some(entry) = core.state::<PlaybackTable>().get(id) {
        entry.video.detach();
    }
}

type Loaded = (usize, Result<Vec<Record>, String>);

/// 驱动任务：计时、读块、送出。Core 只存 Weak，任务不让 Core 无法释放。
struct DriverTask {
    core: Weak<Core>,
    entry: Arc<PlaybackEntry>,
    audio: Option<Arc<AudioEntry>>,
    reader: Arc<Mutex<RecordingReader>>,
    normal_chunks: usize,
    chunks: usize,
    playback_id: String,
    recording_id: String,
    duration_ms: u64,
}

impl DriverTask {
    async fn run(
        self,
        index: SeekIndex,
        first: Vec<Record>,
        mut commands: mpsc::UnboundedReceiver<Cmd>,
    ) {
        let Some(unix_base) = self.core.upgrade().map(|core| core.now_us()) else {
            return;
        };
        let base = Instant::now();
        let now = move || u64::try_from(base.elapsed().as_micros()).unwrap_or(u64::MAX);
        let mut driver = Driver::new(index, self.chunks, unix_base);
        let (loaded_tx, mut loaded) = mpsc::unbounded_channel::<Loaded>();
        self.emit(PlaybackRunState::Loading, 0);
        driver.chunk_loaded(0, first);
        self.emit(driver.state(), 0);
        let mut ticker = tokio::time::interval(STATE_EVERY);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let at = now();
            let step = driver.poll(at);
            for out in step.outs {
                self.deliver(out, &driver, at);
            }
            if let Some(chunk) = step.load {
                self.load(chunk, loaded_tx.clone());
            }
            let wake = step.wake_at.map(|us| base + Duration::from_micros(us));
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Cmd::Control(command)) => driver.command(command, now()),
                    Some(Cmd::Reattach) => driver.reattach(now()),
                    None => return,
                },
                Some((chunk, result)) = loaded.recv() => match result {
                    Ok(records) => driver.chunk_loaded(chunk, records),
                    Err(error) => {
                        tracing::warn!(playback_id = self.entry.id, chunk, %error, "playback chunk read failed");
                        driver.chunk_failed(chunk, now());
                    }
                },
                () = sleep_until(wake) => {}
                _ = ticker.tick() => {
                    if driver.state() == PlaybackRunState::Playing {
                        self.emit(PlaybackRunState::Playing, driver.position_us(now()));
                    }
                }
            }
        }
    }

    fn deliver(&self, out: Out, driver: &Driver, now_us: u64) {
        let video = &self.entry.video;
        match out {
            Out::Config(config) => video.deliver_config(&config),
            Out::Frame(frame) => {
                video.deliver_frame(&frame);
            }
            Out::Cursor(cursor) => video.deliver_cursor(&cursor),
            Out::CursorMsg(message) => video.deliver_cursor_msg(&message),
            Out::Audio {
                seq,
                capture_ts_us,
                payload,
            } => {
                if let Some(audio) = &self.audio {
                    audio.push(seq, capture_ts_us, &payload, driver.unix_us(now_us));
                }
            }
            Out::AudioReset => {
                if let Some(audio) = &self.audio {
                    audio.reset();
                }
            }
            Out::State(state) => self.emit(state, driver.position_us(now_us)),
        }
    }

    fn emit(&self, state: PlaybackRunState, position_us: u64) {
        if self.entry.video.is_closed() {
            return;
        }
        let Some(core) = self.core.upgrade() else {
            return;
        };
        core.events()
            .emit(&CoreEvent::PlaybackState(PlaybackStateEvent {
                playback_id: self.playback_id.clone(),
                recording_id: self.recording_id.clone(),
                state,
                position_ms: (position_us / 1000).min(self.duration_ms),
                duration_ms: self.duration_ms,
                error: (state == PlaybackRunState::Error).then(|| "recording.damaged".to_owned()),
            }));
    }

    fn load(&self, chunk: usize, done: mpsc::UnboundedSender<Loaded>) {
        let reader = self.reader.clone();
        let normal_chunks = self.normal_chunks;
        tokio::task::spawn_blocking(move || {
            let plaintext = {
                let mut reader = lock(&reader);
                if chunk < normal_chunks {
                    reader.read_chunk(chunk)
                } else {
                    reader.read_final()
                }
            };
            let records = plaintext
                .map_err(|error| error.to_string())
                .and_then(|plaintext| {
                    record::decode(&plaintext).map_err(|error| error.to_string())
                });
            let _ = done.send((chunk, records));
        });
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[derive(Debug)]
pub(crate) enum Out {
    Config(pb::StreamConfig),
    Frame(ReceivedFrame),
    Cursor(CursorEvent),
    CursorMsg(pb::CursorMsg),
    Audio {
        seq: u16,
        capture_ts_us: u64,
        payload: Vec<u8>,
    },
    AudioReset,
    State(PlaybackRunState),
}

#[derive(Debug, Default)]
pub(crate) struct Step {
    pub outs: Vec<Out>,
    /// 下一次该 poll 的时刻；None = 等下一个输入。
    pub wake_at: Option<u64>,
    /// 要读的块：普通块序号；等于普通块数时是 FINAL 块。
    pub load: Option<usize>,
}

/// 回放的纯状态机。时间是调用方的单调微秒。建好后调用方先送第 0 块（`chunk_loaded(0, ..)`），
/// 之后每个输入后调一次 `poll`，按 Step 送出、读块和定时。
pub(crate) struct Driver {
    pacer: Pacer,
    remap: EpochRemap,
    index: SeekIndex,
    chunks: usize,
    queue: VecDeque<Record>,
    /// 最近读到的一块的记录数，用来判断「剩下不到半块」。
    loaded_len: usize,
    next_chunk: usize,
    in_flight: Option<usize>,
    request: Option<usize>,
    /// 暂停时跳转：下一次 poll 立即送出状态前导和第一个关键帧。
    prime: bool,
    state: PlaybackRunState,
    outs: Vec<Out>,
    unix_base: u64,
}

impl Driver {
    /// chunks：能读的块数（普通块，FINAL 读得出时再加 1）。unix_base：now = 0 时的 unix 微秒。
    pub(crate) fn new(index: SeekIndex, chunks: usize, unix_base: u64) -> Self {
        Self {
            pacer: Pacer::new(),
            remap: EpochRemap::new(),
            index,
            chunks,
            queue: VecDeque::new(),
            loaded_len: 0,
            next_chunk: 0,
            in_flight: Some(0),
            request: None,
            prime: false,
            state: PlaybackRunState::Paused,
            outs: Vec::new(),
            unix_base,
        }
    }

    pub(crate) fn state(&self) -> PlaybackRunState {
        self.state
    }

    pub(crate) fn position_us(&self, now_us: u64) -> u64 {
        self.pacer.position(now_us)
    }

    pub(crate) fn unix_us(&self, now_us: u64) -> u64 {
        self.unix_base.saturating_add(now_us)
    }

    pub(crate) fn command(&mut self, command: PlaybackCommand, now_us: u64) {
        if self.state == PlaybackRunState::Error {
            return;
        }
        match command {
            PlaybackCommand::Play => {
                if self.state == PlaybackRunState::Ended {
                    self.seek(0, now_us);
                }
                if !self.pacer.is_playing() {
                    self.pacer.play(now_us);
                    // 不让声音缓冲里残留暂停或跳转前的数据。
                    self.audio_reset();
                }
                self.set_state(PlaybackRunState::Playing);
            }
            PlaybackCommand::Pause => {
                if self.state == PlaybackRunState::Playing {
                    self.pacer.pause(now_us);
                    self.set_state(PlaybackRunState::Paused);
                }
            }
            PlaybackCommand::Seek { position_ms } => {
                self.seek(position_ms.saturating_mul(1000), now_us);
                if self.state == PlaybackRunState::Ended {
                    self.set_state(PlaybackRunState::Paused);
                }
            }
        }
    }

    /// 原生刚 attach，解码器还没有关键帧：在当前位置重新跳一次（退到前一个关键帧）。
    pub(crate) fn reattach(&mut self, now_us: u64) {
        if self.state != PlaybackRunState::Error {
            self.seek(self.pacer.position(now_us), now_us);
        }
    }

    pub(crate) fn chunk_loaded(&mut self, chunk: usize, records: Vec<Record>) {
        // 跳转前请求的块已经作废。
        if self.in_flight != Some(chunk) {
            return;
        }
        self.in_flight = None;
        if self.request == Some(chunk) {
            self.request = None;
        }
        self.next_chunk = chunk + 1;
        self.loaded_len = records.len();
        self.queue.extend(records);
    }

    pub(crate) fn chunk_failed(&mut self, chunk: usize, now_us: u64) {
        if self.in_flight != Some(chunk) {
            return;
        }
        self.in_flight = None;
        self.request = None;
        self.prime = false;
        self.queue.clear();
        self.pacer.pause(now_us);
        self.set_state(PlaybackRunState::Error);
    }

    pub(crate) fn poll(&mut self, now_us: u64) -> Step {
        let mut wake_at = None;
        if self.state != PlaybackRunState::Error {
            if self.prime {
                self.deliver_prelude(now_us);
            }
            wake_at = self.deliver_due(now_us);
            self.prefetch();
            if self.state == PlaybackRunState::Playing
                && self.queue.is_empty()
                && self.in_flight.is_none()
                && self.next_chunk >= self.chunks
            {
                self.end(now_us);
            }
        }
        Step {
            outs: std::mem::take(&mut self.outs),
            wake_at,
            load: self.request.take(),
        }
    }

    fn deliver_due(&mut self, now_us: u64) -> Option<u64> {
        loop {
            let due = self.pacer.due_at(self.queue.front()?.t_us)?;
            if due > now_us {
                return Some(due);
            }
            let record = self.queue.pop_front()?;
            self.output(record, now_us);
            if self.state == PlaybackRunState::Ended {
                return None;
            }
        }
    }

    /// 不看 due_at，送到第一个关键帧为止（含），让画面停在跳转点上。
    fn deliver_prelude(&mut self, now_us: u64) {
        while let Some(record) = self.queue.pop_front() {
            let keyframe = matches!(&record.body, Body::Video(video) if video.keyframe);
            self.output(record, now_us);
            if keyframe {
                self.prime = false;
                return;
            }
        }
    }

    /// 队列剩下不到半块时预读下一块，最多预读 1 块。
    fn prefetch(&mut self) {
        if self.in_flight.is_none()
            && self.next_chunk < self.chunks
            && self.queue.len() * 2 < self.loaded_len.max(1)
        {
            self.load(self.next_chunk);
        }
    }

    fn load(&mut self, chunk: usize) {
        self.in_flight = Some(chunk);
        self.request = Some(chunk);
    }

    /// 跳到 target 之前最近的关键帧块；播放或暂停的状态不变。
    fn seek(&mut self, target_us: u64, now_us: u64) {
        let Some(point) = self.index.locate(target_us) else {
            return;
        };
        self.pacer.seek(point.t_us, now_us);
        self.remap.on_seek();
        self.queue.clear();
        self.loaded_len = 0;
        self.load(point.chunk);
        self.prime = !self.pacer.is_playing();
        self.audio_reset();
    }

    /// 播完：停在最后一帧，不关闭 VideoTap。
    fn end(&mut self, now_us: u64) {
        self.pacer.pause(now_us);
        self.queue.clear();
        self.set_state(PlaybackRunState::Ended);
    }

    fn output(&mut self, record: Record, now_us: u64) {
        let out = match record.body {
            Body::Config(config) => {
                let epoch = self.remap.on_config(config.epoch);
                Out::Config(stream_config(&config, epoch))
            }
            Body::Video(video) => {
                if !self.remap.frame_matches(video.epoch) {
                    return;
                }
                Out::Frame(ReceivedFrame {
                    frame: Frame {
                        frame_id: video.frame_id,
                        epoch: self.remap.frame_epoch(),
                        display_id: video.display_id,
                        flags: video.flags,
                        keyframe: video.keyframe,
                        capture_ts_us: video.capture_ts_us,
                        data: video.data,
                    },
                    received_us: self.unix_us(now_us),
                    capture_local_us: None,
                })
            }
            Body::CursorPos { x, y, visible } => Out::Cursor(CursorEvent { x, y, visible }),
            Body::CursorShape(shape) => cursor_msg(pb::cursor_msg::Msg::Shape(cursor_shape(shape))),
            Body::CursorUse(id) => cursor_msg(pb::cursor_msg::Msg::Use(id)),
            Body::CursorHidden(hidden) => cursor_msg(pb::cursor_msg::Msg::Hidden(hidden)),
            Body::Audio(audio) => Out::Audio {
                seq: audio.seq,
                capture_ts_us: audio.capture_ts_us,
                payload: audio.payload,
            },
            Body::End(_) => {
                self.end(now_us);
                return;
            }
            Body::Meta(_) | Body::Gap { .. } | Body::Unknown { .. } => return,
        };
        self.outs.push(out);
    }

    fn set_state(&mut self, state: PlaybackRunState) {
        if self.state != state {
            self.state = state;
            self.outs.push(Out::State(state));
        }
    }

    fn audio_reset(&mut self) {
        if !matches!(self.outs.last(), Some(Out::AudioReset)) {
            self.outs.push(Out::AudioReset);
        }
    }
}

fn stream_config(config: &ConfigRec, epoch: u32) -> pb::StreamConfig {
    pb::StreamConfig {
        epoch,
        display_id: config.display_id,
        codec: i32::from(config.codec),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate_kbps: config.bitrate_kbps,
        cursor_x: config.cursor_x,
        cursor_y: config.cursor_y,
        cursor_visible: config.cursor_visible,
        ..Default::default()
    }
}

fn cursor_msg(msg: pb::cursor_msg::Msg) -> Out {
    Out::CursorMsg(pb::CursorMsg { msg: Some(msg) })
}

fn cursor_shape(shape: CursorShapeRec) -> pb::CursorShape {
    pb::CursorShape {
        id: shape.id,
        width: shape.width,
        height: shape.height,
        hot_x: shape.hot_x,
        hot_y: shape.hot_y,
        scale: shape.scale,
        png: shape.png,
    }
}

#[cfg(test)]
mod tests {
    use rs_recording::record::{AudioRec, RecordingMeta, VideoRec};

    use super::*;

    const FRAME_US: u64 = 33_000;
    const SECOND_US: u64 = 1_000_000;
    const END_US: u64 = SECOND_US + 30 * FRAME_US;

    #[derive(Debug, Clone, PartialEq)]
    enum Seen {
        Config(u32),
        Frame { id: u32, epoch: u8, key: bool },
        Cursor,
        CursorMsg,
        Audio(u16),
        AudioReset,
        State(PlaybackRunState),
    }

    impl Seen {
        fn of(out: &Out) -> Self {
            match out {
                Out::Config(config) => Seen::Config(config.epoch),
                Out::Frame(frame) => Seen::Frame {
                    id: frame.frame.frame_id,
                    epoch: frame.frame.epoch,
                    key: frame.frame.keyframe,
                },
                Out::Cursor(_) => Seen::Cursor,
                Out::CursorMsg(_) => Seen::CursorMsg,
                Out::Audio { seq, .. } => Seen::Audio(*seq),
                Out::AudioReset => Seen::AudioReset,
                Out::State(state) => Seen::State(*state),
            }
        }
    }

    fn key(id: u32, epoch: u8) -> Seen {
        Seen::Frame {
            id,
            epoch,
            key: true,
        }
    }

    fn delta(id: u32, epoch: u8) -> Seen {
        Seen::Frame {
            id,
            epoch,
            key: false,
        }
    }

    fn meta() -> Record {
        Record {
            t_us: 0,
            body: Body::Meta(RecordingMeta {
                host_id: "host".into(),
                host_name: "Mac".into(),
                role: "controller".into(),
                display_id: Some(1),
                started_at_us: 1,
                app_version: "test".into(),
                audio: true,
            }),
        }
    }

    fn config(t_us: u64, epoch: u32) -> Record {
        Record {
            t_us,
            body: Body::Config(ConfigRec {
                epoch,
                display_id: 1,
                codec: 1,
                width: 1920,
                height: 1080,
                fps: 30,
                bitrate_kbps: 8000,
                cursor_x: 0.5,
                cursor_y: 0.5,
                cursor_visible: true,
            }),
        }
    }

    fn video(t_us: u64, frame_id: u32, epoch: u8, keyframe: bool) -> Record {
        Record {
            t_us,
            body: Body::Video(VideoRec {
                frame_id,
                epoch,
                display_id: 1,
                flags: u8::from(keyframe),
                keyframe,
                capture_ts_us: t_us,
                capture_local_us: Some(7),
                data: vec![0, 0, 0, 1, 0x26, frame_id as u8],
            }),
        }
    }

    fn end(t_us: u64) -> Record {
        Record {
            t_us,
            body: Body::End(RecordingEnd {
                duration_us: t_us,
                ..RecordingEnd::default()
            }),
        }
    }

    /// Config(epoch 5)、关键帧和 30 个增量帧，每 33 ms 一帧；frame_id 从 first_id 起。
    fn block(start_us: u64, first_id: u32) -> Vec<Record> {
        let mut records = vec![config(start_us, 5)];
        for i in 0..=30u32 {
            records.push(video(
                start_us + u64::from(i) * FRAME_US,
                first_id + i,
                5,
                i == 0,
            ));
        }
        records
    }

    /// 第 0 块：Meta + block(0)；第 1 块：block(1 s)；然后是 FINAL（End）。
    fn recording() -> Vec<Vec<Record>> {
        let mut first = vec![meta()];
        first.extend(block(0, 0));
        vec![first, block(SECOND_US, 31), vec![end(END_US)]]
    }

    fn index(chunks: &[Vec<Record>]) -> SeekIndex {
        let points = chunks[..chunks.len() - 1]
            .iter()
            .enumerate()
            .map(|(i, records)| (i, true, records[0].t_us));
        let duration_us = chunks.last().and_then(|c| c.last()).map_or(0, |r| r.t_us);
        SeekIndex::from_chunks(points, duration_us)
    }

    struct Harness {
        driver: Driver,
        chunks: Vec<Vec<Record>>,
        loads: Vec<usize>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(recording())
        }

        fn with(chunks: Vec<Vec<Record>>) -> Self {
            let mut driver = Driver::new(index(&chunks), chunks.len(), 1_700_000_000_000_000);
            driver.chunk_loaded(0, chunks[0].clone());
            Self {
                driver,
                chunks,
                loads: Vec::new(),
            }
        }

        /// poll 一次；有读块请求就立即送块再 poll，直到不再要块。
        fn poll(&mut self, now: u64) -> (Vec<Seen>, Option<u64>) {
            let mut seen = Vec::new();
            loop {
                let step = self.driver.poll(now);
                seen.extend(step.outs.iter().map(Seen::of));
                let Some(chunk) = step.load else {
                    return (seen, step.wake_at);
                };
                self.loads.push(chunk);
                self.driver.chunk_loaded(chunk, self.chunks[chunk].clone());
            }
        }

        /// 从 from 起按 wake_at 一路推进，直到下一次唤醒晚于 until。
        fn run(&mut self, from: u64, until: u64) -> Vec<Seen> {
            let mut now = from;
            let mut seen = Vec::new();
            loop {
                let (out, wake) = self.poll(now);
                seen.extend(out);
                match wake {
                    Some(at) if at <= until => now = at,
                    _ => return seen,
                }
            }
        }

        fn command(&mut self, command: PlaybackCommand, now: u64) -> Vec<Seen> {
            self.driver.command(command, now);
            self.poll(now).0
        }
    }

    fn frames(seen: &[Seen]) -> Vec<Seen> {
        seen.iter()
            .filter(|s| matches!(s, Seen::Frame { .. }))
            .cloned()
            .collect()
    }

    fn deltas(ids: std::ops::RangeInclusive<u32>, epoch: u8) -> Vec<Seen> {
        ids.map(|id| delta(id, epoch)).collect()
    }

    /// 测试 2 之后：暂停在 0，已送出 Config(epoch 1) 和关键帧。
    fn seeked_to_start() -> Harness {
        let mut h = Harness::new();
        assert_eq!(
            h.command(PlaybackCommand::Seek { position_ms: 0 }, 0),
            vec![Seen::AudioReset, Seen::Config(1), key(0, 1)]
        );
        h
    }

    /// 测试 3 之后：从 0 开始播放，推进到 1.5 s（第 1 块已经读入）。
    fn playing_at(until: u64) -> Harness {
        let mut h = seeked_to_start();
        h.command(PlaybackCommand::Play, 0);
        h.run(0, until);
        h
    }

    #[test]
    fn playback_driver_starts_paused_without_output() {
        let mut h = Harness::new();
        let step = h.driver.poll(0);
        assert!(step.outs.is_empty(), "{:?}", step.outs);
        assert_eq!((step.wake_at, step.load), (None, None));
        assert_eq!(h.driver.state(), PlaybackRunState::Paused);
        assert_eq!(h.run(0, 10 * SECOND_US), vec![]);
    }

    #[test]
    fn playback_driver_paused_seek_sends_prelude_and_keyframe_at_once() {
        let mut h = seeked_to_start();
        assert_eq!(h.poll(5 * SECOND_US), (vec![], None));
        assert_eq!(h.driver.state(), PlaybackRunState::Paused);
        assert_eq!(h.driver.position_us(5 * SECOND_US), 0);
    }

    #[test]
    fn playback_driver_play_sends_due_frames_in_order_and_prefetches() {
        let mut h = seeked_to_start();
        let started = h.command(PlaybackCommand::Play, 0);
        assert_eq!(
            started,
            vec![Seen::AudioReset, Seen::State(PlaybackRunState::Playing)]
        );
        // 0..400 ms：12 个增量帧，队列还剩一半以上，不预读。
        let seen = h.run(0, 400_000);
        assert_eq!(seen, deltas(1..=12, 1));
        assert_eq!(h.loads, vec![0]);
        // 推进到 500 ms：t <= 500 ms 的帧全部送出，按顺序；剩下不到半块时要第 1 块。
        let seen = h.run(400_000, 500_000);
        assert_eq!(seen, deltas(13..=15, 1));
        assert_eq!(h.loads, vec![0, 1]);
        let (seen, wake) = h.poll(500_000);
        assert_eq!(seen, vec![]);
        assert_eq!(wake, Some(16 * FRAME_US));
        // 第 1 块到达时已经读入，不再重复读；最多预读 1 块，FINAL 还不读。
        let seen = h.run(500_000, SECOND_US);
        assert_eq!(
            seen,
            [deltas(16..=30, 1), vec![Seen::Config(1), key(31, 1)]].concat()
        );
        assert_eq!(h.loads, vec![0, 1]);
    }

    #[test]
    fn playback_driver_seek_back_while_playing_resets_audio_and_renumbers_epoch() {
        let mut h = playing_at(1_500_000);
        assert_eq!(h.loads, vec![0, 1, 2]);
        let seen = h.command(PlaybackCommand::Seek { position_ms: 200 }, 1_500_000);
        assert_eq!(seen, vec![Seen::AudioReset, Seen::Config(2), key(0, 2)]);
        assert_eq!(h.loads, vec![0, 1, 2, 0]);
        assert_eq!(h.driver.state(), PlaybackRunState::Playing);
        assert_eq!(h.driver.position_us(1_500_000), 0);
        assert_eq!(h.run(1_500_000, 1_600_000), deltas(1..=3, 2));
    }

    #[test]
    fn playback_driver_seek_into_the_second_chunk() {
        let mut h = playing_at(1_500_000);
        h.command(PlaybackCommand::Seek { position_ms: 200 }, 1_500_000);
        let seen = h.command(PlaybackCommand::Seek { position_ms: 1500 }, 1_600_000);
        assert_eq!(seen, vec![Seen::AudioReset, Seen::Config(3), key(31, 3)]);
        assert_eq!(h.driver.position_us(1_600_000), SECOND_US);
        assert_eq!(h.run(1_600_000, 1_700_000), deltas(32..=34, 3));
    }

    #[test]
    fn playback_driver_ends_at_the_end_record_and_play_restarts() {
        let mut h = playing_at(1_500_000);
        let seen = h.run(1_500_000, 3 * SECOND_US);
        assert_eq!(
            seen,
            [
                deltas(47..=61, 1),
                vec![Seen::State(PlaybackRunState::Ended)]
            ]
            .concat()
        );
        assert_eq!(h.driver.state(), PlaybackRunState::Ended);
        assert_eq!(h.poll(10 * SECOND_US), (vec![], None));
        let now = 10 * SECOND_US;
        let seen = h.command(PlaybackCommand::Play, now);
        assert_eq!(
            seen,
            vec![
                Seen::AudioReset,
                Seen::State(PlaybackRunState::Playing),
                Seen::Config(2),
                key(0, 2),
            ]
        );
        assert_eq!(h.run(now, now + 100_000), deltas(1..=3, 2));
    }

    #[test]
    fn playback_driver_play_after_end_takes_the_next_epoch() {
        // 卡里的顺序：Seek(200) → epoch 2，Seek(1500) → epoch 3，播完后 Play → epoch 4。
        let mut h = playing_at(1_500_000);
        h.command(PlaybackCommand::Seek { position_ms: 200 }, 1_500_000);
        h.command(PlaybackCommand::Seek { position_ms: 1500 }, 1_600_000);
        let seen = h.run(1_600_000, 4 * SECOND_US);
        assert_eq!(seen.last(), Some(&Seen::State(PlaybackRunState::Ended)));
        let seen = h.command(PlaybackCommand::Play, 5 * SECOND_US);
        assert_eq!(&seen[2..], [Seen::Config(4), key(0, 4)]);
        assert_eq!(h.driver.state(), PlaybackRunState::Playing);
    }

    #[test]
    fn playback_driver_drops_frames_with_a_stale_epoch() {
        let first = vec![
            meta(),
            config(0, 5),
            video(0, 0, 5, true),
            video(FRAME_US, 1, 6, false),
            video(2 * FRAME_US, 2, 5, false),
        ];
        let mut h = Harness::with(vec![first, vec![end(2 * FRAME_US)]]);
        h.command(PlaybackCommand::Seek { position_ms: 0 }, 0);
        h.command(PlaybackCommand::Play, 0);
        let seen = h.run(0, SECOND_US);
        assert_eq!(frames(&seen), vec![delta(2, 1)]);
        assert_eq!(seen.last(), Some(&Seen::State(PlaybackRunState::Ended)));
    }

    #[test]
    fn playback_driver_reattach_seeks_at_the_current_position() {
        let mut h = Harness::new();
        h.driver.reattach(0);
        assert_eq!(
            h.poll(0).0,
            vec![Seen::AudioReset, Seen::Config(1), key(0, 1)]
        );
        h.command(PlaybackCommand::Play, 0);
        h.run(0, 700_000);
        h.driver.reattach(700_000);
        assert_eq!(
            h.poll(700_000).0,
            vec![Seen::AudioReset, Seen::Config(2), key(0, 2)]
        );
        assert_eq!(h.driver.state(), PlaybackRunState::Playing);
        // 在第 1 块里暂停后 reattach：退到 1 s 的关键帧，暂停中也立即送出。
        h.run(700_000, 2_300_000);
        h.command(PlaybackCommand::Pause, 2_300_000);
        h.driver.reattach(2_400_000);
        assert_eq!(
            h.poll(2_400_000).0,
            vec![Seen::AudioReset, Seen::Config(3), key(31, 3)]
        );
        assert_eq!(h.driver.state(), PlaybackRunState::Paused);
    }

    #[test]
    fn playback_driver_pause_holds_position_and_resumes_on_time() {
        let mut h = playing_at(200_000);
        assert_eq!(
            h.command(PlaybackCommand::Pause, 200_000),
            vec![Seen::State(PlaybackRunState::Paused)]
        );
        assert_eq!(h.poll(900_000), (vec![], None));
        assert_eq!(h.driver.position_us(900_000), 200_000);
        let seen = h.command(PlaybackCommand::Play, 900_000);
        assert_eq!(
            seen,
            vec![Seen::AudioReset, Seen::State(PlaybackRunState::Playing)]
        );
        // 第 7 帧（231 ms）在恢复后 31 ms 到期。
        let (_, wake) = h.poll(900_000);
        assert_eq!(wake, Some(900_000 + 7 * FRAME_US - 200_000));
    }

    #[test]
    fn playback_driver_ignores_a_chunk_requested_before_a_seek() {
        let mut h = seeked_to_start();
        h.driver.command(PlaybackCommand::Play, 0);
        let mut now = 0;
        let requested = loop {
            let step = h.driver.poll(now);
            if let Some(chunk) = step.load {
                break chunk;
            }
            now = step.wake_at.unwrap();
        };
        assert_eq!(requested, 1);
        h.driver
            .command(PlaybackCommand::Seek { position_ms: 0 }, now);
        let step = h.driver.poll(now);
        assert_eq!(step.load, Some(0));
        h.driver.chunk_loaded(1, h.chunks[1].clone());
        assert!(h.driver.poll(now).outs.is_empty());
        h.driver.chunk_loaded(0, h.chunks[0].clone());
        let seen: Vec<_> = h.driver.poll(now).outs.iter().map(Seen::of).collect();
        assert_eq!(seen, vec![Seen::Config(2), key(0, 2)]);
    }

    #[test]
    fn playback_driver_stops_on_a_read_failure() {
        let mut h = playing_at(400_000);
        let mut now = 400_000;
        let requested = loop {
            let step = h.driver.poll(now);
            if let Some(chunk) = step.load {
                break chunk;
            }
            now = step.wake_at.unwrap();
        };
        h.driver.chunk_failed(requested, now);
        let step = h.driver.poll(now);
        let seen: Vec<_> = step.outs.iter().map(Seen::of).collect();
        assert_eq!(seen, vec![Seen::State(PlaybackRunState::Error)]);
        assert_eq!((step.wake_at, step.load), (None, None));
        h.driver
            .command(PlaybackCommand::Seek { position_ms: 0 }, now);
        h.driver.reattach(now);
        assert!(h.driver.poll(now + SECOND_US).outs.is_empty());
        assert_eq!(h.driver.state(), PlaybackRunState::Error);
    }

    #[test]
    fn playback_driver_forwards_cursor_audio_and_unix_receive_time() {
        let shape = CursorShapeRec {
            id: 9,
            width: 2,
            height: 2,
            hot_x: 1,
            hot_y: 1,
            scale: 2.0,
            png: vec![1, 2, 3],
        };
        let first = vec![
            meta(),
            config(0, 5),
            Record {
                t_us: 0,
                body: Body::CursorPos {
                    x: 0.25,
                    y: 0.75,
                    visible: true,
                },
            },
            Record {
                t_us: 0,
                body: Body::CursorShape(shape),
            },
            Record {
                t_us: 0,
                body: Body::CursorUse(9),
            },
            video(0, 0, 5, true),
            Record {
                t_us: 10_000,
                body: Body::Audio(AudioRec {
                    seq: 3,
                    capture_ts_us: 77,
                    payload: vec![0xf8],
                }),
            },
            Record {
                t_us: 20_000,
                body: Body::Gap {
                    reason: rs_recording::record::GapReason::Paused,
                    real_us: 9_000_000,
                },
            },
            Record {
                t_us: 30_000,
                body: Body::CursorHidden(true),
            },
        ];
        let mut h = Harness::with(vec![first, vec![end(30_000)]]);
        h.driver
            .command(PlaybackCommand::Seek { position_ms: 0 }, 0);
        let step = h.driver.poll(0);
        assert_eq!(step.load, Some(0));
        h.driver.chunk_loaded(0, h.chunks[0].clone());
        let step = h.driver.poll(5);
        // 前导之后队列不到半块，预读 FINAL。
        assert_eq!(step.load, Some(1));
        h.driver.chunk_loaded(1, h.chunks[1].clone());
        let frame = step
            .outs
            .iter()
            .find_map(|out| match out {
                Out::Frame(frame) => Some(frame),
                _ => None,
            })
            .unwrap();
        assert_eq!(frame.received_us, 1_700_000_000_000_005);
        assert_eq!(frame.capture_local_us, None);
        assert_eq!(frame.frame.data, vec![0, 0, 0, 1, 0x26, 0]);
        let messages: Vec<_> = step
            .outs
            .iter()
            .filter_map(|out| match out {
                Out::CursorMsg(message) => message.msg.clone(),
                _ => None,
            })
            .collect();
        assert!(matches!(&messages[..], [
            pb::cursor_msg::Msg::Shape(pb::CursorShape { id: 9, png, .. }),
            pb::cursor_msg::Msg::Use(9),
        ] if png[..] == [1, 2, 3]));
        let config = step
            .outs
            .iter()
            .find_map(|out| match out {
                Out::Config(config) => Some(*config),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            (config.codec, config.width, config.height, config.fps),
            (1, 1920, 1080, 30)
        );
        h.command(PlaybackCommand::Play, 5);
        assert_eq!(
            h.run(5, SECOND_US),
            vec![
                Seen::Audio(3),
                Seen::CursorMsg,
                Seen::State(PlaybackRunState::Ended)
            ]
        );
    }
}
