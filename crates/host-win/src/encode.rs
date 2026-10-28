#![cfg(windows)]
//! Windows encoder worker, hardware warm-up and bounded asynchronous delivery.
//!
//! Set `encoder_factory()` to `Some(Arc::new(crate::encode::open_encoder))`.
//! Before connecting the session pipe, call [`configure`] with
//! `EncodePrefs { encoder: host_config.video.encoder }`, if available, then
//! [`warm_up`]. Otherwise `RS_VIDEO_ENCODER` supplies the preference (Auto by
//! default); the service may set it when launching the session process.
//!
//! HostStats in packetizer.rs includes still/refine retries with their original
//! capture timestamp. Use encode_probe / [`EncoderStatus`] capture latency for
//! W3 acceptance: those windows exclude both kinds of repeated input.
//! This worker retains the latest capture for idle keyframe requests. Together
//! with the session's four retained captures and the pending mailbox frame it
//! occupies at most six of the sixteen capture-pool slots.
//!
//! Inbox guards only move work and flags. Native calls, sink waits, and releases
//! of displaced frames happen after releasing that guard. Each worker owns its
//! COM apartment; backend/processor objects are destroyed before the apartment.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use host_core::{
    config::VideoEncoder,
    platform::{CapturedFrame, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, PlatformError},
};
use rsp_core::{clock::unix_micros, pb::Codec};
use tokio::{runtime::Handle, sync::mpsc::error::TrySendError};
use tracing::{debug, error, info, trace, warn};
use windows::Win32::{
    Graphics::Direct3D11::ID3D11Device,
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
};

use crate::{
    capture::WinFrame,
    ffmpeg::{FfEncoder, FfError, FfLib, FfOpen, FfPacket, HwPath},
    mf_enc::{mf_startup, MfEncoder, MfError, MfOpen},
    vpmf_model::VpKey,
    vproc::{self, VideoProc},
};
use host_core::encode_model::{
    au_is_key, effective_bps, ffmpeg_codec_name, ffmpeg_options, ffmpeg_options_retry,
    ffmpeg_rate_options, parse_encoder_pref, probe_plan, rate_cost, refine_action, refine_mode,
    source_change, validate, Backend, BackendChooser, EncParams, EncodeStats, FailureState,
    FrameMeta, JobFlags, KeyTracker, LatencyWindow, Mailbox, OutputGate, ParamError, ParamSetCache,
    ProbeEntry, ProbeResult, ProbeTable, PtsMap, RateApplier, RateCost, RefineAction, SourceChange,
    SourceKey, ENCODER_ENV, VENDOR_MICROSOFT_BASIC,
};

pub const PROBE_WAIT: Duration = Duration::from_secs(3);
pub const PROBE_SIZE: (u32, u32) = (1280, 720);
pub const PROBE_FPS: u32 = 60;
pub const PROBE_KBPS: u32 = 4_000;
pub const PROBE_FRAMES: i64 = 4;
pub const PROBE_FRAME_WAIT: Duration = Duration::from_millis(500);
pub const SINK_WAIT: Duration = Duration::from_millis(50);
pub const MF_POLL: Duration = Duration::from_millis(10);
pub const DROP_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EncodePrefs {
    pub encoder: VideoEncoder,
}

impl EncodePrefs {
    pub fn from_env() -> EncodePrefs {
        static WARNED: AtomicBool = AtomicBool::new(false);
        let encoder = match std::env::var(ENCODER_ENV) {
            Ok(value) => match parse_encoder_pref(&value) {
                Some(pref) => pref,
                None => {
                    if !WARNED.swap(true, Ordering::Relaxed) {
                        warn!("invalid RS_VIDEO_ENCODER; using auto");
                    }
                    VideoEncoder::Auto
                }
            },
            Err(std::env::VarError::NotPresent) => VideoEncoder::Auto,
            Err(_) => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    warn!("invalid RS_VIDEO_ENCODER; using auto");
                }
                VideoEncoder::Auto
            }
        };
        Self { encoder }
    }
}

static PREFS: OnceLock<EncodePrefs> = OnceLock::new();
static ENV_PREFS: OnceLock<EncodePrefs> = OnceLock::new();
static PROBE_STARTED: OnceLock<()> = OnceLock::new();
static PROBES: Mutex<Option<ProbeTable>> = Mutex::new(None);
static PROBE_READY: Condvar = Condvar::new();
static NEXT_WORKER: AtomicU32 = AtomicU32::new(0);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn configure(prefs: EncodePrefs) -> bool {
    if PREFS.set(prefs).is_ok() {
        true
    } else {
        debug!("encoder preferences already configured");
        false
    }
}

pub fn prefs() -> EncodePrefs {
    PREFS
        .get()
        .unwrap_or_else(|| ENV_PREFS.get_or_init(EncodePrefs::from_env))
        .clone()
}

struct Apartment;

impl Apartment {
    fn enter() -> Result<Self, PlatformError> {
        // SAFETY: called once on a newly spawned, dedicated worker thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| PlatformError::Platform(e.to_string()))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances this thread's successful CoInitializeEx, after all
        // worker/probe locals and the final mailbox frame have been dropped.
        unsafe { CoUninitialize() };
    }
}

pub fn warm_up() {
    PROBE_STARTED.get_or_init(|| {
        if let Err(error) = thread::Builder::new()
            .name("rs-encode-probe".into())
            .spawn(|| {
                let result = catch_unwind(AssertUnwindSafe(run_probe));
                let table = match result {
                    Ok(Ok(table)) => table,
                    Ok(Err(error)) => {
                        warn!(%error, "encoder probe failed");
                        ProbeTable::default()
                    }
                    Err(_) => {
                        error!("encoder probe panicked");
                        ProbeTable::default()
                    }
                };
                *lock(&PROBES) = Some(table);
                PROBE_READY.notify_all();
            })
        {
            warn!(%error, "cannot start encoder probe");
            *lock(&PROBES) = Some(ProbeTable::default());
            PROBE_READY.notify_all();
        }
    });
}

pub fn probe_table() -> Option<ProbeTable> {
    lock(&PROBES).clone()
}

fn record_unsupported(luid: i64, vendor_id: u32, backend: Backend, codec: Codec) {
    if let Some(table) = lock(&PROBES).as_mut() {
        table.record(ProbeEntry {
            luid,
            vendor_id,
            has_output: true,
            backend,
            codec,
            result: ProbeResult::Unsupported,
        });
    }
}

pub fn wait_probe(timeout: Duration) -> Option<ProbeTable> {
    let (table, _) = PROBE_READY
        .wait_timeout_while(lock(&PROBES), timeout, |table| table.is_none())
        .unwrap_or_else(|error| error.into_inner());
    table.clone()
}

// Both probing and real sessions use this opening path, including retry options.
enum NativeEncoder {
    Ff(FfEncoder),
    Mf(Box<MfEncoder>),
}

#[derive(Debug, thiserror::Error)]
enum NativeError {
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
    #[error("encoder input busy")]
    Busy,
}

impl From<FfError> for NativeError {
    fn from(value: FfError) -> Self {
        match value {
            FfError::Unsupported(message) => Self::Unsupported(message),
            error => Self::Failed(error.to_string()),
        }
    }
}

impl From<MfError> for NativeError {
    fn from(value: MfError) -> Self {
        match value {
            MfError::Unsupported(message) => Self::Unsupported(message),
            MfError::Busy => Self::Busy,
            error => Self::Failed(error.to_string()),
        }
    }
}

fn open_native(
    backend: Backend,
    params: &EncParams,
    device: &ID3D11Device,
    luid: i64,
    vendor: u32,
) -> Result<NativeEncoder, NativeError> {
    if backend == Backend::MfHw {
        mf_startup().map_err(|e| NativeError::Failed(e.to_string()))?;
        return MfEncoder::open(&MfOpen {
            device,
            adapter_luid: luid,
            vendor_id: vendor,
            codec: params.codec,
            width: params.width,
            height: params.height,
            fps: params.fps,
            bitrate_bps: params.bitrate_bps,
        })
        .map(|enc| NativeEncoder::Mf(Box::new(enc)))
        .map_err(Into::into);
    }
    let lib = FfLib::shared().map_err(|e| NativeError::Failed(e.to_string()))?;
    let codec_name = ffmpeg_codec_name(backend, params.codec)
        .ok_or_else(|| NativeError::Unsupported("unknown FFmpeg codec".into()))?;
    let open = |options: &[host_core::encode_model::FfOpt]| {
        let options: Vec<_> = options
            .iter()
            .map(|o| (o.name, o.value.as_str(), o.required))
            .collect();
        FfEncoder::open(
            &lib,
            &FfOpen {
                codec_name,
                hw: if backend == Backend::FfQsv {
                    HwPath::Qsv
                } else {
                    HwPath::D3d11
                },
                width: params.width,
                height: params.height,
                device,
                options: &options,
            },
        )
    };
    let mut result = open(&ffmpeg_options(backend, params));
    if result.is_err() {
        if let Some(options) = ffmpeg_options_retry(backend, params) {
            result = open(&options);
        }
    }
    result.map(NativeEncoder::Ff).map_err(Into::into)
}

impl NativeEncoder {
    fn name(&self) -> &str {
        match self {
            Self::Ff(enc) => enc.codec_name(),
            Self::Mf(enc) => enc.name(),
        }
    }

    fn poll(&mut self) -> Result<Vec<FfPacket>, NativeError> {
        match self {
            Self::Ff(_) => Ok(Vec::new()),
            Self::Mf(enc) => enc
                .poll()
                .map(|packets| {
                    packets
                        .into_iter()
                        .map(|p| FfPacket {
                            data: p.data,
                            pts: p.pts,
                            keyframe: p.keyframe,
                        })
                        .collect()
                })
                .map_err(Into::into),
        }
    }
}

fn probe_one(
    backend: Backend,
    codec: Codec,
    device: &ID3D11Device,
    luid: i64,
    vendor: u32,
) -> ProbeResult {
    let Ok(params) = validate(codec, PROBE_SIZE.0, PROBE_SIZE.1, PROBE_FPS, PROBE_KBPS) else {
        return ProbeResult::Failed;
    };
    let mut enc = match open_native(backend, &params, device, luid, vendor) {
        Ok(enc) => enc,
        Err(NativeError::Unsupported(_)) => return ProbeResult::Unsupported,
        Err(_) => return ProbeResult::Failed,
    };
    let start = Instant::now();
    let mut pts = 0;
    let mut key = false;
    while start.elapsed() < PROBE_FRAME_WAIT {
        let result = match &mut enc {
            NativeEncoder::Ff(ff) if pts < PROBE_FRAMES => {
                let result = ff.encode(pts, pts == 0, None).map_err(NativeError::from);
                pts += 1;
                result
            }
            NativeEncoder::Mf(mf) if pts < PROBE_FRAMES => {
                if let Some((slot, _texture)) = mf.input_slot() {
                    match mf.encode(slot, pts, pts == 0, None) {
                        Ok(packets) => {
                            pts += 1;
                            Ok(packets
                                .into_iter()
                                .map(|p| FfPacket {
                                    data: p.data,
                                    pts: p.pts,
                                    keyframe: p.keyframe,
                                })
                                .collect())
                        }
                        Err(MfError::Busy) => enc.poll(),
                        Err(error) => Err(error.into()),
                    }
                } else {
                    enc.poll()
                }
            }
            _ => enc.poll(),
        };
        match result {
            Ok(packets) => {
                key |= packets
                    .iter()
                    .any(|p| p.keyframe || au_is_key(codec, &p.data));
                if key && pts == PROBE_FRAMES {
                    return ProbeResult::Ok;
                }
            }
            Err(_) => return ProbeResult::Failed,
        }
        if pts == PROBE_FRAMES || backend == Backend::MfHw {
            thread::sleep(Duration::from_millis(1));
        }
    }
    ProbeResult::Failed
}

fn run_probe() -> Result<ProbeTable, PlatformError> {
    let _apartment = Apartment::enter()?;
    let mf_ok = mf_startup().is_ok();
    let lib = FfLib::shared();
    match &lib {
        Ok(lib) => info!(
            avcodec = lib.versions().avcodec,
            avutil = lib.versions().avutil,
            "ffmpeg available"
        ),
        Err(_) => info!("ffmpeg unavailable"),
    }
    let mut table = ProbeTable::default();
    for adapter in vproc::adapters()?
        .into_iter()
        .filter(|a| a.has_output && a.vendor_id != VENDOR_MICROSOFT_BASIC)
    {
        let device = vproc::create_device(adapter.luid);
        for backend in probe_plan(adapter.vendor_id) {
            for codec in [Codec::Hevc, Codec::H264] {
                let started = Instant::now();
                let result = match &device {
                    Err(_) => ProbeResult::Failed,
                    Ok(_) if backend == Backend::MfHw && !mf_ok => ProbeResult::Failed,
                    Ok(device) => {
                        if backend.is_ffmpeg() {
                            let Ok(lib) = &lib else { continue };
                            if !ffmpeg_codec_name(backend, codec)
                                .is_some_and(|name| lib.has_encoder(name))
                            {
                                ProbeResult::Unsupported
                            } else {
                                probe_one(backend, codec, device, adapter.luid, adapter.vendor_id)
                            }
                        } else {
                            probe_one(backend, codec, device, adapter.luid, adapter.vendor_id)
                        }
                    }
                };
                let entry = ProbeEntry {
                    luid: adapter.luid,
                    vendor_id: adapter.vendor_id,
                    has_output: true,
                    backend,
                    codec,
                    result,
                };
                info!(
                    ?entry,
                    elapsed_ms = started.elapsed().as_millis(),
                    "encoder probe"
                );
                table.record(entry);
            }
        }
    }
    Ok(table)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncoderStatus {
    pub backend: Option<Backend>,
    pub encoder_name: String,
    pub adapter_luid: i64,
    pub forced_fallback: bool,
    pub stats: EncodeStats,
    pub busy_dropped: u64,
    pub failed_dropped: u64,
    pub capture_to_encoded_p50_us: Option<u64>,
    pub capture_to_encoded_p95_us: Option<u64>,
    pub submit_to_encoded_p95_us: Option<u64>,
    pub last_error: Option<String>,
}

struct Inbox {
    mailbox: Mailbox<CapturedFrame>,
    force_key: bool,
    rate: Option<(u32, u32)>,
    boost: bool,
    stop: bool,
    dead: Option<String>,
}

struct Shared {
    inbox: Mutex<Inbox>,
    ready: Condvar,
    status: Mutex<EncoderStatus>,
}

pub struct WinEncoder {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    done: mpsc::Receiver<()>,
}

impl WinEncoder {
    pub fn status(&self) -> EncoderStatus {
        lock(&self.shared.status).clone()
    }
}

impl Encoder for WinEncoder {
    fn encode(
        &mut self,
        frame: CapturedFrame,
        opts: EncodeOpts,
    ) -> host_core::platform::Result<()> {
        if frame.native.downcast_ref::<WinFrame>().is_none() {
            return Err(PlatformError::Platform(
                "not a Windows capture frame".into(),
            ));
        }
        let displaced = {
            let mut inbox = lock(&self.shared.inbox);
            if let Some(message) = &inbox.dead {
                return Err(PlatformError::Platform(message.clone()));
            }
            inbox.mailbox.put(
                frame,
                JobFlags {
                    force_key: opts.force_keyframe,
                    refine: opts.quality_boost,
                },
            )
        };
        self.shared.ready.notify_one();
        drop(displaced);
        Ok(())
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        lock(&self.shared.inbox).rate = Some((kbps, fps));
        self.shared.ready.notify_one();
    }

    fn force_keyframe(&mut self) {
        lock(&self.shared.inbox).force_key = true;
        self.shared.ready.notify_one();
    }

    fn set_quality_boost(&mut self, on: bool) {
        lock(&self.shared.inbox).boost = on;
        self.shared.ready.notify_one();
    }
}

impl Drop for WinEncoder {
    fn drop(&mut self) {
        lock(&self.shared.inbox).stop = true;
        self.shared.ready.notify_one();
        if self.done.recv_timeout(DROP_WAIT).is_ok() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        } else {
            warn!("encoder worker did not stop within one second; detaching");
        }
    }
}

pub fn create_encoder(cfg: EncoderConfig) -> Result<WinEncoder, PlatformError> {
    let params =
        validate(cfg.codec, cfg.width, cfg.height, cfg.fps, cfg.bitrate_kbps).map_err(|error| {
            match error {
                ParamError::Codec | ParamError::Size { .. } => PlatformError::Unsupported,
                error => PlatformError::Platform(error.to_string()),
            }
        })?;
    warm_up();
    if wait_probe(PROBE_WAIT).is_some_and(|t| t.codec_supported(cfg.codec) == Some(false)) {
        info!(codec = ?cfg.codec, "codec unavailable on output adapters");
        return Err(PlatformError::Unsupported);
    }
    let handle = Handle::try_current().ok();
    let shared = Arc::new(Shared {
        inbox: Mutex::new(Inbox {
            mailbox: Mailbox::new(),
            force_key: false,
            rate: None,
            boost: true,
            stop: false,
            dead: None,
        }),
        ready: Condvar::new(),
        status: Mutex::new(EncoderStatus::default()),
    });
    let (done_tx, done) = mpsc::channel();
    let state = Arc::clone(&shared);
    let worker = thread::Builder::new()
        .name(format!(
            "rs-encode-{}",
            NEXT_WORKER.fetch_add(1, Ordering::Relaxed)
        ))
        .spawn(move || {
            // The apartment outlives catch_unwind, including Worker unwinding
            // and the terminal mailbox drain below.
            let apartment = Apartment::enter();
            let result = match &apartment {
                Ok(_) => catch_unwind(AssertUnwindSafe(|| {
                    Worker::new(Arc::clone(&state), cfg, params, handle).run()
                })),
                Err(error) => Ok(Some(error.to_string())),
            };
            let message = match result {
                Ok(message) => message.unwrap_or_else(|| "encoder worker stopped".into()),
                Err(_) => {
                    error!("encoder worker panicked");
                    "encoder worker panicked".into()
                }
            };
            let pending = {
                let mut inbox = lock(&state.inbox);
                inbox.dead = Some(message.clone());
                inbox.mailbox.take()
            };
            drop(pending);
            let mut status = lock(&state.status);
            if message != "encoder worker stopped" {
                status.last_error = Some(message);
            }
            info!(?status, "encoder stopped");
            drop(status);
            drop(apartment);
            let _ = done_tx.send(());
        })
        .map_err(|error| PlatformError::Platform(error.to_string()))?;
    info!(codec = ?params.codec, w = params.width, h = params.height, fps = params.fps,
        bps = params.bitrate_bps, pref = ?prefs().encoder, "encoder worker created");
    Ok(WinEncoder {
        shared,
        worker: Some(worker),
        done,
    })
}

pub fn open_encoder(cfg: EncoderConfig) -> host_core::platform::Result<Box<dyn Encoder>> {
    create_encoder(cfg).map(|encoder| Box::new(encoder) as Box<dyn Encoder>)
}

struct Session {
    backend: Backend,
    enc: NativeEncoder,
    name: String,
    luid: i64,
    vendor: u32,
    device: ID3D11Device,
    source: SourceKey,
    vp: Option<VideoProc>,
    // MfEncoder does not expose its flow count. Count accepted input/output
    // here to avoid polling an idle MFT forever.
    in_flight: usize,
}

struct Worker {
    shared: Arc<Shared>,
    cfg: EncoderConfig,
    handle: Option<Handle>,
    origin: Instant,
    params: EncParams,
    target_bps: u64,
    session: Option<Session>,
    chooser: BackendChooser,
    fail: FailureState,
    rate: RateApplier,
    key: KeyTracker,
    gate: OutputGate,
    pts: PtsMap,
    psc: ParamSetCache,
    retained: Option<CapturedFrame>,
    retry_pts: Option<i64>,
    lost_gen: Option<u64>,
    last_source: Option<SourceKey>,
    last_adapter: (i64, u32),
    cap_lat: LatencyWindow,
    sub_lat: LatencyWindow,
    status: EncoderStatus,
    opened: bool,
    boost: bool,
    dead: Option<String>,
}

impl Worker {
    fn new(
        shared: Arc<Shared>,
        cfg: EncoderConfig,
        params: EncParams,
        handle: Option<Handle>,
    ) -> Self {
        Self {
            shared,
            cfg,
            handle,
            origin: Instant::now(),
            params,
            target_bps: params.bitrate_bps,
            session: None,
            chooser: BackendChooser::new(),
            fail: FailureState::new(),
            rate: RateApplier::new(RateCost::Free, params.bitrate_bps),
            key: KeyTracker::new(),
            gate: OutputGate::new(),
            pts: PtsMap::new(),
            psc: ParamSetCache::new(params.codec),
            retained: None,
            retry_pts: None,
            lost_gen: None,
            last_source: None,
            last_adapter: (0, 0),
            cap_lat: LatencyWindow::new(),
            sub_lat: LatencyWindow::new(),
            status: EncoderStatus::default(),
            opened: false,
            boost: true,
            dead: None,
        }
    }

    fn now(&self) -> u64 {
        self.origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }

    fn retry_due(&self, now: u64) -> bool {
        self.lost_gen.is_none()
            && self.retained.is_some()
            && self.session.as_ref().is_none_or(|s| s.vp.is_none())
            && self.fail.retry_at().is_some_and(|at| now >= at)
    }

    fn deadline(&self) -> Option<u64> {
        let key = self.retained.as_ref().and(self.key.deadline());
        let retry = if self.lost_gen.is_none()
            && self.retained.is_some()
            && self.session.as_ref().is_none_or(|s| s.vp.is_none())
        {
            self.fail.retry_at()
        } else {
            None
        };
        let poll = self
            .session
            .as_ref()
            .filter(|s| s.backend == Backend::MfHw && s.in_flight != 0)
            .map(|_| self.now().saturating_add(MF_POLL.as_micros() as u64));
        [key, retry, self.rate.deadline(), poll]
            .into_iter()
            .flatten()
            .min()
    }

    fn run(mut self) -> Option<String> {
        loop {
            let deadline = self.deadline();
            let (stop, job, force, rate, boost, dropped) = {
                let mut inbox = lock(&self.shared.inbox);
                while !inbox.stop
                    && inbox.mailbox.is_empty()
                    && !inbox.force_key
                    && inbox.rate.is_none()
                {
                    if let Some(at) = deadline {
                        let now = self.now();
                        if now >= at {
                            break;
                        }
                        let (guard, timeout) = self
                            .shared
                            .ready
                            .wait_timeout(inbox, Duration::from_micros(at - now))
                            .unwrap_or_else(|error| error.into_inner());
                        inbox = guard;
                        if timeout.timed_out() {
                            break;
                        }
                    } else {
                        inbox = self
                            .shared
                            .ready
                            .wait(inbox)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                }
                (
                    inbox.stop,
                    inbox.mailbox.take(),
                    std::mem::take(&mut inbox.force_key),
                    inbox.rate.take(),
                    inbox.boost,
                    inbox.mailbox.dropped(),
                )
            };
            self.status.stats.mailbox_dropped = dropped;
            if stop {
                break;
            }
            self.boost = boost;
            let now = self.now();
            if force {
                self.key.request(now);
            }
            if let Some((kbps, fps)) = rate {
                self.target_bps = effective_bps(kbps, fps, self.params.fps);
                if self.session.is_some() {
                    let from = self.rate.current();
                    if let Some(bps) = self.rate.request(now, self.target_bps) {
                        self.apply_rate(from, bps);
                    }
                }
            }
            let from = self.rate.current();
            if let Some(bps) = self.rate.poll(now) {
                self.apply_rate(from, bps);
            }
            if let Some((frame, flags)) = job {
                self.process(&frame, flags, false);
                // Never re-retain the device just identified as lost.
                if frame
                    .native
                    .downcast_ref::<WinFrame>()
                    .is_some_and(|wf| self.lost_gen != Some(wf.device_gen))
                {
                    self.retained = Some(frame);
                }
            } else if self.retry_due(now) {
                if let Some(frame) = self.retained.clone() {
                    self.process(
                        &frame,
                        JobFlags {
                            force_key: true,
                            refine: false,
                        },
                        true,
                    );
                }
            } else if self.key.poll(now) {
                if let Some(frame) = self.retained.clone() {
                    if frame.native.downcast_ref::<WinFrame>().is_some_and(|wf| {
                        self.session
                            .as_ref()
                            .is_some_and(|s| s.source.device_gen == wf.device_gen)
                    }) {
                        self.status.stats.idle_key_retries += 1;
                        self.process(
                            &frame,
                            JobFlags {
                                force_key: true,
                                refine: false,
                            },
                            true,
                        );
                    }
                }
            } else {
                self.poll();
            }
            self.publish();
            if self.dead.is_some() {
                break;
            }
        }
        self.publish();
        self.dead.take()
        // self, including the session and retained frame, drops here on worker.
    }

    fn publish(&mut self) {
        self.status.capture_to_encoded_p50_us = self.cap_lat.percentile(50);
        self.status.capture_to_encoded_p95_us = self.cap_lat.percentile(95);
        self.status.submit_to_encoded_p95_us = self.sub_lat.percentile(95);
        *lock(&self.shared.status) = self.status.clone();
    }

    fn failure(&mut self, message: String) {
        let now = self.now();
        self.status.stats.failures += 1;
        self.status.last_error = Some(message);
        self.fail.on_failure(now);
        if self.fail.should_report(now) {
            warn!(backend = ?self.status.backend, luid = self.last_adapter.0,
                vendor = self.last_adapter.1,
                codec = ?self.params.codec, excluded = ?self.chooser.excluded(),
                last_error = ?self.status.last_error, "encoder failure; will retry");
        }
    }

    fn candidates(&mut self, wf: &WinFrame) -> Vec<Backend> {
        let (list, forced) = self.chooser.choose(
            wf.vendor_id,
            wf.adapter_luid,
            &prefs().encoder,
            self.params.codec,
            FfLib::shared().is_ok(),
            &probe_table().unwrap_or_default(),
        );
        if forced && !self.status.forced_fallback {
            warn!("forced encoder unavailable on this adapter; using auto");
            self.status.forced_fallback = true;
        }
        list
    }

    fn open_session(&mut self, wf: &WinFrame, source: SourceKey) -> bool {
        let mut list = self.candidates(wf);
        if list.is_empty() && self.fail.failed() {
            self.chooser.reset();
            list = self.candidates(wf);
        }
        for _ in 0..2 {
            for backend in list {
                let started = Instant::now();
                let params = EncParams {
                    bitrate_bps: self.target_bps,
                    ..self.params
                };
                match open_native(backend, &params, &wf.device, wf.adapter_luid, wf.vendor_id) {
                    Ok(enc) => {
                        let name = enc.name().to_owned();
                        info!(
                            backend = backend.name(),
                            name,
                            luid = wf.adapter_luid,
                            w = params.width,
                            h = params.height,
                            fps = params.fps,
                            bps = params.bitrate_bps,
                            open_ms = started.elapsed().as_millis(),
                            "encoder opened"
                        );
                        self.session = Some(Session {
                            backend,
                            enc,
                            name: name.clone(),
                            luid: wf.adapter_luid,
                            vendor: wf.vendor_id,
                            device: wf.device.clone(),
                            source,
                            vp: None,
                            in_flight: 0,
                        });
                        self.rate.reset(rate_cost(backend), params.bitrate_bps);
                        self.reopened();
                        self.status.backend = Some(backend);
                        self.status.encoder_name = name;
                        self.status.adapter_luid = wf.adapter_luid;
                        return true;
                    }
                    Err(error) => {
                        self.chooser.exclude(backend);
                        if matches!(error, NativeError::Unsupported(_)) {
                            record_unsupported(
                                wf.adapter_luid,
                                wf.vendor_id,
                                backend,
                                params.codec,
                            );
                        }
                        debug!(?backend, %error, "encoder open failed");
                        self.status.last_error = Some(error.to_string());
                    }
                }
            }
            // A failed forced candidate must get an automatic pass immediately.
            list = self.candidates(wf);
            if list.is_empty() {
                break;
            }
        }
        self.failure(
            self.status
                .last_error
                .clone()
                .unwrap_or_else(|| "no available encoder backend".into()),
        );
        false
    }

    fn reopened(&mut self) {
        self.gate.on_reopen();
        self.key.request(self.now());
        self.psc.clear();
        self.pts.clear();
        self.retry_pts = None;
        if self.opened {
            self.status.stats.reopens += 1;
        }
        self.opened = true;
    }

    fn process(&mut self, frame: &CapturedFrame, flags: JobFlags, retry: bool) {
        let Some(wf) = frame.native.downcast_ref::<WinFrame>() else {
            return;
        };
        if self.lost_gen == Some(wf.device_gen) {
            self.status.failed_dropped += 1;
            return;
        }
        let source = SourceKey {
            device_gen: wf.device_gen,
            width: frame.width,
            height: frame.height,
            rotation: wf.rotation,
        };
        if self
            .last_source
            .is_some_and(|old| old.device_gen != source.device_gen)
            || self.lost_gen.is_some()
        {
            self.chooser.reset();
            self.fail.on_device_change();
            self.lost_gen = None;
            self.retained = None;
        }
        self.last_source = Some(source);
        self.last_adapter = (wf.adapter_luid, wf.vendor_id);
        match source_change(self.session.as_ref().map(|s| s.source), source) {
            SourceChange::Reopen => {
                self.session = None;
            }
            SourceChange::Reproc => {
                if let Some(session) = &mut self.session {
                    session.vp = None;
                    session.source = source;
                }
            }
            SourceChange::Same => {}
        }
        if self.session.as_ref().is_none_or(|s| s.vp.is_none())
            && self.fail.retry_at().is_some_and(|at| self.now() < at)
        {
            self.status.failed_dropped += 1;
            return;
        }
        if self.session.is_none() && !self.open_session(wf, source) {
            self.status.failed_dropped += 1;
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.vp.is_none() {
            match VideoProc::new(
                &wf.device,
                VpKey {
                    device_gen: wf.device_gen,
                    in_w: frame.width,
                    in_h: frame.height,
                    out_w: self.params.width,
                    out_h: self.params.height,
                    rotation: wf.rotation,
                },
                self.params.fps,
            ) {
                Ok(vp) => session.vp = Some(vp),
                Err(error) => {
                    // The FFmpeg adapter represents unsupported capabilities with
                    // Platform("unsupported: ..."), not the Unsupported variant.
                    let unsupported = matches!(error, PlatformError::Unsupported)
                        || matches!(&error, PlatformError::Platform(s) if s.starts_with("unsupported:"));
                    if unsupported {
                        self.failure("capture device lacks video support".into());
                    } else {
                        self.runtime_error(error.to_string(), &wf.device, wf.device_gen);
                    }
                    self.status.failed_dropped += 1;
                    return;
                }
            }
        }
        let idr = self.key.take_for_frame() || flags.force_key || retry;
        let action = refine_action(
            refine_mode(session.backend),
            flags.refine && !retry,
            idr,
            self.boost,
            self.rate.current(),
        );
        if action == RefineAction::Skip {
            return;
        }
        let meta = FrameMeta {
            capture_ts_us: frame.capture_ts_us,
            refine: matches!(
                action,
                RefineAction::Repeat | RefineAction::Roi(..) | RefineAction::Bump(_)
            ),
            submit_us: unix_micros(),
        };
        let Some(vp) = session.vp.as_mut() else {
            return;
        };
        let mut submitted_pts = None;
        let result = (|| -> Result<Vec<FfPacket>, NativeError> {
            match &mut session.enc {
                NativeEncoder::Ff(ff) => {
                    vp.blit(&wf.texture, &ff.input().texture, ff.input().array_index)
                        .map_err(|e| NativeError::Failed(e.to_string()))?;
                    let pts = self.pts.next_pts(meta);
                    submitted_pts = Some(pts);
                    let roi = match action {
                        RefineAction::Roi(a, b) => Some((a, b)),
                        _ => None,
                    };
                    ff.encode(pts, idr, roi).map_err(Into::into)
                }
                NativeEncoder::Mf(mf) => {
                    let (slot, texture) = mf.input_slot().ok_or(NativeError::Busy)?;
                    vp.blit(&wf.texture, &texture, 0)
                        .map_err(|e| NativeError::Failed(e.to_string()))?;
                    let pts = self.pts.next_pts(meta);
                    submitted_pts = Some(pts);
                    let bump = match action {
                        RefineAction::Bump(bps) => Some(bps),
                        _ => None,
                    };
                    mf.encode(slot, pts, idr, bump)
                        .map(|packets| {
                            packets
                                .into_iter()
                                .map(|p| FfPacket {
                                    data: p.data,
                                    pts: p.pts,
                                    keyframe: p.keyframe,
                                })
                                .collect()
                        })
                        .map_err(Into::into)
                }
            }
        })();
        // frame remains borrowed through both GPU command submissions above.
        match result {
            Ok(packets) => {
                if retry {
                    self.retry_pts = submitted_pts;
                }
                session.in_flight += 1;
                self.status.stats.submitted += 1;
                trace!(pts = ?submitted_pts, idr, retry, "encoder submitted");
                self.deliver(packets);
            }
            Err(NativeError::Busy) => {
                self.status.busy_dropped += 1;
                if idr {
                    self.key.request(self.now());
                }
                self.poll();
            }
            Err(error) if error.to_string().contains("vp input size changed") => {
                session.vp = None;
                if idr {
                    self.key.request(self.now());
                }
            }
            Err(error) => self.runtime_error(error.to_string(), &wf.device, wf.device_gen),
        }
    }

    fn poll(&mut self) {
        let Some(session) = self.session.as_mut().filter(|s| s.backend == Backend::MfHw) else {
            return;
        };
        let result = session.enc.poll();
        match result {
            Ok(packets) => self.deliver(packets),
            Err(NativeError::Busy) => {}
            Err(error) => {
                let device = session.device.clone();
                let generation = session.source.device_gen;
                self.runtime_error(error.to_string(), &device, generation);
            }
        }
    }

    fn runtime_error(&mut self, message: String, device: &ID3D11Device, generation: u64) {
        // SAFETY: device is kept alive by the current frame/session local, and
        // this read only queries the D3D11 device removal status.
        let removed = unsafe { device.GetDeviceRemovedReason() };
        if let Err(error) = removed {
            self.session = None;
            self.retained = None;
            self.pts.clear();
            self.lost_gen = Some(generation);
            let now = self.now();
            self.status.stats.failures += 1;
            self.status.last_error = Some(format!("D3D11 device lost: {error}; {message}"));
            self.fail.on_failure(now);
            if self.fail.should_report(now) {
                warn!(
                    hr = error.code().0,
                    "D3D11 device lost; waiting for capture to rebuild"
                );
            }
        } else {
            if let Some(session) = self.session.take() {
                self.chooser.exclude(session.backend);
            }
            self.failure(message);
        }
    }

    fn apply_rate(&mut self, from: u64, bps: u64) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        let backend = session.backend;
        let mut reopen = rate_cost(backend) == RateCost::Reopen;
        let result = match &mut session.enc {
            NativeEncoder::Mf(mf) => mf.set_bitrate(bps).map_err(NativeError::from),
            NativeEncoder::Ff(ff) if !reopen => {
                let options = ffmpeg_rate_options(backend, bps, self.params.fps);
                let options: Vec<_> = options
                    .iter()
                    .map(|o| (o.name, o.value.as_str(), o.required))
                    .collect();
                if ff.set_options(&options).is_err() {
                    reopen = true;
                }
                Ok(())
            }
            NativeEncoder::Ff(_) => Ok(()),
        };
        if let Err(error) = result {
            let device = session.device.clone();
            let generation = session.source.device_gen;
            self.session = Some(session);
            self.runtime_error(error.to_string(), &device, generation);
            return;
        }
        if reopen {
            // Move out the encoder so it is destroyed BEFORE opening the new
            // one; keep the processor (including its bounded view cache).
            let Session {
                enc,
                name: _,
                backend,
                luid,
                vendor,
                device,
                source,
                vp,
                ..
            } = session;
            drop(enc);
            let params = EncParams {
                bitrate_bps: bps,
                ..self.params
            };
            match open_native(backend, &params, &device, luid, vendor) {
                Ok(enc) => {
                    let name = enc.name().to_owned();
                    session = Session {
                        enc,
                        name,
                        backend,
                        luid,
                        vendor,
                        device,
                        source,
                        vp,
                        in_flight: 0,
                    };
                    self.reopened();
                }
                Err(error) => {
                    drop(vp);
                    self.chooser.exclude(backend);
                    if matches!(error, NativeError::Unsupported(_)) {
                        record_unsupported(luid, vendor, backend, params.codec);
                    }
                    self.failure(error.to_string());
                    return;
                }
            }
        }
        self.status.encoder_name = session.name.clone();
        self.session = Some(session);
        self.status.stats.rate_applied += 1;
        debug!(from, to = bps, ?backend, "encoder rate applied");
    }

    fn deliver(&mut self, packets: Vec<FfPacket>) {
        if let Some(session) = &mut self.session {
            session.in_flight = session.in_flight.saturating_sub(packets.len());
        }
        for packet in packets {
            let meta = self.pts.take(packet.pts).unwrap_or_else(|| {
                debug!(pts = packet.pts, "encoder packet has no capture metadata");
                let now = unix_micros();
                FrameMeta {
                    capture_ts_us: now,
                    refine: false,
                    submit_us: now,
                }
            });
            let keyframe = packet.keyframe || au_is_key(self.params.codec, &packet.data);
            let data = self.psc.fix(&packet.data, keyframe);
            if !self.gate.admit(keyframe) {
                self.status.stats.gated += 1;
                self.key.request(self.now());
                continue;
            }
            let encoded_ts_us = unix_micros();
            self.sub_lat
                .push(encoded_ts_us.saturating_sub(meta.submit_us));
            if !meta.refine && Some(packet.pts) != self.retry_pts {
                self.cap_lat
                    .push(encoded_ts_us.saturating_sub(meta.capture_ts_us));
            }
            let still_refine = meta.refine && !keyframe;
            let frame = EncodedFrame {
                data,
                keyframe,
                still_refine,
                capture_ts_us: meta.capture_ts_us,
                encoded_ts_us,
            };
            let sent = match self.cfg.sink.try_send(frame) {
                Ok(()) => true,
                Err(TrySendError::Closed(_)) => {
                    self.dead = Some("encoder output closed".into());
                    break;
                }
                Err(TrySendError::Full(frame)) => {
                    if let Some(handle) = &self.handle {
                        // Construct timeout INSIDE the entered runtime. This is
                        // a dedicated std thread, never the caller's Tokio task.
                        match handle.block_on(async {
                            tokio::time::timeout(SINK_WAIT, self.cfg.sink.send(frame)).await
                        }) {
                            Ok(Ok(())) => true,
                            Ok(Err(_)) => {
                                self.dead = Some("encoder output closed".into());
                                break;
                            }
                            Err(_) => false,
                        }
                    } else {
                        false
                    }
                }
            };
            if sent {
                self.status.stats.encoded += 1;
                self.status.stats.keyframes += u64::from(keyframe);
                self.status.stats.refines += u64::from(still_refine);
                self.fail.on_success();
            } else {
                self.status.stats.sink_dropped += 1;
                self.gate.on_sink_drop();
                self.key.request(self.now());
            }
        }
    }
}
