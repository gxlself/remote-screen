//! Portal-authorized PipeWire video streams, shared by all captures of a display.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::capture_model::CursorModel;
use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, CursorUpdate, DisplayInfo, FrameSink,
    PlatformError,
};
use rsp_core::pb;
use tokio::sync::{mpsc, watch};

use crate::capture_model::{
    cursor_pixels_from_spa, cursor_to_shape, pointer_report, BufferInfo, CaptureHub, FrameDecision,
    FrameGate, HubHandle,
};
use crate::frame::{copy_to_bgrx, FramePool, SrcFormat};
use crate::portal::PortalGrant;
use crate::portal_model::{cursor, StreamDisplay};
use crate::pw::{
    DataView, Direction, PwLib, PwStream, Remote, StreamCtl, StreamHandler, StreamSpec,
};
use crate::pw_ffi::{state, stream_flag};
use crate::spa_pod::{self, data_type, meta_type, param, video_format, MetaSize};

const POOL_SIZE: usize = 4;
const MAX_SIDE: u32 = 8192;
const MAX_CURSOR_META: usize = spa_pod::cursor_meta_size(256, 256) as usize;

/// One fresh portal remote fd per display; streams retain it until capture teardown.
pub struct WlStreamSource {
    pub display_id: u32,
    pub node_id: u32,
    pub fd: OwnedFd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPointer {
    pub display_id: u32,
    /// Normalized hotspot coordinates in the stream, in 0..=1.
    pub nx: f64,
    pub ny: f64,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WlStats {
    pub buffers: u64,
    pub encoded: u64,
    pub cursor_only: u64,
    pub skipped: u64,
    pub pool_full: u64,
    pub bad_buffers: u64,
    pub active: bool,
}

pub struct WaylandCapture {
    inner: Arc<Inner>,
}

struct Inner {
    // Serializes add/remove/set_active. Callbacks never acquire this lock, and
    // shared.state is always released before entering any PwStream method.
    streams: Mutex<BTreeMap<u32, PwStream>>,
    shared: Arc<Shared>,
}

// Callbacks own Shared, not Inner: no ownership cycle, and the last callback
// cannot destroy/join its own PipeWire thread when a capture handle is dropped.
struct Shared {
    state: Mutex<CaptureState>,
    lost: watch::Sender<bool>,
}

struct CaptureState {
    displays: BTreeMap<u32, DisplayState>,
    hub: CaptureHub<u32>,
    display_watchers: Vec<mpsc::Sender<Vec<DisplayInfo>>>,
    cursor: CursorModel,
    cursor_watchers: Vec<mpsc::Sender<CursorUpdate>>,
    pointer_watchers: Vec<mpsc::Sender<RawPointer>>,
    cursor_seq: i64,
    metadata_cursor: bool,
}

struct DisplayState {
    info: DisplayInfo,
    logical_width: u32,
    stats: WlStats,
    bad: bool,
    disconnected: bool,
    activation: u64,
}

impl DisplayState {
    fn new(display: &StreamDisplay) -> Self {
        Self {
            info: DisplayInfo {
                id: display.id,
                name: format!("Display {}", display.id),
                width_px: display.width,
                height_px: display.height,
                scale: 1.0,
                x: display.x,
                y: display.y,
                refresh_hz: 60,
                primary: display.primary,
                rotation: 0,
            },
            logical_width: display.width,
            stats: WlStats::default(),
            bad: false,
            disconnected: false,
            activation: 0,
        }
    }
}

impl CaptureState {
    fn displays(&self) -> Vec<DisplayInfo> {
        self.displays
            .values()
            .filter(|display| !display.bad)
            .map(|display| display.info.clone())
            .collect()
    }

    fn notify_displays(&mut self) {
        let displays = self.displays();
        broadcast(&mut self.display_watchers, &displays);
    }
}

impl Shared {
    fn new(displays: BTreeMap<u32, DisplayState>, metadata_cursor: bool) -> Self {
        Self {
            state: Mutex::new(CaptureState {
                displays,
                hub: CaptureHub::new(),
                display_watchers: Vec::new(),
                cursor: CursorModel::new(),
                cursor_watchers: Vec::new(),
                pointer_watchers: Vec::new(),
                cursor_seq: 0,
                metadata_cursor,
            }),
            lost: watch::channel(false).0,
        }
    }

    fn fail(&self, id: u32, disconnected: bool) {
        let mut state = lock(&self.state);
        if let Some(display) = state.displays.get_mut(&id) {
            let changed = !display.bad;
            display.bad = true;
            display.disconnected |= disconnected;
            display.stats.active = false;
            state.cursor.display_stopped(id);
            if changed {
                state.notify_displays();
            }
        }
        self.check_lost(&state);
    }

    fn check_lost(&self, state: &CaptureState) {
        if state.displays.values().all(|display| display.disconnected) {
            self.lost.send_if_modified(|lost| {
                let changed = !*lost;
                *lost = true;
                changed
            });
        }
    }
}

impl WaylandCapture {
    /// Immediately creates inactive streams using the supplied portal fds.
    /// Displays whose streams cannot be created are omitted from the list.
    pub fn new(lib: Arc<PwLib>, grant: &PortalGrant, sources: Vec<WlStreamSource>) -> Self {
        let mut displays = BTreeMap::new();
        let mut accepted = Vec::new();
        for source in sources {
            let Some(display) = grant.displays.iter().find(|display| {
                display.id == source.display_id && display.node_id == source.node_id
            }) else {
                tracing::warn!(
                    display = source.display_id,
                    "ungranted PipeWire stream source"
                );
                continue;
            };
            if displays.contains_key(&display.id) {
                let display_id = display.id;
                tracing::warn!(display_id, "duplicate PipeWire stream source");
                continue;
            }
            displays.insert(display.id, DisplayState::new(display));
            accepted.push((source, display));
        }
        // Register all displays before any callback can run, so the first failed
        // connection cannot report the entire portal session as lost prematurely.
        let shared = Arc::new(Shared::new(displays, grant.cursor_mode == cursor::METADATA));
        let mut streams = BTreeMap::new();
        for (source, display) in accepted {
            let handler = VideoHandler::new(shared.clone(), display.id);
            match PwStream::start(
                lib.clone(),
                Remote::Fd(source.fd),
                stream_spec(display),
                Box::new(handler),
            ) {
                Ok(stream) => {
                    streams.insert(display.id, stream);
                }
                Err(error) => {
                    let display_id = display.id;
                    tracing::warn!(display_id, %error, "cannot create PipeWire capture");
                    lock(&shared.state).displays.remove(&display.id);
                }
            }
        }
        shared.check_lost(&lock(&shared.state));
        Self {
            inner: Arc::new(Inner {
                streams: Mutex::new(streams),
                shared,
            }),
        }
    }

    pub fn displays(&self) -> Vec<DisplayInfo> {
        lock(&self.inner.shared.state).displays()
    }

    pub fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        let mut state = lock(&self.inner.shared.state);
        let _ = tx.try_send(state.displays());
        state.display_watchers.push(tx);
    }

    pub fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, PlatformError> {
        let streams = lock(&self.inner.streams);
        let stream = streams.get(&display).ok_or_else(display_unavailable)?;
        let (id, first) = {
            let mut state = lock(&self.inner.shared.state);
            let entry = state
                .displays
                .get_mut(&display)
                .filter(|entry| !entry.bad)
                .ok_or_else(display_unavailable)?;
            // Set this before activating: a process callback can run before
            // set_active returns. The handler observes activation and resets its gate.
            entry.stats.active = true;
            let (id, first) = state.hub.add(display, sink, cfg);
            if first {
                if let Some(entry) = state.displays.get_mut(&display) {
                    entry.activation = entry.activation.wrapping_add(1);
                }
            }
            (id, first)
        };
        if first {
            if let Err(error) = stream.set_active(true) {
                lock(&self.inner.shared.state).hub.remove(&display, id);
                self.inner.shared.fail(display, false);
                let display_id = display;
                tracing::warn!(display_id, %error, "cannot activate PipeWire capture");
                return Err(display_unavailable());
            }
        }
        {
            let mut state = lock(&self.inner.shared.state);
            if state.displays.get(&display).is_none_or(|entry| entry.bad) {
                state.hub.remove(&display, id);
                return Err(display_unavailable());
            }
        }
        let inner = self.inner.clone();
        Ok(Box::new(HubHandle::new(move || inner.remove(display, id))))
    }

    pub fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        let mut state = lock(&self.inner.shared.state);
        if state.metadata_cursor {
            // The new receiver only learns the current shape from the snapshot.
            // Previously seen shapes must be sent in full before future Use IDs.
            state.cursor.reset_shapes();
            let snapshot = state.cursor.snapshot();
            if snapshot.is_empty() {
                let _ = tx.try_send(CursorUpdate::Hidden(true));
            }
            for update in snapshot {
                let _ = tx.try_send(update);
            }
        } else {
            // An embedded cursor must not also be drawn by the remote client.
            let _ = tx.try_send(CursorUpdate::Hidden(true));
        }
        state.cursor_watchers.push(tx);
    }

    pub fn watch_pointer(&self, tx: mpsc::Sender<RawPointer>) {
        lock(&self.inner.shared.state).pointer_watchers.push(tx);
    }

    /// Becomes true once every stream has disconnected or entered ERROR.
    pub fn lost(&self) -> watch::Receiver<bool> {
        self.inner.shared.lost.subscribe()
    }

    pub fn stats(&self, display: u32) -> Option<WlStats> {
        lock(&self.inner.shared.state)
            .displays
            .get(&display)
            .map(|entry| entry.stats)
    }
}

impl Inner {
    fn remove(&self, display: u32, id: u64) {
        let streams = lock(&self.streams);
        let stop = {
            let mut state = lock(&self.shared.state);
            state.hub.remove(&display, id);
            // deliver() may already have pruned a closed receiver.
            let stop = state.hub.effective(&display).is_none();
            if stop {
                if let Some(entry) = state.displays.get_mut(&display) {
                    entry.stats.active = false;
                }
                state.cursor.display_stopped(display);
            }
            stop
        };
        if stop {
            if let Some(stream) = streams.get(&display) {
                if let Err(error) = stream.set_active(false) {
                    self.shared.fail(display, false);
                    let display_id = display;
                    tracing::warn!(display_id, %error, "cannot deactivate PipeWire capture");
                }
            }
        }
    }
}

fn stream_spec(display: &StreamDisplay) -> StreamSpec {
    let size = if display.width == 0 || display.height == 0 {
        (1920, 1080)
    } else {
        (display.width.min(MAX_SIDE), display.height.min(MAX_SIDE))
    };
    StreamSpec {
        name: format!("capture-{}", display.id),
        props: vec![
            ("media.type".into(), "Video".into()),
            ("media.category".into(), "Capture".into()),
            ("media.role".into(), "Screen".into()),
        ],
        direction: Direction::Input,
        target: Some(display.node_id),
        flags: stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS | stream_flag::INACTIVE,
        params: vec![spa_pod::video_enum_format(
            &[
                video_format::BGRX,
                video_format::BGRA,
                video_format::RGBX,
                video_format::RGBA,
            ],
            size,
            (MAX_SIDE, MAX_SIDE),
            60,
        )],
    }
}

fn buffer_params(metadata_cursor: bool) -> Vec<Vec<u8>> {
    let mut params = vec![
        spa_pod::video_buffers((1 << data_type::MEM_FD) | (1 << data_type::MEM_PTR)),
        spa_pod::meta_param(meta_type::HEADER, MetaSize::Fixed(32)),
        spa_pod::meta_param(
            meta_type::VIDEO_DAMAGE,
            MetaSize::Range {
                default: 16 * 16,
                min: 16,
                max: 16 * 16,
            },
        ),
    ];
    if metadata_cursor {
        params.push(spa_pod::meta_param(
            meta_type::CURSOR,
            MetaSize::Range {
                default: spa_pod::cursor_meta_size(64, 64),
                min: spa_pod::cursor_meta_size(1, 1),
                max: spa_pod::cursor_meta_size(256, 256),
            },
        ));
    }
    params
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Format {
    pixels: SrcFormat,
    width: u32,
    height: u32,
}

impl Format {
    fn parse(pod: &[u8]) -> Option<Self> {
        let format = spa_pod::parse_video_format(pod).ok()?;
        if format.width == 0
            || format.height == 0
            || format.width > MAX_SIDE
            || format.height > MAX_SIDE
        {
            return None;
        }
        let pixels = match format.format {
            video_format::BGRX => SrcFormat::Bgrx,
            video_format::BGRA => SrcFormat::Bgra,
            video_format::RGBX => SrcFormat::Rgbx,
            video_format::RGBA => SrcFormat::Rgba,
            _ => return None,
        };
        Some(Self {
            pixels,
            width: format.width,
            height: format.height,
        })
    }
}

struct VideoHandler {
    shared: Arc<Shared>,
    display: u32,
    format: Option<Format>,
    generation: u64,
    activation: u64,
    gate: FrameGate,
    pool: FramePool,
    metadata_cursor: bool,
    cursor_position: Option<(bool, i32, i32, i32, i32)>,
    cursor_bitmap: Vec<u8>,
    cursor_shape: Option<pb::CursorShape>,
}

struct BufferView<'a> {
    data: Option<DataView<'a>>,
    header: Option<&'a [u8]>,
    damage: Option<&'a [u8]>,
    cursor: Option<&'a [u8]>,
}

impl VideoHandler {
    fn new(shared: Arc<Shared>, display: u32) -> Self {
        let metadata_cursor = lock(&shared.state).metadata_cursor;
        Self {
            shared,
            display,
            format: None,
            generation: 0,
            activation: 0,
            gate: FrameGate::new(),
            pool: FramePool::new(POOL_SIZE),
            metadata_cursor,
            cursor_position: None,
            cursor_bitmap: Vec::with_capacity(if metadata_cursor { MAX_CURSOR_META } else { 0 }),
            cursor_shape: None,
        }
    }

    fn configure(&mut self, format: Format) {
        if self.format == Some(format) {
            return;
        }
        self.generation = self.generation.saturating_add(1);
        self.format = Some(format);
        self.gate.reset();
        self.pool.clear();
        // Hold all four slots while warming them; otherwise acquire would keep
        // reusing the first slot. Large pixel allocations happen only on FORMAT.
        let _warm: Vec<_> = (0..POOL_SIZE)
            .filter_map(|_| {
                self.pool.acquire(
                    format.width,
                    format.height,
                    format.width * 4,
                    self.generation,
                    |_| {},
                )
            })
            .collect();
        let mut state = lock(&self.shared.state);
        if let Some(display) = state.displays.get_mut(&self.display) {
            display.info.width_px = format.width;
            display.info.height_px = format.height;
            display.info.scale = if display.logical_width == 0 {
                1.0
            } else {
                format.width as f32 / display.logical_width as f32
            };
        }
        state.notify_displays();
    }

    fn consume(&mut self, buffer: BufferView<'_>, counts: &mut WlStats) {
        let activation = {
            let state = lock(&self.shared.state);
            let Some(display) = state
                .displays
                .get(&self.display)
                .filter(|display| !display.bad && display.stats.active)
            else {
                counts.skipped += 1;
                return;
            };
            display.activation
        };
        if activation != self.activation {
            self.activation = activation;
            self.gate.reset();
        }
        let Some(format) = self.format else {
            counts.bad_buffers += 1;
            return;
        };
        let cursor_changed = self.metadata_cursor
            && buffer
                .cursor
                .is_some_and(|bytes| self.cursor_update(bytes, format, counts));
        let info = BufferInfo {
            header_flags: buffer
                .header
                .and_then(spa_pod::parse_meta_header)
                .map(|header| header.flags),
            damage_regions: buffer
                .damage
                .map(|bytes| spa_pod::parse_damage(bytes).len()),
            chunk_size: buffer.data.as_ref().map_or(0, |data| data.chunk.size),
            chunk_flags: buffer.data.as_ref().map_or(0, |data| data.chunk.flags),
            cursor_changed,
        };
        let previous_gate = self.gate;
        match self.gate.decide(&info) {
            FrameDecision::CursorOnly => counts.cursor_only += 1,
            FrameDecision::Skip => counts.skipped += 1,
            FrameDecision::Encode => {
                let Some(data) = buffer.data.filter(|data| {
                    matches!(data.data_type, data_type::MEM_FD | data_type::MEM_PTR)
                }) else {
                    counts.bad_buffers += 1;
                    self.gate = previous_gate;
                    return;
                };
                let Some(bytes) = data.valid() else {
                    counts.bad_buffers += 1;
                    self.gate = previous_gate;
                    return;
                };
                let stride = format.width * 4;
                let source_stride = if data.chunk.stride <= 0 {
                    stride as usize
                } else {
                    data.chunk.stride as usize
                };
                let mut copied = false;
                let frame = self.pool.acquire(
                    format.width,
                    format.height,
                    stride,
                    self.generation,
                    |dst| {
                        copied = copy_to_bgrx(
                            bytes,
                            source_stride,
                            format.pixels,
                            format.width,
                            format.height,
                            dst,
                            stride as usize,
                        );
                    },
                );
                let Some(frame) = frame else {
                    counts.pool_full += 1;
                    self.gate = previous_gate;
                    return;
                };
                if !copied {
                    counts.bad_buffers += 1;
                    self.gate = previous_gate;
                    return;
                }
                let frame = CapturedFrame {
                    display_id: self.display,
                    width: format.width,
                    height: format.height,
                    capture_ts_us: unix_micros(),
                    native: frame,
                };
                lock(&self.shared.state).hub.deliver(&self.display, &frame);
                counts.encoded += 1;
            }
        }
    }

    fn cursor_update(&mut self, bytes: &[u8], format: Format, counts: &mut WlStats) -> bool {
        if bytes.len() < spa_pod::META_CURSOR_SIZE || bytes.len() > MAX_CURSOR_META {
            counts.bad_buffers += 1;
            return false;
        }
        // parse_cursor intentionally returns None for id=0; that is a hide,
        // not absent metadata. Preserve the last hotspot position while hidden.
        let visible = bytes[..4] != [0; 4];
        let (x, y, hot_x, hot_y) = if visible {
            let has_bitmap = bytes[24..28] != [0; 4];
            // Ignore position/id when checking a repeated bitmap. Parse only
            // the header on moves, avoiding pixel copies and PNG compression.
            let new_bitmap = has_bitmap && self.cursor_bitmap != bytes[16..];
            let input = if new_bitmap {
                bytes
            } else {
                &bytes[..spa_pod::META_CURSOR_SIZE]
            };
            let Some(meta) = spa_pod::parse_cursor(input) else {
                counts.bad_buffers += 1;
                return false;
            };
            if new_bitmap {
                let shape = meta.bitmap.as_ref().and_then(|bitmap| {
                    let pixels = cursor_pixels_from_spa(bitmap.format)?;
                    cursor_to_shape(
                        pixels,
                        bitmap.width,
                        bitmap.height,
                        bitmap.stride,
                        &bitmap.pixels,
                        meta.hot_x,
                        meta.hot_y,
                        1.0,
                        true,
                    )
                    .ok()
                });
                if let Some(shape) = shape {
                    self.cursor_shape = Some(shape);
                    self.cursor_bitmap.clear();
                    self.cursor_bitmap.extend_from_slice(&bytes[16..]);
                } else {
                    counts.bad_buffers += 1;
                }
            }
            (meta.x, meta.y, meta.hot_x, meta.hot_y)
        } else {
            let (_, x, y, hot_x, hot_y) = self.cursor_position.unwrap_or_default();
            (x, y, hot_x, hot_y)
        };
        let position = (visible, x, y, hot_x, hot_y);
        let mut changed = self.cursor_position != Some(position);
        self.cursor_position = Some(position);
        let mut state = lock(&self.shared.state);
        if visible {
            if let Some(shape) = &self.cursor_shape {
                if let Some(update) = state.cursor.on_shape(shape.clone()) {
                    changed = true;
                    broadcast(&mut state.cursor_watchers, &update);
                }
            }
        }
        state.cursor_seq = state.cursor_seq.saturating_add(1);
        let report = pointer_report(
            self.display,
            state.cursor_seq,
            visible,
            (x, y),
            (hot_x, hot_y),
            (format.width, format.height),
        );
        for update in state.cursor.on_report(&report) {
            changed = true;
            broadcast(&mut state.cursor_watchers, &update);
        }
        broadcast(
            &mut state.pointer_watchers,
            &RawPointer {
                display_id: self.display,
                nx: (f64::from(x) / f64::from(format.width)).clamp(0.0, 1.0),
                ny: (f64::from(y) / f64::from(format.height)).clamp(0.0, 1.0),
                visible,
            },
        );
        changed
    }
}

impl StreamHandler for VideoHandler {
    fn state_changed(&mut self, _old: i32, new: i32, _error: Option<&str>) {
        if matches!(new, state::ERROR | state::UNCONNECTED) {
            self.shared.fail(self.display, true);
        }
    }

    fn param_changed(&mut self, ctl: &mut StreamCtl<'_>, id: u32, pod: Option<&[u8]>) {
        if id != param::FORMAT {
            return;
        }
        let Some(pod) = pod else {
            self.format = None;
            self.gate.reset();
            self.pool.clear();
            return;
        };
        let Some(format) = Format::parse(pod) else {
            self.shared.fail(self.display, false);
            tracing::warn!(display = self.display, "invalid PipeWire video format");
            return;
        };
        if let Err(error) = ctl.update_params(&buffer_params(self.metadata_cursor)) {
            self.shared.fail(self.display, false);
            tracing::warn!(display = self.display, %error, "cannot negotiate PipeWire buffers");
            return;
        }
        self.configure(format);
    }

    fn process(&mut self, ctl: &mut StreamCtl<'_>) {
        let mut latest = None;
        let mut counts = WlStats::default();
        while let Some(buffer) = ctl.dequeue() {
            counts.buffers += 1;
            if let Some(previous) = latest.replace(buffer) {
                ctl.queue(previous);
                counts.skipped += 1;
            }
        }
        if let Some(buffer) = latest {
            self.consume(
                BufferView {
                    data: buffer.data(0),
                    header: buffer.meta(meta_type::HEADER),
                    damage: buffer.meta(meta_type::VIDEO_DAMAGE),
                    cursor: buffer.meta(meta_type::CURSOR),
                },
                &mut counts,
            );
            ctl.queue(buffer);
        }
        if let Some(display) = lock(&self.shared.state).displays.get_mut(&self.display) {
            let stats = &mut display.stats;
            stats.buffers = stats.buffers.saturating_add(counts.buffers);
            stats.encoded = stats.encoded.saturating_add(counts.encoded);
            stats.cursor_only = stats.cursor_only.saturating_add(counts.cursor_only);
            stats.skipped = stats.skipped.saturating_add(counts.skipped);
            stats.pool_full = stats.pool_full.saturating_add(counts.pool_full);
            stats.bad_buffers = stats.bad_buffers.saturating_add(counts.bad_buffers);
        }
    }
}

fn display_unavailable() -> PlatformError {
    PlatformError::Platform("display not available".into())
}

fn broadcast<T: Clone>(watchers: &mut Vec<mpsc::Sender<T>>, value: &T) {
    watchers.retain(|tx| match tx.try_send(value.clone()) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    });
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_micros().min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::LinuxFrame;
    use crate::pw::Chunk;
    use crate::spa_pod::{buffers_key, format_key, meta_key, Value};

    const CONFIG: CaptureConfig = CaptureConfig {
        fps: 30,
        max_width: 0,
        max_height: 0,
    };

    fn display(id: u32) -> StreamDisplay {
        StreamDisplay {
            id,
            node_id: id + 100,
            x: -100,
            y: 20,
            width: 2,
            height: 2,
            primary: id == 1,
        }
    }

    fn test_capture(displays: &[StreamDisplay], metadata: bool) -> WaylandCapture {
        WaylandCapture {
            inner: Arc::new(Inner {
                streams: Mutex::new(BTreeMap::new()),
                shared: Arc::new(Shared::new(
                    displays
                        .iter()
                        .map(|display| (display.id, DisplayState::new(display)))
                        .collect(),
                    metadata,
                )),
            }),
        }
    }

    fn start(capture: &WaylandCapture, id: u32) -> (HubHandle, mpsc::Receiver<CapturedFrame>) {
        let (tx, rx) = mpsc::channel(8);
        let mut state = lock(&capture.inner.shared.state);
        let (sink, first) = state.hub.add(id, tx, CONFIG);
        let display = state.displays.get_mut(&id).unwrap();
        display.stats.active = true;
        if first {
            display.activation += 1;
        }
        let inner = capture.inner.clone();
        (HubHandle::new(move || inner.remove(id, sink)), rx)
    }

    fn handler(capture: &WaylandCapture, id: u32, pixels: SrcFormat) -> VideoHandler {
        let mut handler = VideoHandler::new(capture.inner.shared.clone(), id);
        handler.configure(Format {
            pixels,
            width: 2,
            height: 2,
        });
        handler
    }

    fn buffer(bytes: &[u8]) -> BufferView<'_> {
        BufferView {
            data: Some(DataView {
                data_type: data_type::MEM_FD,
                flags: 0,
                fd: -1,
                mapoffset: 0,
                maxsize: bytes.len() as u32,
                bytes: Some(bytes),
                chunk: Chunk {
                    offset: 0,
                    size: bytes.len() as u32,
                    stride: 0,
                    flags: 0,
                },
            }),
            header: None,
            damage: None,
            cursor: None,
        }
    }

    fn cursor_meta(x: i32, y: i32) -> Vec<u8> {
        let mut bytes = vec![0; spa_pod::cursor_meta_size(2, 2) as usize];
        for (offset, value) in [
            (0, 1),
            (8, x as u32),
            (12, y as u32),
            (16, 1),
            (20, 1),
            (24, 28),
            (28, video_format::BGRA),
            (32, 2),
            (36, 2),
            (40, 8),
            (44, 20),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for pixel in bytes[48..].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[20, 30, 40, 255]);
        }
        bytes
    }

    #[test]
    fn negotiation_requests_shm_and_bounded_metadata() {
        let spec = stream_spec(&display(1));
        assert_eq!(spec.direction, Direction::Input);
        assert_eq!(spec.target, Some(101));
        assert_eq!(
            spec.flags,
            stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS | stream_flag::INACTIVE
        );
        let format = spa_pod::parse_object(&spec.params[0]).unwrap();
        let Some(Value::Choice { values, .. }) = format.get(format_key::VIDEO_FORMAT) else {
            panic!("format enum")
        };
        assert_eq!(
            values,
            [8, 8, 12, 7, 11]
                .into_iter()
                .map(Value::Id)
                .collect::<Vec<_>>()
        );
        let mut unknown_size = display(1);
        unknown_size.width = 0;
        let spec = stream_spec(&unknown_size);
        let parsed = spa_pod::parse_video_format(&spec.params[0]).unwrap();
        assert_eq!((parsed.width, parsed.height), (1920, 1080));
        let params = buffer_params(true);
        assert_eq!(params.len(), 4);
        assert_eq!(
            spa_pod::parse_object(&params[0])
                .unwrap()
                .get(buffers_key::DATA_TYPE),
            Some(Value::Choice {
                kind: spa_pod::choice::FLAGS,
                values: vec![Value::Int(6)]
            })
        );
        assert_eq!(
            spa_pod::parse_object(&params[3])
                .unwrap()
                .get(meta_key::SIZE),
            Some(Value::Choice {
                kind: spa_pod::choice::RANGE,
                values: vec![
                    Value::Int(spa_pod::cursor_meta_size(64, 64)),
                    Value::Int(spa_pod::cursor_meta_size(1, 1)),
                    Value::Int(spa_pod::cursor_meta_size(256, 256))
                ]
            })
        );
        assert_eq!(buffer_params(false).len(), 3);
        for (pixel_format, size) in [
            (video_format::ARGB, (2, 2)),
            (video_format::BGRX, (0, 2)),
            (video_format::BGRX, (8193, 2)),
        ] {
            assert!(Format::parse(&spa_pod::video_enum_format(
                &[pixel_format],
                size,
                (MAX_SIDE, MAX_SIDE),
                60
            ))
            .is_none());
        }
    }

    #[test]
    fn copies_chunk_offset_and_stride_and_fans_out_one_native_frame() {
        let capture = test_capture(&[display(1)], true);
        let (handle1, mut rx1) = start(&capture, 1);
        let (handle2, mut rx2) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Rgba);
        let pixels = [
            99, 99, 99, 99, 1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 9, 10, 11, 12, 13, 14, 15, 16,
        ];
        let mut frame = buffer(&pixels);
        frame.data.as_mut().unwrap().chunk.offset = 4;
        frame.data.as_mut().unwrap().chunk.size = 20;
        frame.data.as_mut().unwrap().chunk.stride = 12;
        frame.damage = Some(&[0; 16]);
        let mut counts = WlStats::default();
        handler.consume(frame, &mut counts);
        let first = rx1.try_recv().unwrap();
        let second = rx2.try_recv().unwrap();
        assert!(Arc::ptr_eq(&first.native, &second.native));
        let native = LinuxFrame::of(&first).unwrap();
        assert_eq!(native.row(0).unwrap(), [3, 2, 1, 4, 7, 6, 5, 8]);
        assert_eq!(native.row(1).unwrap(), [11, 10, 9, 12, 15, 14, 13, 16]);
        assert_eq!(native.generation, 1);
        assert_eq!(counts.encoded, 1);
        drop(handle1);
        assert!(capture.stats(1).unwrap().active);
        handler.consume(buffer(&[1; 16]), &mut counts);
        assert!(rx2.try_recv().is_ok());
        assert!(rx1.try_recv().is_err());
        drop(handle2);
        assert!(!capture.stats(1).unwrap().active);
        let mut clean = buffer(&[1; 16]);
        clean.damage = Some(&[0; 16]);
        let (_handle, mut rx) = start(&capture, 1);
        handler.consume(clean, &mut counts);
        assert!(rx.try_recv().is_ok(), "reactivation requires a fresh frame");
    }

    #[test]
    fn bad_first_buffer_does_not_consume_the_first_frame_gate() {
        let capture = test_capture(&[display(1)], false);
        let (_handle, mut rx) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        let mut counts = WlStats::default();
        let mut truncated = buffer(&[1; 4]);
        truncated.damage = Some(&[0; 16]);
        handler.consume(truncated, &mut counts);
        assert_eq!(counts.bad_buffers, 1);
        assert!(rx.try_recv().is_err());
        let mut frame = buffer(&[5; 16]);
        frame.damage = Some(&[0; 16]);
        frame.data.as_mut().unwrap().chunk.stride = -8;
        handler.consume(frame, &mut counts);
        assert_eq!(counts.encoded, 1);
        assert_eq!(
            LinuxFrame::of(&rx.try_recv().unwrap()).unwrap().pixels,
            [5; 16]
        );
        let mut clean = buffer(&[1; 16]);
        clean.damage = Some(&[0; 16]);
        handler.consume(clean, &mut counts);
        let mut header = [0; 32];
        header[..4].copy_from_slice(&spa_pod::header_flag::CORRUPTED.to_le_bytes());
        let mut corrupt = buffer(&[1; 16]);
        corrupt.header = Some(&header);
        handler.consume(corrupt, &mut counts);
        let mut empty = buffer(&[1; 16]);
        empty.data.as_mut().unwrap().chunk.flags = spa_pod::chunk_flag::EMPTY;
        handler.consume(empty, &mut counts);
        assert_eq!(counts.skipped, 3);
        assert_eq!(counts.encoded, 1);
    }

    #[test]
    fn unmapped_or_non_shm_buffers_are_rejected_without_losing_first_frame() {
        let capture = test_capture(&[display(1)], false);
        let (_handle, mut rx) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Bgra);
        let mut counts = WlStats::default();
        let mut unmapped = buffer(&[1; 16]);
        unmapped.data.as_mut().unwrap().bytes = None;
        handler.consume(unmapped, &mut counts);
        let mut dmabuf = buffer(&[1; 16]);
        dmabuf.data.as_mut().unwrap().data_type = data_type::DMA_BUF;
        handler.consume(dmabuf, &mut counts);
        let mut bad_offset = buffer(&[1; 16]);
        bad_offset.data.as_mut().unwrap().chunk.offset = 1;
        handler.consume(bad_offset, &mut counts);
        let mut frame = buffer(&[1; 16]);
        frame.damage = Some(&[0; 16]);
        handler.consume(frame, &mut counts);
        assert_eq!(counts.bad_buffers, 3);
        assert_eq!(counts.encoded, 1);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn exhausted_pool_drops_until_a_slot_is_released() {
        let capture = test_capture(&[display(1)], false);
        let (_handle, mut rx) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        assert_eq!(handler.pool.len(), 4, "all slots preallocated on FORMAT");
        let mut counts = WlStats::default();
        let mut held = Vec::new();
        for value in 0..4 {
            handler.consume(buffer(&[value; 16]), &mut counts);
            held.push(rx.try_recv().unwrap());
        }
        handler.consume(buffer(&[9; 16]), &mut counts);
        assert_eq!(counts.pool_full, 1);
        assert_eq!(counts.encoded, 4);
        assert!(rx.try_recv().is_err());
        drop(held.pop());
        handler.consume(buffer(&[10; 16]), &mut counts);
        assert_eq!(
            LinuxFrame::of(&rx.try_recv().unwrap()).unwrap().pixels,
            [10; 16]
        );
        assert_eq!(handler.pool.len(), 4);
        assert_eq!(LinuxFrame::of(&held[0]).unwrap().pixels, [0; 16]);
    }

    #[test]
    fn renegotiation_notifies_scale_and_preserves_outstanding_frames() {
        let capture = test_capture(&[display(1)], false);
        let (_handle, mut rx) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        let mut counts = WlStats::default();
        handler.consume(buffer(&[1; 16]), &mut counts);
        let old = rx.try_recv().unwrap();
        let (tx, mut displays) = mpsc::channel(4);
        capture.watch_displays(tx);
        let original = displays.try_recv().unwrap().remove(0);
        assert_eq!((original.x, original.y, original.scale), (-100, 20, 1.0));
        let format = Format {
            pixels: SrcFormat::Rgbx,
            width: 4,
            height: 3,
        };
        handler.configure(format);
        let resized = displays.try_recv().unwrap().remove(0);
        assert_eq!(
            (resized.width_px, resized.height_px, resized.scale),
            (4, 3, 2.0)
        );
        handler.configure(format);
        assert!(displays.try_recv().is_err());
        assert_eq!(handler.generation, 2);
        let mut fresh = buffer(&[2; 48]);
        fresh.damage = Some(&[0; 16]);
        handler.consume(fresh, &mut counts);
        let frame = rx.try_recv().unwrap();
        let native = LinuxFrame::of(&frame).unwrap();
        assert_eq!(
            (
                native.width,
                native.height,
                native.stride,
                native.generation
            ),
            (4, 3, 16, 2)
        );
        assert_eq!(LinuxFrame::of(&old).unwrap().pixels, [1; 16]);
        handler.configure(Format {
            pixels: SrcFormat::Rgba,
            ..format
        });
        assert_eq!(
            handler.generation, 3,
            "pixel-format changes also invalidate frames"
        );
        assert!(displays.try_recv().is_ok());
    }

    #[test]
    fn metadata_cursor_moves_and_hides_without_encoding_clean_frames() {
        let capture = test_capture(&[display(1)], true);
        let (_handle, mut frames) = start(&capture, 1);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        let (tx, mut cursors) = mpsc::channel(16);
        capture.watch_cursor(tx);
        assert_eq!(cursors.try_recv().unwrap(), CursorUpdate::Hidden(true));
        let (tx, mut pointers) = mpsc::channel(8);
        capture.watch_pointer(tx);
        let mut counts = WlStats::default();
        let mut metadata = cursor_meta(1, 1);
        let mut send = |metadata: &[u8]| {
            let mut frame = buffer(&[1; 16]);
            frame.damage = Some(&[0; 16]);
            frame.cursor = Some(metadata);
            handler.consume(frame, &mut counts);
        };
        send(&metadata);
        let CursorUpdate::Shape(shape) = cursors.try_recv().unwrap() else {
            panic!("new cursor shape")
        };
        assert_eq!(
            (shape.width, shape.height, shape.hot_x, shape.hot_y),
            (2, 2, 1, 1)
        );
        assert!(matches!(
            cursors.try_recv().unwrap(),
            CursorUpdate::Position {
                x: 0.5,
                y: 0.5,
                visible: true,
                ..
            }
        ));
        assert_eq!(cursors.try_recv().unwrap(), CursorUpdate::Hidden(false));
        assert_eq!(
            pointers.try_recv().unwrap(),
            RawPointer {
                display_id: 1,
                nx: 0.5,
                ny: 0.5,
                visible: true
            }
        );
        send(&metadata);
        assert!(
            cursors.try_recv().is_err(),
            "unchanged bitmap/position is deduplicated"
        );
        metadata[8..12].copy_from_slice(&2_i32.to_le_bytes());
        send(&metadata);
        assert!(matches!(
            cursors.try_recv().unwrap(),
            CursorUpdate::Position { x: 1.0, y: 0.5, .. }
        ));
        metadata[48] = 30;
        send(&metadata);
        assert!(matches!(
            cursors.try_recv().unwrap(),
            CursorUpdate::Shape(_)
        ));
        send(&[0; 28]);
        assert!(matches!(
            cursors.try_recv().unwrap(),
            CursorUpdate::Position { visible: false, .. }
        ));
        assert_eq!(cursors.try_recv().unwrap(), CursorUpdate::Hidden(true));
        send(&[0; 28]);
        assert!(cursors.try_recv().is_err());
        assert_eq!(
            (counts.encoded, counts.cursor_only, counts.skipped),
            (1, 3, 2)
        );
        assert_eq!(counts.bad_buffers, 0);
        assert!(frames.try_recv().is_ok());
        assert!(frames.try_recv().is_err());
        let (tx, mut snapshot) = mpsc::channel(4);
        capture.watch_cursor(tx);
        assert!(matches!(
            snapshot.try_recv().unwrap(),
            CursorUpdate::Shape(_)
        ));
        assert!(matches!(
            snapshot.try_recv().unwrap(),
            CursorUpdate::Position { visible: false, .. }
        ));
        assert_eq!(snapshot.try_recv().unwrap(), CursorUpdate::Hidden(true));
    }

    #[test]
    fn cursor_watchers_drop_full_updates_and_remove_closed_channels() {
        let capture = test_capture(&[display(1)], true);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        let (tx, mut full) = mpsc::channel(1);
        capture.watch_cursor(tx);
        let (tx, closed) = mpsc::channel(1);
        capture.watch_cursor(tx);
        drop(closed);
        let (tx, closed) = mpsc::channel(1);
        capture.watch_pointer(tx);
        drop(closed);
        let mut counts = WlStats::default();
        assert!(handler.cursor_update(&cursor_meta(1, 1), handler.format.unwrap(), &mut counts));
        assert_eq!(full.try_recv().unwrap(), CursorUpdate::Hidden(true));
        assert_eq!(lock(&capture.inner.shared.state).cursor_watchers.len(), 1);
        assert!(lock(&capture.inner.shared.state)
            .pointer_watchers
            .is_empty());
        assert!(!handler.cursor_update(&[1, 2], handler.format.unwrap(), &mut counts));
        assert_eq!(counts.bad_buffers, 1);
        let embedded = test_capture(&[display(1)], false);
        let (tx, mut rx) = mpsc::channel(1);
        embedded.watch_cursor(tx);
        assert_eq!(rx.try_recv().unwrap(), CursorUpdate::Hidden(true));
    }

    #[test]
    fn new_cursor_subscriber_gets_old_shapes_before_use_references() {
        let capture = test_capture(&[display(1)], true);
        let mut handler = handler(&capture, 1, SrcFormat::Bgrx);
        let mut counts = WlStats::default();
        let first = cursor_meta(1, 1);
        let mut second = first.clone();
        second[48] = 60;
        handler.cursor_update(&first, handler.format.unwrap(), &mut counts);
        handler.cursor_update(&second, handler.format.unwrap(), &mut counts);
        let (tx, mut rx) = mpsc::channel(4);
        capture.watch_cursor(tx);
        assert!(matches!(rx.try_recv().unwrap(), CursorUpdate::Shape(_)));
        assert!(matches!(
            rx.try_recv().unwrap(),
            CursorUpdate::Position { .. }
        ));
        handler.cursor_update(&first, handler.format.unwrap(), &mut counts);
        assert!(matches!(rx.try_recv().unwrap(), CursorUpdate::Shape(_)));
    }

    #[test]
    fn loss_requires_all_streams_and_is_retained_for_late_subscribers() {
        let capture = test_capture(&[display(1), display(2)], true);
        let mut first = handler(&capture, 1, SrcFormat::Bgrx);
        let mut second = handler(&capture, 2, SrcFormat::Bgrx);
        first.state_changed(state::UNCONNECTED, state::CONNECTING, None);
        assert!(!*capture.lost().borrow());
        first.state_changed(state::STREAMING, state::ERROR, Some("closed"));
        assert!(!*capture.lost().borrow());
        assert_eq!(capture.displays().len(), 1);
        assert!(!capture.stats(1).unwrap().active);
        second.state_changed(state::PAUSED, state::UNCONNECTED, None);
        assert!(*capture.lost().borrow());
        assert!(capture.displays().is_empty());
        let (tx, _rx) = mpsc::channel(1);
        assert!(
            matches!(capture.capture(99, CONFIG, tx), Err(PlatformError::Platform(message)) if message == "display not available")
        );
        assert!(capture.stats(99).is_none());
    }
}
