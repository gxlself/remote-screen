//! A synthetic platform that replays a real, decodable HEVC clip.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, ClipboardItem, ConsoleState, CursorUpdate,
    DisplayInfo, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, FrameSink, HostInfo, InputEvent,
    InputMsg, KeyStore, Nic, PermissionKind, Permissions, Platform, PlatformError, Result,
    SessionActionKind,
};
use rsp_core::clock::unix_micros;
use rsp_core::nal::{hevc_nal_type, hevc_type, split_annexb};
use rsp_core::pb;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{FakePlatform, SyntheticFrame};

const CLIP_WIDTH: u32 = 640;
const CLIP_HEIGHT: u32 = 360;
const CLIP_FPS: u32 = 30;

/// A sequence of Annex-B HEVC access units with the indices of its IRAP frames.
pub struct Clip {
    aus: Vec<Arc<[u8]>>,
    keyframes: Vec<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("no access unit delimiter found")]
    NoAud,
    #[error("no IRAP picture in clip")]
    NoKeyframe,
}

impl Clip {
    /// Splits an Annex-B stream at access unit delimiters. Bytes before the
    /// first AUD become the start of the first access unit.
    pub fn parse(annexb: &[u8]) -> std::result::Result<Clip, ClipError> {
        let mut groups: Vec<Vec<&[u8]>> = Vec::new();
        let mut current = Vec::new();
        let mut saw_aud = false;
        for nal in split_annexb(annexb) {
            let is_aud = hevc_nal_type(nal) == Some(hevc_type::AUD);
            if is_aud && saw_aud {
                groups.push(std::mem::take(&mut current));
            }
            saw_aud |= is_aud;
            current.push(nal);
        }
        if !saw_aud {
            return Err(ClipError::NoAud);
        }
        if !current.is_empty() {
            groups.push(current);
        }

        let mut aus = Vec::with_capacity(groups.len());
        let mut keyframes = Vec::new();
        for (index, nals) in groups.into_iter().enumerate() {
            if nals
                .iter()
                .any(|nal| matches!(hevc_nal_type(nal), Some(kind) if (16..=21).contains(&kind)))
            {
                keyframes.push(index);
            }
            let capacity = nals.iter().map(|nal| 4 + nal.len()).sum();
            let mut au = Vec::with_capacity(capacity);
            for nal in nals {
                au.extend_from_slice(&[0, 0, 0, 1]);
                au.extend_from_slice(nal);
            }
            aus.push(Arc::from(au.into_boxed_slice()));
        }
        if keyframes.is_empty() {
            return Err(ClipError::NoKeyframe);
        }
        Ok(Clip { aus, keyframes })
    }

    /// The four-second testsrc clip committed with the fixture.
    pub fn bundled() -> &'static Clip {
        static CLIP: OnceLock<Clip> = OnceLock::new();
        CLIP.get_or_init(|| {
            Clip::parse(include_bytes!("../assets/testsrc-640x360-30.hevc"))
                .expect("bundled HEVC clip contains AUDs and IRAP pictures")
        })
    }

    pub fn len(&self) -> usize {
        self.aus.len()
    }

    pub fn is_empty(&self) -> bool {
        self.aus.is_empty()
    }

    pub fn au(&self, index: usize) -> &[u8] {
        self.aus[index].as_ref()
    }

    pub fn is_keyframe(&self, index: usize) -> bool {
        self.keyframes.binary_search(&index).is_ok()
    }

    pub fn keyframe_indices(&self) -> &[usize] {
        &self.keyframes
    }

    /// Finds the next keyframe, wrapping around to the first one after the end.
    pub fn next_keyframe_at_or_after(&self, index: usize) -> usize {
        let next = self.keyframes.partition_point(|&key| key < index);
        self.keyframes
            .get(next)
            .copied()
            .unwrap_or(self.keyframes[0])
    }
}

/// Replays [`Clip::bundled`] as an encoded HEVC stream.
pub struct ClipEncoder {
    clip: &'static Clip,
    cursor: usize,
    force: bool,
    rate_kbps: u32,
    rate_fps: u32,
    quality_boost: bool,
    sink: mpsc::Sender<EncodedFrame>,
}

impl ClipEncoder {
    pub fn new(clip: &'static Clip, cfg: EncoderConfig) -> Result<Self> {
        if cfg.codec != pb::Codec::Hevc {
            return Err(PlatformError::Unsupported);
        }
        Ok(Self {
            clip,
            cursor: clip.keyframe_indices()[0],
            force: false,
            rate_kbps: cfg.bitrate_kbps,
            rate_fps: cfg.fps,
            quality_boost: false,
            sink: cfg.sink,
        })
    }

    /// Returns the last rate recorded by [`Encoder::set_rate`].
    pub fn rate(&self) -> (u32, u32) {
        (self.rate_kbps, self.rate_fps)
    }

    /// Returns the last quality-boost setting.
    pub fn quality_boost(&self) -> bool {
        self.quality_boost
    }

    fn advance(&mut self) {
        self.cursor = (self.cursor + 1) % self.clip.len();
    }
}

impl Encoder for ClipEncoder {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()> {
        if self.force || opts.force_keyframe {
            self.cursor = self.clip.next_keyframe_at_or_after(self.cursor);
        }
        let output = EncodedFrame {
            data: self.clip.au(self.cursor).to_vec(),
            keyframe: self.clip.is_keyframe(self.cursor),
            still_refine: false,
            capture_ts_us: frame.capture_ts_us,
            encoded_ts_us: unix_micros(),
        };
        match self.sink.try_send(output) {
            Ok(()) => {
                self.force = false;
                self.advance();
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.force = true;
                self.advance();
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PlatformError::Platform("encoder sink closed".into()))
            }
        }
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        self.rate_kbps = kbps;
        self.rate_fps = fps;
    }

    fn force_keyframe(&mut self) {
        self.force = true;
    }

    fn set_quality_boost(&mut self, on: bool) {
        self.quality_boost = on;
    }
}

/// A [`FakePlatform`] whose display and encoder replay a real HEVC clip.
#[derive(Clone)]
pub struct ClipPlatform {
    inner: FakePlatform,
    clip: &'static Clip,
}

impl ClipPlatform {
    pub fn new() -> Self {
        Self {
            inner: FakePlatform::with_displays(vec![clip_display()]),
            clip: Clip::bundled(),
        }
    }

    pub fn inner(&self) -> &FakePlatform {
        &self.inner
    }
}

impl Default for ClipPlatform {
    fn default() -> Self {
        Self::new()
    }
}

fn clip_display() -> DisplayInfo {
    DisplayInfo {
        id: 1,
        name: "Clip display".into(),
        width_px: CLIP_WIDTH,
        height_px: CLIP_HEIGHT,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: CLIP_FPS,
        primary: true,
        rotation: 0,
    }
}

struct ClipCapture {
    task: JoinHandle<()>,
}

impl CaptureHandle for ClipCapture {}

impl Drop for ClipCapture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Platform for ClipPlatform {
    fn info(&self) -> HostInfo {
        self.inner.info()
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        vec![clip_display()]
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        self.inner.watch_displays(tx);
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        if display != 1 {
            return Err(PlatformError::Platform("unknown display".into()));
        }
        if cfg.fps == 0 || cfg.fps > 120 {
            return Err(PlatformError::Platform("capture fps must be 1..120".into()));
        }
        let fps = cfg.fps.min(CLIP_FPS);
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| PlatformError::Platform(error.to_string()))?;
        let task = runtime.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(fps)));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut frame_number = 0u64;
            loop {
                tick.tick().await;
                let capture_ts_us = unix_micros();
                let frame = CapturedFrame {
                    display_id: 1,
                    width: CLIP_WIDTH,
                    height: CLIP_HEIGHT,
                    capture_ts_us,
                    native: Arc::new(SyntheticFrame {
                        frame_number,
                        capture_ts_us,
                    }),
                };
                if let Err(mpsc::error::TrySendError::Closed(_)) = sink.try_send(frame) {
                    return;
                }
                frame_number = frame_number.wrapping_add(1);
            }
        });
        Ok(Box::new(ClipCapture { task }))
    }

    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        Ok(Box::new(ClipEncoder::new(self.clip, cfg)?))
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.inner.inject(ev, display);
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        self.inner.inject_input(msg, display);
    }

    fn release_all(&self) {
        self.inner.release_all();
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        self.inner.session_action(kind)
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        self.inner.watch_cursor(tx);
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        self.inner.watch_local_activity(tx);
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.inner.clipboard_get()
    }

    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        self.inner.clipboard_set(item)
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        self.inner.watch_clipboard(tx);
    }

    fn power_keep_awake(&self, on: bool) {
        self.inner.power_keep_awake(on);
    }

    fn console_state(&self) -> ConsoleState {
        self.inner.console_state()
    }

    fn permissions(&self) -> Permissions {
        self.inner.permissions()
    }

    fn request_permission(&self, permission: PermissionKind) {
        self.inner.request_permission(permission);
    }

    fn nics(&self) -> Vec<Nic> {
        self.inner.nics()
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        self.inner.send_wol(mac, via)
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.inner.keystore()
    }
}
