//! Media proxy: only frame coordinates and encoded/PCM output cross the pipe.

use std::collections::HashMap;
use std::sync::{mpsc as sync_mpsc, Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureConfig, CaptureHandle, CapturedFrame,
    EncodeOpts, EncodedFrame, Encoder, EncoderConfig, FrameSink, PixelRect, PlatformError, Result,
    RoiRegion,
};
use tokio::sync::mpsc::error::TrySendError;

use crate::pipe_proto::{Ctl, PipeFrame, RegionWire, RoiWire, WireError};
use crate::proxy::Inner;

/// Coordinates of a frame retained by the session process, stored in CapturedFrame.native.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFrame {
    pub cap: u32,
    pub seq: u64,
}

#[derive(Default)]
pub(crate) struct MediaState {
    captures: HashMap<u32, CaptureEntry>,
    encoders: HashMap<u32, EncoderEntry>,
    audio: Option<AudioEntry>,
    next_cap: u32,
    next_enc: u32,
    next_audio: u64,
    replay: HashMap<u32, Resource>,
    dropped_frames: u64,
}

struct CaptureEntry {
    display: u32,
    cfg: CaptureConfig,
    sink: FrameSink,
    region: Option<PixelRect>,
}

struct EncoderEntry {
    cfg: EncoderConfig,
    waiting_keyframe: bool,
    error: Option<WireError>,
    roi_supported: bool,
    roi: Vec<RoiRegion>,
}

struct AudioEntry {
    // Distinguishes an old handle from a new capture after replay fails.
    id: u64,
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resource {
    Capture(u32),
    Encoder(u32),
    Audio(u64),
}

impl Resource {
    fn start(self, media: &MediaState, req: u32) -> Option<Ctl> {
        match self {
            Self::Capture(cap) => media.captures.get(&cap).map(|entry| Ctl::CaptureStart {
                req,
                cap,
                display: entry.display,
                fps: entry.cfg.fps,
                max_width: entry.cfg.max_width,
                max_height: entry.cfg.max_height,
                region: entry.region.map(RegionWire::from),
            }),
            Self::Encoder(enc) => media.encoders.get(&enc).map(|entry| Ctl::EncoderOpen {
                req,
                enc,
                codec: entry.cfg.codec as i32,
                width: entry.cfg.width,
                height: entry.cfg.height,
                fps: entry.cfg.fps,
                bitrate_kbps: entry.cfg.bitrate_kbps,
            }),
            Self::Audio(id) => media
                .audio
                .as_ref()
                .filter(|entry| entry.id == id)
                .map(|entry| Ctl::AudioStart {
                    req,
                    sample_rate: entry.cfg.sample_rate,
                    channels: entry.cfg.channels,
                    exclude_self: entry.cfg.exclude_self,
                }),
        }
    }

    fn stop(self) -> Ctl {
        match self {
            Self::Capture(cap) => Ctl::CaptureStop { cap },
            Self::Encoder(enc) => Ctl::EncoderClose { enc },
            Self::Audio(_) => Ctl::AudioStop,
        }
    }

    fn remove(self, inner: &mut Inner) -> bool {
        inner.media.replay.retain(|req, resource| {
            if *resource == self {
                inner.pending.remove(req);
                false
            } else {
                true
            }
        });
        match self {
            Self::Capture(cap) => inner.media.captures.remove(&cap).is_some(),
            Self::Encoder(enc) => inner.media.encoders.remove(&enc).is_some(),
            Self::Audio(id) => {
                if inner
                    .media
                    .audio
                    .as_ref()
                    .is_some_and(|entry| entry.id == id)
                {
                    inner.media.audio = None;
                    true
                } else {
                    false
                }
            }
        }
    }
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|error| error.into_inner())
}

fn platform_error(message: &str) -> PlatformError {
    PlatformError::Platform(message.into())
}

fn next_id(next: &mut u32) -> Result<u32> {
    *next = next
        .checked_add(1)
        .ok_or_else(|| platform_error("session process media ids exhausted"))?;
    Ok(*next)
}

fn next_req(inner: &mut Inner) -> u32 {
    loop {
        inner.next_req = inner.next_req.wrapping_add(1);
        if !inner.pending.contains_key(&inner.next_req) {
            return inner.next_req;
        }
    }
}

/// Register and send atomically with respect to detach/replay, then wait without the lock.
fn open(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    register: impl FnOnce(&mut MediaState) -> Result<Resource>,
) -> Result<Resource> {
    let (tx, rx) = sync_mpsc::channel();
    let (resource, generation, req) = {
        let mut inner = lock(shared);
        if !inner.connected() {
            return Err(platform_error("session process not connected"));
        }
        let resource = register(&mut inner.media)?;
        let req = next_req(&mut inner);
        let Some(ctl) = resource.start(&inner.media, req) else {
            resource.remove(&mut inner);
            return Err(platform_error("session process media registration missing"));
        };
        inner.pending.insert(req, tx);
        if let Err(error) = inner.send(PipeFrame::Ctl(ctl)) {
            resource.remove(&mut inner);
            return Err(error);
        }
        (resource, inner.generation, req)
    };

    let (result, stop) = match rx.recv_timeout(timeout) {
        Ok(Ok(None)) => return Ok(resource),
        Ok(Ok(Some(_))) => (
            Err(platform_error("unexpected session process response")),
            true,
        ),
        Ok(Err(error)) => (Err(error), false),
        Err(error) => (
            Err(platform_error(match error {
                sync_mpsc::RecvTimeoutError::Timeout => "session process timeout",
                sync_mpsc::RecvTimeoutError::Disconnected => "session process disconnected",
            })),
            true,
        ),
    };
    let mut inner = lock(shared);
    if inner.generation == generation {
        inner.pending.remove(&req);
    }
    if resource.remove(&mut inner) && (stop || inner.generation != generation) {
        // A reconnect can have replayed a registration before this waiter woke up.
        let _ = inner.send(PipeFrame::Ctl(resource.stop()));
    }
    result
}

pub(crate) fn capture(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    display: u32,
    cfg: CaptureConfig,
    sink: FrameSink,
) -> Result<Box<dyn CaptureHandle>> {
    capture_with_region(shared, timeout, display, None, cfg, sink)
}

pub(crate) fn capture_region(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    display: u32,
    rect: PixelRect,
    cfg: CaptureConfig,
    sink: FrameSink,
) -> Result<Box<dyn CaptureHandle>> {
    capture_with_region(shared, timeout, display, Some(rect), cfg, sink)
}

fn capture_with_region(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    display: u32,
    region: Option<PixelRect>,
    cfg: CaptureConfig,
    sink: FrameSink,
) -> Result<Box<dyn CaptureHandle>> {
    let resource = open(shared, timeout, |media| {
        let cap = next_id(&mut media.next_cap)?;
        media.captures.insert(
            cap,
            CaptureEntry {
                display,
                cfg,
                sink,
                region,
            },
        );
        Ok(Resource::Capture(cap))
    })?;
    let Resource::Capture(cap) = resource else {
        return Err(platform_error("unexpected session process media handle"));
    };
    Ok(Box::new(ProxyCapture {
        cap,
        inner: Arc::downgrade(shared),
    }))
}

pub(crate) fn encoder(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    cfg: EncoderConfig,
) -> Result<Box<dyn Encoder>> {
    let resource = open(shared, timeout, |media| {
        let enc = next_id(&mut media.next_enc)?;
        media.encoders.insert(
            enc,
            EncoderEntry {
                cfg,
                waiting_keyframe: false,
                error: None,
                roi_supported: false,
                roi: Vec::new(),
            },
        );
        Ok(Resource::Encoder(enc))
    })?;
    let Resource::Encoder(enc) = resource else {
        return Err(platform_error("unexpected session process media handle"));
    };
    Ok(Box::new(ProxyEncoder {
        enc,
        inner: Arc::downgrade(shared),
    }))
}

pub(crate) fn capture_audio(
    shared: &Arc<Mutex<Inner>>,
    timeout: Duration,
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
) -> Result<Box<dyn CaptureHandle>> {
    if lock(shared).media.audio.is_some() {
        return Err(platform_error("audio capture already active"));
    }
    let resource = open(shared, timeout, |media| {
        // Recheck after taking the registration lock in case another caller won.
        if media.audio.is_some() {
            return Err(platform_error("audio capture already active"));
        }
        media.next_audio = media
            .next_audio
            .checked_add(1)
            .ok_or_else(|| platform_error("session process audio ids exhausted"))?;
        let id = media.next_audio;
        media.audio = Some(AudioEntry { id, cfg, sink });
        Ok(Resource::Audio(id))
    })?;
    let Resource::Audio(id) = resource else {
        return Err(platform_error("unexpected session process media handle"));
    };
    Ok(Box::new(ProxyAudio {
        id,
        inner: Arc::downgrade(shared),
    }))
}

pub(crate) struct ProxyCapture {
    cap: u32,
    inner: Weak<Mutex<Inner>>,
}

impl CaptureHandle for ProxyCapture {}

impl Drop for ProxyCapture {
    fn drop(&mut self) {
        stop(&self.inner, Resource::Capture(self.cap));
    }
}

pub(crate) struct ProxyAudio {
    id: u64,
    inner: Weak<Mutex<Inner>>,
}

impl CaptureHandle for ProxyAudio {}

impl Drop for ProxyAudio {
    fn drop(&mut self) {
        stop(&self.inner, Resource::Audio(self.id));
    }
}

fn stop(weak: &Weak<Mutex<Inner>>, resource: Resource) {
    if let Some(shared) = weak.upgrade() {
        let mut inner = lock(&shared);
        if resource.remove(&mut inner) {
            let _ = inner.send(PipeFrame::Ctl(resource.stop()));
        }
    }
}

pub(crate) struct ProxyEncoder {
    enc: u32,
    inner: Weak<Mutex<Inner>>,
}

impl Encoder for ProxyEncoder {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()> {
        let remote = frame
            .native
            .downcast_ref::<RemoteFrame>()
            .ok_or_else(|| platform_error("frame not from session process"))?;
        let shared = self
            .inner
            .upgrade()
            .ok_or_else(|| platform_error("session process not connected"))?;
        let mut inner = lock(&shared);
        if let Some(error) = inner
            .media
            .encoders
            .get(&self.enc)
            .and_then(|entry| entry.error.as_ref())
        {
            return Err(error.clone().into());
        }
        inner.send(PipeFrame::Ctl(Ctl::Encode {
            enc: self.enc,
            cap: remote.cap,
            seq: remote.seq,
            force_keyframe: opts.force_keyframe,
            quality_boost: opts.quality_boost,
        }))
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        if let Some(shared) = self.inner.upgrade() {
            let mut inner = lock(&shared);
            if let Some(entry) = inner.media.encoders.get_mut(&self.enc) {
                entry.cfg.bitrate_kbps = kbps;
                entry.cfg.fps = fps;
                let _ = inner.send(PipeFrame::Ctl(Ctl::EncoderRate {
                    enc: self.enc,
                    kbps,
                    fps,
                }));
            }
        }
    }

    fn force_keyframe(&mut self) {
        if let Some(shared) = self.inner.upgrade() {
            let _ = lock(&shared).send(PipeFrame::Ctl(Ctl::EncoderKeyframe { enc: self.enc }));
        }
    }

    fn set_quality_boost(&mut self, on: bool) {
        if let Some(shared) = self.inner.upgrade() {
            let _ = lock(&shared).send(PipeFrame::Ctl(Ctl::EncoderBoost { enc: self.enc, on }));
        }
    }

    fn roi_supported(&self) -> bool {
        self.inner
            .upgrade()
            .and_then(|shared| {
                lock(&shared)
                    .media
                    .encoders
                    .get(&self.enc)
                    .map(|entry| entry.roi_supported)
            })
            .unwrap_or(false)
    }

    fn set_roi(&mut self, regions: &[RoiRegion]) -> Result<()> {
        let shared = self
            .inner
            .upgrade()
            .ok_or_else(|| platform_error("session process not connected"))?;
        let mut inner = lock(&shared);
        let Some(entry) = inner.media.encoders.get_mut(&self.enc) else {
            return Err(platform_error("encoder not found"));
        };
        if !entry.roi_supported {
            return Err(PlatformError::Unsupported);
        }
        entry.roi.clear();
        entry.roi.extend_from_slice(regions);
        inner.send(PipeFrame::Ctl(Ctl::EncoderRoi {
            enc: self.enc,
            regions: regions.iter().copied().map(RoiWire::from).collect(),
        }))
    }
}

impl Drop for ProxyEncoder {
    fn drop(&mut self) {
        stop(&self.inner, Resource::Encoder(self.enc));
    }
}

pub(crate) fn on_media(inner: &mut Inner, frame: PipeFrame) {
    match frame {
        PipeFrame::Ctl(Ctl::Frame {
            cap,
            seq,
            width,
            height,
            capture_ts_us,
        }) => {
            if let Some(entry) = inner.media.captures.get(&cap) {
                if let Err(TrySendError::Full(_)) = entry.sink.try_send(CapturedFrame {
                    display_id: entry.display,
                    width,
                    height,
                    capture_ts_us,
                    native: Arc::new(RemoteFrame { cap, seq }),
                }) {
                    inner.media.dropped_frames = inner.media.dropped_frames.saturating_add(1);
                }
            }
        }
        PipeFrame::Ctl(Ctl::CaptureEnded { cap, .. }) => {
            Resource::Capture(cap).remove(inner);
        }
        PipeFrame::Video(packet) => {
            let Some(entry) = inner.media.encoders.get_mut(&packet.enc) else {
                return;
            };
            if entry.waiting_keyframe && !packet.keyframe {
                return;
            }
            match entry.cfg.sink.try_send(EncodedFrame {
                data: packet.data,
                keyframe: packet.keyframe,
                still_refine: packet.still_refine,
                capture_ts_us: packet.capture_ts_us,
                encoded_ts_us: packet.encoded_ts_us,
            }) {
                Ok(()) if packet.keyframe => entry.waiting_keyframe = false,
                Err(TrySendError::Full(_)) => {
                    let request = !entry.waiting_keyframe;
                    entry.waiting_keyframe = true;
                    if request {
                        let _ =
                            inner.send(PipeFrame::Ctl(Ctl::EncoderKeyframe { enc: packet.enc }));
                    }
                }
                _ => {}
            }
        }
        PipeFrame::Ctl(Ctl::EncoderFailed { enc, error }) => {
            if let Some(entry) = inner.media.encoders.get_mut(&enc) {
                entry.error = Some(error);
            }
        }
        PipeFrame::Ctl(Ctl::EncoderInfo { enc, roi }) => {
            if let Some(entry) = inner.media.encoders.get_mut(&enc) {
                entry.roi_supported = roi;
            }
        }
        PipeFrame::Audio(packet) => {
            if let Some(entry) = &inner.media.audio {
                let _ = entry.sink.try_send(AudioChunk {
                    capture_ts_us: packet.capture_ts_us,
                    channels: packet.channels,
                    frames: packet.frames,
                    samples: packet.samples,
                });
            }
        }
        PipeFrame::Ctl(Ctl::Done { req, error }) => {
            let Some(resource) = inner.media.replay.remove(&req) else {
                return;
            };
            inner.pending.remove(&req);
            let replay_roi = if error.is_none() {
                match resource {
                    Resource::Encoder(enc) => inner
                        .media
                        .encoders
                        .get(&enc)
                        .filter(|entry| !entry.roi.is_empty())
                        .map(|entry| (enc, entry.roi.clone())),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(error) = error {
                match resource {
                    Resource::Encoder(enc) => {
                        if let Some(entry) = inner.media.encoders.get_mut(&enc) {
                            entry.error = Some(error);
                        }
                    }
                    _ => {
                        resource.remove(inner);
                    }
                }
            }
            if let Some((enc, regions)) = replay_roi {
                let _ = inner.send(PipeFrame::Ctl(Ctl::EncoderRoi {
                    enc,
                    regions: regions.into_iter().map(RoiWire::from).collect(),
                }));
            }
        }
        _ => {}
    }
}

pub(crate) fn detach(inner: &mut Inner) {
    // Registrations keep the host-core channels alive across a process restart.
    for req in inner.media.replay.keys() {
        inner.pending.remove(req);
    }
    inner.media.replay.clear();
    for entry in inner.media.encoders.values_mut() {
        entry.waiting_keyframe = true;
        entry.roi_supported = false;
    }
}

pub(crate) fn replay_media(inner: &mut Inner) {
    detach(inner);
    let resources: Vec<Resource> = inner
        .media
        .captures
        .keys()
        .copied()
        .map(Resource::Capture)
        .chain(inner.media.encoders.keys().copied().map(Resource::Encoder))
        .chain(
            inner
                .media
                .audio
                .as_ref()
                .map(|entry| Resource::Audio(entry.id)),
        )
        .collect();
    for resource in resources {
        let req = next_req(inner);
        let Some(ctl) = resource.start(&inner.media, req) else {
            continue;
        };
        // Reserve in the shared request table too, so control requests cannot reuse
        // this id. Replay Done is handled by on_media; it has no blocking waiter.
        let (tx, _rx) = sync_mpsc::channel();
        inner.pending.insert(req, tx);
        inner.media.replay.insert(req, resource);
        if let Resource::Encoder(enc) = resource {
            if let Some(entry) = inner.media.encoders.get_mut(&enc) {
                entry.error = None;
            }
        }
        if inner.send(PipeFrame::Ctl(ctl)).is_err() {
            detach(inner);
            break;
        }
        if let Resource::Encoder(enc) = resource {
            if inner
                .send(PipeFrame::Ctl(Ctl::EncoderKeyframe { enc }))
                .is_err()
            {
                detach(inner);
                break;
            }
        }
    }
}
