//! Session-side dispatch for the service/session pipe.
//!
//! The service owns the pipe and the handshake.  Once a session has been
//! accepted, this module translates control frames into calls on the platform
//! boundary and translates the asynchronous platform callbacks back into pipe
//! frames.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use host_core::platform::{
    AudioCaptureConfig, CaptureConfig, CaptureHandle, CapturedFrame, ClipboardItem, EncodeOpts,
    Encoder, EncoderConfig, PixelRect, Platform,
};
use rsp_core::pb;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tracing::debug;

use crate::pipe_proto::{
    cursor_to_ctl, input_from_b64, BlobKind, Ctl, DisplayWire, PipeFrame, RoiWire, VideoPacket,
    WatchKind, WireError,
};
use crate::viewport_crop;

/// Number of captured frames retained for a later encode request.
pub const FRAME_RING: usize = 4;

/// A frame ring is shared by the capture forwarding task and `Encode` calls.
struct FrameRing {
    next_seq: u64,
    frames: VecDeque<(u64, CapturedFrame)>,
}

impl FrameRing {
    fn new() -> Self {
        Self {
            next_seq: 0,
            frames: VecDeque::with_capacity(FRAME_RING),
        }
    }

    fn push(&mut self, frame: CapturedFrame) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        let seq = self.next_seq;
        if self.frames.len() == FRAME_RING {
            self.frames.pop_front();
        }
        self.frames.push_back((seq, frame));
        seq
    }

    fn get(&self, seq: u64) -> Option<CapturedFrame> {
        self.frames
            .iter()
            .find_map(|(stored_seq, frame)| (*stored_seq == seq).then(|| frame.clone()))
    }
}

struct CaptureEntry {
    handle: Box<dyn CaptureHandle>,
    abort: AbortHandle,
    ring: Arc<Mutex<FrameRing>>,
    token: Arc<()>,
    region: Option<(PixelRect, u32, u32)>,
}

struct EncoderEntry {
    encoder: Box<dyn Encoder>,
    abort: AbortHandle,
}

struct AudioEntry {
    handle: Box<dyn CaptureHandle>,
    abort: AbortHandle,
    token: Arc<()>,
}

struct WatchEntry {
    enabled: bool,
    abort: Option<AbortHandle>,
}

#[derive(Default)]
struct ServerState {
    started: bool,
    shutdown: bool,
    displays: Vec<host_core::platform::DisplayInfo>,
    display_abort: Option<AbortHandle>,
    captures: HashMap<u32, CaptureEntry>,
    encoders: HashMap<u32, EncoderEntry>,
    audio: Option<AudioEntry>,
    watches: HashMap<WatchKind, WatchEntry>,
    keep_awake: bool,
}

#[derive(Default)]
struct Stats {
    invalid_inputs: AtomicU64,
    dropped_encodes: AtomicU64,
}

struct EncoderOpenArgs {
    req: u32,
    enc: u32,
    raw_codec: i32,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
}

fn valid_region(region: crate::pipe_proto::RegionWire, width: u32, height: u32) -> bool {
    region.w >= 2
        && region.h >= 2
        && region
            .x
            .checked_add(region.w)
            .is_some_and(|right| right <= width)
        && region
            .y
            .checked_add(region.h)
            .is_some_and(|bottom| bottom <= height)
}

/// Dispatches service-to-session pipe messages to a concrete platform.
pub struct SessionServer<P: Platform> {
    platform: Arc<P>,
    out: mpsc::UnboundedSender<PipeFrame>,
    state: Arc<Mutex<ServerState>>,
    stats: Arc<Stats>,
}

impl<P: Platform> SessionServer<P> {
    /// Create a server.  Call [`Self::start`] once after the pipe handshake.
    pub fn new(platform: Arc<P>, out: mpsc::UnboundedSender<PipeFrame>) -> Self {
        Self {
            platform,
            out,
            state: Arc::new(Mutex::new(ServerState::default())),
            stats: Arc::new(Stats::default()),
        }
    }

    /// Send the initial display list and begin watching for display changes.
    pub fn start(&mut self) {
        let displays = self.platform.displays();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.started || state.shutdown {
                return;
            }
            state.started = true;
            state.displays = displays.clone();
        }
        self.send_displays(&displays);

        let (tx, mut rx) = mpsc::channel::<Vec<host_core::platform::DisplayInfo>>(16);
        self.platform.watch_displays(tx);
        let state = Arc::clone(&self.state);
        let out = self.out.clone();
        let task = tokio::spawn(async move {
            while let Some(displays) = rx.recv().await {
                let changed = {
                    let mut state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if state.displays == displays {
                        false
                    } else {
                        state.displays = displays.clone();
                        true
                    }
                };
                if changed {
                    let list = displays.iter().map(DisplayWire::from).collect();
                    let _ = out.send(PipeFrame::Ctl(Ctl::Displays { list }));
                }
            }
        });
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.display_abort = Some(task.abort_handle());
    }

    /// Dispatch one incoming pipe frame.
    pub fn handle(&mut self, frame: PipeFrame) {
        if self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .shutdown
        {
            return;
        }

        match frame {
            PipeFrame::Ctl(ctl) => self.handle_ctl(ctl),
            PipeFrame::Blob(blob) => self.handle_blob(blob.req, blob.kind, blob.data),
            PipeFrame::Video(_) | PipeFrame::Audio(_) => {
                debug!("discarding media frame received by the session server")
            }
        }
    }

    /// Run until the service closes its input side, then release all platform resources.
    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<PipeFrame>) {
        while let Some(frame) = rx.recv().await {
            self.handle(frame);
        }
        self.shutdown();
    }

    /// Stop all callbacks and release platform state.
    pub fn shutdown(&mut self) {
        let (captures, encoders, audio, display_abort, watch_aborts, keep_awake) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.shutdown {
                return;
            }
            state.shutdown = true;
            let captures = std::mem::take(&mut state.captures);
            let encoders = std::mem::take(&mut state.encoders);
            let audio = state.audio.take();
            let display_abort = state.display_abort.take();
            let watch_aborts = state
                .watches
                .drain()
                .filter_map(|(_, entry)| entry.abort)
                .collect::<Vec<_>>();
            let keep_awake = state.keep_awake;
            state.keep_awake = false;
            (
                captures,
                encoders,
                audio,
                display_abort,
                watch_aborts,
                keep_awake,
            )
        };

        if let Some(abort) = display_abort {
            abort.abort();
        }
        for abort in watch_aborts {
            abort.abort();
        }
        for entry in captures.into_values() {
            entry.abort.abort();
            drop(entry.handle);
        }
        for entry in encoders.into_values() {
            entry.abort.abort();
            drop(entry.encoder);
        }
        if let Some(entry) = audio {
            entry.abort.abort();
            drop(entry.handle);
        }

        self.platform.release_all();
        if keep_awake {
            self.platform.power_keep_awake(false);
        }
    }

    /// Notify the service about a desktop/session switch.
    pub fn notify_desktop(&self, name: &str) {
        let _ = self.out.send(PipeFrame::Ctl(Ctl::Desktop {
            name: name.to_owned(),
        }));
    }

    fn send(&self, frame: PipeFrame) {
        let _ = self.out.send(frame);
    }

    fn send_done(&self, req: u32, error: Option<WireError>) {
        self.send(PipeFrame::Ctl(Ctl::Done { req, error }));
    }

    fn send_displays(&self, displays: &[host_core::platform::DisplayInfo]) {
        let list = displays.iter().map(DisplayWire::from).collect();
        self.send(PipeFrame::Ctl(Ctl::Displays { list }));
    }

    fn handle_ctl(&mut self, ctl: Ctl) {
        match ctl {
            Ctl::Inject { display_id, input } => self.inject(display_id, &input),
            Ctl::ReleaseAll => self.platform.release_all(),
            Ctl::Action { req, kind } => self.action(req, kind),
            Ctl::ClipboardGet { req } => self.clipboard_get(req),
            Ctl::Watch { what } => self.watch(what),
            Ctl::Unwatch { what } => self.unwatch(what),
            Ctl::CaptureStart {
                req,
                cap,
                display,
                fps,
                max_width,
                max_height,
                region,
            } => self.capture_start(req, cap, display, fps, max_width, max_height, region),
            Ctl::CaptureStop { cap } => self.capture_stop(cap),
            Ctl::EncoderOpen {
                req,
                enc,
                codec,
                width,
                height,
                fps,
                bitrate_kbps,
            } => self.encoder_open(EncoderOpenArgs {
                req,
                enc,
                raw_codec: codec,
                width,
                height,
                fps,
                bitrate_kbps,
            }),
            Ctl::Encode {
                enc,
                cap,
                seq,
                force_keyframe,
                quality_boost,
            } => self.encode(enc, cap, seq, force_keyframe, quality_boost),
            Ctl::EncoderRate { enc, kbps, fps } => self.encoder_rate(enc, kbps, fps),
            Ctl::EncoderKeyframe { enc } => self.encoder_keyframe(enc),
            Ctl::EncoderBoost { enc, on } => self.encoder_boost(enc, on),
            Ctl::EncoderRoi { enc, regions } => self.encoder_roi(enc, regions),
            Ctl::EncoderClose { enc } => self.encoder_close(enc),
            Ctl::AudioStart {
                req,
                sample_rate,
                channels,
                exclude_self,
            } => self.audio_start(req, sample_rate, channels, exclude_self),
            Ctl::AudioStop => self.audio_stop(),
            Ctl::KeepAwake { on } => {
                self.platform.power_keep_awake(on);
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.keep_awake = on;
            }
            Ctl::Ping { n } => self.send(PipeFrame::Ctl(Ctl::Pong { n })),
            Ctl::Hello { .. }
            | Ctl::Welcome { .. }
            | Ctl::Pong { .. }
            | Ctl::Done { .. }
            | Ctl::Displays { .. }
            | Ctl::Frame { .. }
            | Ctl::CaptureEnded { .. }
            | Ctl::EncoderFailed { .. }
            | Ctl::EncoderInfo { .. }
            | Ctl::CursorShape { .. }
            | Ctl::CursorUse { .. }
            | Ctl::CursorHidden { .. }
            | Ctl::CursorPos { .. }
            | Ctl::Activity { .. }
            | Ctl::ClipboardChanged
            | Ctl::Desktop { .. } => debug!("discarding session-to-service control frame"),
        }
    }

    fn inject(&self, display_id: u32, encoded: &str) {
        let msg = match input_from_b64(encoded) {
            Ok(msg) => msg,
            Err(_) => {
                self.stats.invalid_inputs.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        let mut displays = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.displays.clone()
        };
        if !displays.iter().any(|display| display.id == display_id) {
            displays = self.platform.displays();
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.displays = displays.clone();
        }
        let display = displays
            .iter()
            .find(|display| display.id == display_id)
            .or_else(|| displays.iter().find(|display| display.primary))
            .or_else(|| displays.first());
        if let Some(display) = display {
            self.platform.inject_input(&msg, display);
        }
    }

    fn action(&self, req: u32, raw_kind: i32) {
        let kind = match host_core::platform::SessionActionKind::try_from(raw_kind) {
            Ok(kind) => kind,
            Err(_) => {
                self.send_done(req, Some(WireError::Unsupported));
                return;
            }
        };
        let error = self
            .platform
            .session_action(kind)
            .err()
            .map(|error| WireError::from(&error));
        self.send_done(req, error);
    }

    fn clipboard_get(&self, req: u32) {
        let Some(item) = self.platform.clipboard_get() else {
            self.send_done(req, None);
            return;
        };
        let kind = match item.kind {
            pb::clipboard_header::Kind::Text => BlobKind::Text,
            pb::clipboard_header::Kind::Png => BlobKind::Png,
            _ => {
                self.send_done(req, None);
                return;
            }
        };
        self.send(PipeFrame::Blob(crate::pipe_proto::BlobPacket {
            req,
            kind,
            data: item.data,
        }));
    }

    fn handle_blob(&self, req: u32, kind: BlobKind, data: Vec<u8>) {
        let kind = match kind {
            BlobKind::Text => pb::clipboard_header::Kind::Text,
            BlobKind::Png => pb::clipboard_header::Kind::Png,
        };
        let digest = Sha256::digest(&data);
        let mut sha256 = [0_u8; 32];
        sha256.copy_from_slice(&digest);
        let item = ClipboardItem { kind, data, sha256 };
        let error = self
            .platform
            .clipboard_set(item)
            .err()
            .map(|error| WireError::from(&error));
        self.send_done(req, error);
    }

    fn watch(&self, what: WatchKind) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(entry) = state.watches.get_mut(&what) {
                entry.enabled = true;
                return;
            }
            state.watches.insert(
                what,
                WatchEntry {
                    enabled: true,
                    abort: None,
                },
            );
        }

        match what {
            WatchKind::Cursor => {
                let (tx, mut rx) = mpsc::channel(16);
                self.platform.watch_cursor(tx);
                let state = Arc::clone(&self.state);
                let out = self.out.clone();
                let task = tokio::spawn(async move {
                    while let Some(update) = rx.recv().await {
                        let enabled = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .watches
                            .get(&WatchKind::Cursor)
                            .map(|entry| entry.enabled)
                            .unwrap_or(false);
                        if enabled {
                            let _ = out.send(PipeFrame::Ctl(cursor_to_ctl(&update)));
                        }
                    }
                });
                self.set_watch_abort(what, task.abort_handle());
            }
            WatchKind::Activity => {
                let (tx, mut rx) = mpsc::channel(16);
                self.platform.watch_local_activity(tx);
                let state = Arc::clone(&self.state);
                let out = self.out.clone();
                let task = tokio::spawn(async move {
                    while let Some(active) = rx.recv().await {
                        let enabled = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .watches
                            .get(&WatchKind::Activity)
                            .map(|entry| entry.enabled)
                            .unwrap_or(false);
                        if enabled {
                            let _ = out.send(PipeFrame::Ctl(Ctl::Activity { active }));
                        }
                    }
                });
                self.set_watch_abort(what, task.abort_handle());
            }
            WatchKind::Clipboard => {
                let (tx, mut rx) = mpsc::channel(16);
                self.platform.watch_clipboard(tx);
                let state = Arc::clone(&self.state);
                let out = self.out.clone();
                let task = tokio::spawn(async move {
                    while rx.recv().await.is_some() {
                        let enabled = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .watches
                            .get(&WatchKind::Clipboard)
                            .map(|entry| entry.enabled)
                            .unwrap_or(false);
                        if enabled {
                            let _ = out.send(PipeFrame::Ctl(Ctl::ClipboardChanged));
                        }
                    }
                });
                self.set_watch_abort(what, task.abort_handle());
            }
        }
    }

    fn set_watch_abort(&self, what: WatchKind, abort: AbortHandle) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = state.watches.get_mut(&what) {
            entry.abort = Some(abort);
        } else {
            abort.abort();
        }
    }

    fn unwatch(&self, what: WatchKind) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .watches
            .get_mut(&what)
        {
            entry.enabled = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_start(
        &self,
        req: u32,
        cap: u32,
        display: u32,
        fps: u32,
        max_width: u32,
        max_height: u32,
        region: Option<crate::pipe_proto::RegionWire>,
    ) {
        self.capture_stop(cap);
        let region = match region {
            Some(region) => {
                let mut displays = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .displays
                    .clone();
                let display_info = displays.iter().find(|info| info.id == display).cloned();
                let display_info = match display_info {
                    Some(info) => info,
                    None => {
                        displays = self.platform.displays();
                        let mut state = self
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.displays = displays.clone();
                        let Some(info) = displays.iter().find(|info| info.id == display).cloned()
                        else {
                            self.send_done(
                                req,
                                Some(WireError::Platform {
                                    message: "crop outside display".into(),
                                }),
                            );
                            return;
                        };
                        info
                    }
                };
                if !valid_region(region, display_info.width_px, display_info.height_px) {
                    self.send_done(
                        req,
                        Some(WireError::Platform {
                            message: "crop outside display".into(),
                        }),
                    );
                    return;
                }
                Some((region.into(), display_info.width_px, display_info.height_px))
            }
            None => None,
        };
        let (sink, mut rx) = mpsc::channel(4);
        let result = self.platform.capture(
            display,
            CaptureConfig {
                fps,
                max_width,
                max_height,
            },
            sink,
        );
        let handle = match result {
            Ok(handle) => handle,
            Err(error) => {
                self.send_done(req, Some(WireError::from(&error)));
                return;
            }
        };

        let ring = Arc::new(Mutex::new(FrameRing::new()));
        let token = Arc::new(());
        let token_for_task = Arc::clone(&token);
        let ring_for_task = Arc::clone(&ring);
        let state = Arc::clone(&self.state);
        let out = self.out.clone();
        let task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let (seq, width, height, capture_ts_us) = {
                    let mut ring = ring_for_task
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let seq = ring.push(frame.clone());
                    (seq, frame.width, frame.height, frame.capture_ts_us)
                };
                let _ = out.send(PipeFrame::Ctl(Ctl::Frame {
                    cap,
                    seq,
                    width,
                    height,
                    capture_ts_us,
                }));
            }
            let _ = out.send(PipeFrame::Ctl(Ctl::CaptureEnded { cap, error: None }));
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let remove = state
                .captures
                .get(&cap)
                .map(|entry| Arc::ptr_eq(&entry.token, &token_for_task))
                .unwrap_or(false);
            if remove {
                state.captures.remove(&cap);
            }
        });
        let abort = task.abort_handle();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.captures.insert(
            cap,
            CaptureEntry {
                handle,
                abort,
                ring,
                token,
                region,
            },
        );
        self.send_done(req, None);
    }

    fn capture_stop(&self, cap: u32) {
        let entry = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .captures
            .remove(&cap);
        if let Some(entry) = entry {
            entry.abort.abort();
            drop(entry.handle);
        }
    }

    fn encoder_open(&self, args: EncoderOpenArgs) {
        let codec = match pb::Codec::try_from(args.raw_codec) {
            Ok(codec) => codec,
            Err(_) => {
                self.send_done(args.req, Some(WireError::Unsupported));
                return;
            }
        };
        self.encoder_close(args.enc);
        let (sink, mut rx) = mpsc::channel(8);
        let encoder = match self.platform.encoder(EncoderConfig {
            codec,
            width: args.width,
            height: args.height,
            fps: args.fps,
            bitrate_kbps: args.bitrate_kbps,
            sink,
        }) {
            Ok(encoder) => encoder,
            Err(error) => {
                self.send_done(args.req, Some(WireError::from(&error)));
                return;
            }
        };
        let out = self.out.clone();
        let enc = args.enc;
        let task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let _ = out.send(PipeFrame::Video(VideoPacket {
                    enc,
                    keyframe: frame.keyframe,
                    still_refine: frame.still_refine,
                    capture_ts_us: frame.capture_ts_us,
                    encoded_ts_us: frame.encoded_ts_us,
                    data: frame.data,
                }));
            }
        });
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.encoders.insert(
            enc,
            EncoderEntry {
                encoder,
                abort: task.abort_handle(),
            },
        );
        let roi = state
            .encoders
            .get(&enc)
            .map(|entry| entry.encoder.roi_supported())
            .unwrap_or(false);
        drop(state);
        self.send(PipeFrame::Ctl(Ctl::EncoderInfo { enc, roi }));
        self.send_done(args.req, None);
    }

    fn encode(&self, enc: u32, cap: u32, seq: u64, force_keyframe: bool, quality_boost: bool) {
        let (frame, region) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = state.captures.get(&cap) else {
                self.stats.dropped_encodes.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let found = entry
                .ring
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(seq);
            (found, entry.region)
        };
        let Some(frame) = frame else {
            self.stats.dropped_encodes.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = state.encoders.get_mut(&enc) else {
            self.stats.dropped_encodes.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let frame = viewport_crop::wrap_frame(frame, region);
        if let Err(error) = entry.encoder.encode(
            frame,
            EncodeOpts {
                force_keyframe,
                quality_boost,
            },
        ) {
            let _ = self.out.send(PipeFrame::Ctl(Ctl::EncoderFailed {
                enc,
                error: WireError::from(&error),
            }));
        }
    }

    fn encoder_rate(&self, enc: u32, kbps: u32, fps: u32) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .encoders
            .get_mut(&enc)
        {
            entry.encoder.set_rate(kbps, fps);
        }
    }

    fn encoder_keyframe(&self, enc: u32) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .encoders
            .get_mut(&enc)
        {
            entry.encoder.force_keyframe();
        }
    }

    fn encoder_boost(&self, enc: u32, on: bool) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .encoders
            .get_mut(&enc)
        {
            entry.encoder.set_quality_boost(on);
        }
    }

    fn encoder_roi(&self, enc: u32, regions: Vec<RoiWire>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = state.encoders.get_mut(&enc) else {
            return;
        };
        let regions = regions.into_iter().map(Into::into).collect::<Vec<_>>();
        if let Err(error) = entry.encoder.set_roi(&regions) {
            debug!(enc, error = %error, "encoder ROI update failed");
        }
    }

    fn encoder_close(&self, enc: u32) {
        let entry = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .encoders
            .remove(&enc);
        if let Some(entry) = entry {
            entry.abort.abort();
            drop(entry.encoder);
        }
    }

    fn audio_start(&self, req: u32, sample_rate: u32, channels: u8, exclude_self: bool) {
        self.audio_stop();
        let (sink, mut rx) = mpsc::channel(16);
        let handle = match self.platform.capture_audio(
            AudioCaptureConfig {
                sample_rate,
                channels,
                exclude_self,
            },
            sink,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                self.send_done(req, Some(WireError::from(&error)));
                return;
            }
        };

        let token = Arc::new(());
        let token_for_task = Arc::clone(&token);
        let state = Arc::clone(&self.state);
        let out = self.out.clone();
        let task = tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                let _ = out.send(PipeFrame::Audio(crate::pipe_proto::AudioPacket {
                    capture_ts_us: chunk.capture_ts_us,
                    channels: chunk.channels,
                    frames: chunk.frames,
                    samples: chunk.samples,
                }));
            }
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let remove = state
                .audio
                .as_ref()
                .map(|entry| Arc::ptr_eq(&entry.token, &token_for_task))
                .unwrap_or(false);
            if remove {
                state.audio = None;
            }
        });
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.audio = Some(AudioEntry {
            handle,
            abort: task.abort_handle(),
            token,
        });
        self.send_done(req, None);
    }

    fn audio_stop(&self) {
        let entry = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .audio
            .take();
        if let Some(entry) = entry {
            entry.abort.abort();
            drop(entry.handle);
        }
    }
}
