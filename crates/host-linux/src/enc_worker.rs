//! Backend-independent Linux encoder worker.
//!
//! Capture callbacks only replace the mailbox item. Backend calls, displaced
//! frame releases, bounded sink waits and retries happen on a dedicated thread.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::encode_model::{
    au_is_key, effective_bps, refine_action, source_change, validate, EncParams, EncodeStats,
    FailureState, FrameMeta, JobFlags, KeyTracker, Mailbox, OutputGate, ParamError, ParamSetCache,
    PtsMap, RateApplier, RateCost, RefineAction, RefineMode, SourceChange, SourceKey,
};
use host_core::platform::{
    CapturedFrame, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, PlatformError,
};
use rsp_core::clock::unix_micros;
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, info, warn};

use crate::frame::LinuxFrame;

pub const SINK_WAIT: Duration = Duration::from_millis(50);
pub const DROP_WAIT: Duration = Duration::from_secs(1);

/// One backend-produced Annex-B access unit, possibly from an earlier input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// Permanently exclude this backend from the worker's chain.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Retry the same backend after the model's reopen backoff.
    #[error("encoder failed: {0}")]
    Failed(String),
}

/// Input dimensions; probing before the first capture uses the encoding size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrcSize {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

pub trait EncBackend: Send {
    fn name(&self) -> &'static str;
    fn rate_cost(&self) -> RateCost;
    /// The source matches the dimensions used to open this backend. `idr`
    /// requires an IDR. Return all available packets, including delayed ones.
    fn encode(
        &mut self,
        frame: &LinuxFrame,
        pts: i64,
        idr: bool,
    ) -> Result<Vec<EncPacket>, BackendError>;
    /// Only called for Free/Idr backends. Reopen backends are opened again.
    fn set_rate(&mut self, bitrate_bps: u64, fps: u32) -> Result<(), BackendError>;
}

pub trait BackendFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn open(&self, params: &EncParams, src: SrcSize) -> Result<Box<dyn EncBackend>, BackendError>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerStatus {
    pub backend: Option<&'static str>,
    pub stats: EncodeStats,
    pub dead: Option<String>,
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
    status: Mutex<WorkerStatus>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

impl Shared {
    fn mark_dead(&self, message: String) {
        let pending = {
            let mut inbox = lock(&self.inbox);
            inbox.dead = Some(message.clone());
            lock(&self.status).dead = Some(message);
            inbox.mailbox.take()
        };
        drop(pending);
    }
}

pub struct EncWorker {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    done: mpsc::Receiver<()>,
}

impl EncWorker {
    /// Factories are tried in priority order. Probe failures are permanently
    /// removed. A full sink waits at most SINK_WAIT when a runtime is supplied;
    /// without one it drops immediately.
    pub fn start(
        cfg: EncoderConfig,
        factories: Vec<Arc<dyn BackendFactory>>,
        handle: Option<Handle>,
    ) -> Result<Self, PlatformError> {
        let params = validate(cfg.codec, cfg.width, cfg.height, cfg.fps, cfg.bitrate_kbps)
            .map_err(|error| match error {
                ParamError::Codec | ParamError::Size { .. } => PlatformError::Unsupported,
                error => PlatformError::Platform(error.to_string()),
            })?;
        let src = SrcSize {
            width: params.width,
            height: params.height,
            stride: params.width * 4,
        };
        let mut chain = factories.into_iter();
        let mut errors = Vec::new();
        let (first_factory, backend) = loop {
            let Some(factory) = chain.next() else {
                info!(errors = ?errors, "no usable Linux encoder backend during probe");
                return Err(PlatformError::Unsupported);
            };
            match factory.open(&params, src) {
                Ok(backend) => break (factory, backend),
                Err(error) => errors.push((factory.name(), error.to_string())),
            }
        };
        let mut factories = vec![first_factory];
        factories.extend(chain);
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
            status: Mutex::new(WorkerStatus {
                backend: Some(backend.name()),
                ..WorkerStatus::default()
            }),
        });
        let state = Arc::clone(&shared);
        let (done_tx, done) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("rs-linux-enc".into())
            .spawn(move || {
                // Match the Windows worker's panic boundary: even a backend
                // panic makes later submissions fail and releases the mailbox.
                let result = catch_unwind(AssertUnwindSafe(|| {
                    Worker::new(
                        Arc::clone(&state),
                        cfg,
                        params,
                        factories,
                        backend,
                        src,
                        handle,
                    )
                    .run();
                }));
                if result.is_err() {
                    warn!("Linux encoder worker panicked");
                    state.mark_dead("encoder worker panicked".into());
                }
                let pending = lock(&state.inbox).mailbox.take();
                drop(pending);
                // Worker, backend, retained frame and sink have all dropped
                // before acknowledging shutdown to EncWorker::drop.
                let _ = done_tx.send(());
            })
            .map_err(|error| PlatformError::Platform(error.to_string()))?;
        Ok(Self {
            shared,
            worker: Some(worker),
            done,
        })
    }

    pub fn status(&self) -> WorkerStatus {
        let mut status = lock(&self.shared.status).clone();
        status.stats.mailbox_dropped = lock(&self.shared.inbox).mailbox.dropped();
        status
    }
}

impl Encoder for EncWorker {
    fn encode(
        &mut self,
        frame: CapturedFrame,
        opts: EncodeOpts,
    ) -> host_core::platform::Result<()> {
        if LinuxFrame::of(&frame).is_none() {
            return Err(PlatformError::Platform("not a Linux capture frame".into()));
        }
        let displaced = {
            let mut inbox = lock(&self.shared.inbox);
            if let Some(message) = &inbox.dead {
                return Err(PlatformError::Platform(message.clone()));
            }
            let displaced = inbox.mailbox.put(
                frame,
                JobFlags {
                    force_key: opts.force_keyframe,
                    refine: opts.quality_boost,
                },
            );
            lock(&self.shared.status).stats.submitted += 1;
            displaced
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

impl Drop for EncWorker {
    fn drop(&mut self) {
        lock(&self.shared.inbox).stop = true;
        self.shared.ready.notify_one();
        if self.done.recv_timeout(DROP_WAIT).is_ok() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        } else {
            warn!("Linux encoder worker did not stop within one second; detaching");
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    cfg: EncoderConfig,
    params: EncParams,
    factories: Vec<Arc<dyn BackendFactory>>,
    backend: Option<Box<dyn EncBackend>>,
    src: SrcSize,
    handle: Option<Handle>,
    origin: Instant,
    rate: RateApplier,
    key: KeyTracker,
    gate: OutputGate,
    pts: PtsMap,
    psc: ParamSetCache,
    fail: FailureState,
    last_source: Option<SourceKey>,
    last_frame: Option<CapturedFrame>,
    boost: bool,
    rate_idr: bool,
    rate_pending: bool,
    dead: Option<String>,
}

impl Worker {
    fn new(
        shared: Arc<Shared>,
        cfg: EncoderConfig,
        params: EncParams,
        factories: Vec<Arc<dyn BackendFactory>>,
        backend: Box<dyn EncBackend>,
        src: SrcSize,
        handle: Option<Handle>,
    ) -> Self {
        let mut worker = Self {
            shared,
            cfg,
            params,
            factories,
            backend: None,
            src,
            handle,
            origin: Instant::now(),
            rate: RateApplier::new(backend.rate_cost(), params.bitrate_bps),
            key: KeyTracker::new(),
            gate: OutputGate::new(),
            pts: PtsMap::new(),
            psc: ParamSetCache::new(params.codec),
            fail: FailureState::new(),
            last_source: None,
            last_frame: None,
            boost: true,
            rate_idr: false,
            rate_pending: false,
            dead: None,
        };
        worker.install(backend, false);
        worker
    }

    fn now(&self) -> u64 {
        self.origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }

    fn publish(&self) {
        let dropped = lock(&self.shared.inbox).mailbox.dropped();
        let mut status = lock(&self.shared.status);
        status.stats.mailbox_dropped = dropped;
        status.backend = self.backend.as_ref().map(|backend| backend.name());
    }

    fn die(&mut self, message: &str) {
        self.dead = Some(message.into());
        self.shared.mark_dead(message.into());
    }

    fn install(&mut self, backend: Box<dyn EncBackend>, reopen: bool) {
        self.gate.on_reopen();
        self.psc.clear();
        self.pts.clear();
        self.key.request(self.now());
        self.rate
            .reset(backend.rate_cost(), self.params.bitrate_bps);
        self.rate_idr = false;
        {
            let mut status = lock(&self.shared.status);
            status.backend = Some(backend.name());
            status.stats.reopens += u64::from(reopen);
            status.stats.rate_applied += u64::from(self.rate_pending);
        }
        self.rate_pending = false;
        self.backend = Some(backend);
    }

    fn runtime_failure(&mut self, message: String) {
        let now = self.now();
        lock(&self.shared.status).stats.failures += 1;
        self.fail.on_failure(now);
        if self.fail.should_report(now) {
            let name = self.factories.first().map(|factory| factory.name());
            warn!(
                backend = name,
                error = message,
                "Linux encoder failed; will retry"
            );
        }
        self.backend = None;
        self.key.request(now);
    }

    /// In fallback mode both kinds of open failure permanently skip a
    /// candidate. Otherwise Failed retains the current factory for backoff.
    fn open(&mut self, mut fallback: bool) -> bool {
        loop {
            let Some(factory) = self.factories.first().cloned() else {
                self.die("no usable encoder");
                return false;
            };
            match factory.open(&self.params, self.src) {
                Ok(backend) => {
                    debug!(
                        backend = backend.name(),
                        w = self.src.width,
                        h = self.src.height,
                        bps = self.params.bitrate_bps,
                        "Linux encoder opened"
                    );
                    self.install(backend, true);
                    return true;
                }
                Err(BackendError::Failed(message)) if !fallback => {
                    self.runtime_failure(message);
                    return false;
                }
                Err(error) => {
                    debug!(backend = factory.name(), %error, "Linux encoder candidate unavailable");
                    self.factories.remove(0);
                    fallback = true;
                }
            }
        }
    }

    fn backend_error(&mut self, error: BackendError) {
        match error {
            BackendError::Unsupported(message) => {
                let name = self.factories.first().map(|factory| factory.name());
                debug!(
                    backend = name,
                    error = message,
                    "Linux encoder backend unsupported"
                );
                self.backend = None;
                if !self.factories.is_empty() {
                    self.factories.remove(0);
                }
                self.open(true);
            }
            BackendError::Failed(message) => self.runtime_failure(message),
        }
    }

    fn apply_rate(&mut self, bps: u64) {
        // Preserve an applied target across any later source change/failure.
        self.params.bitrate_bps = bps;
        self.rate_pending = true;
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let cost = backend.rate_cost();
        if cost == RateCost::Reopen {
            self.backend = None;
            self.open(false);
            return;
        }
        match backend.set_rate(bps, self.params.fps) {
            Ok(()) => {
                self.rate_idr |= cost == RateCost::Idr;
                lock(&self.shared.status).stats.rate_applied += 1;
                self.rate_pending = false;
            }
            Err(error) => self.backend_error(error),
        }
    }

    /// Return an input taken just before a rate/open failure. If capture put a
    /// newer frame in the meantime, retain it and merge flags via Mailbox.
    fn return_job(&self, frame: CapturedFrame, flags: JobFlags) {
        let displaced = {
            let mut inbox = lock(&self.shared.inbox);
            let newer = inbox.mailbox.take();
            inbox.mailbox.put(frame, flags);
            newer.and_then(|(newer, flags)| inbox.mailbox.put(newer, flags))
        };
        drop(displaced);
    }

    fn process(&mut self, frame: CapturedFrame, flags: JobFlags, retry: bool) {
        if self.dead.is_some() {
            return;
        }
        if self.backend.is_none() && self.fail.retry_at().is_some_and(|at| self.now() < at) {
            self.return_job(frame, flags);
            return;
        }
        let Some(linux) = LinuxFrame::of(&frame) else {
            return;
        };
        let source = SourceKey {
            device_gen: linux.generation,
            width: linux.width,
            height: linux.height,
            rotation: 0,
        };
        let src = SrcSize {
            width: linux.width,
            height: linux.height,
            stride: linux.stride,
        };
        // There is no previous capture generation during probing. Adopt the
        // first generation if its source size matches the already open probe.
        let change = self
            .last_source
            .map_or(SourceChange::Same, |old| source_change(Some(old), source));
        self.last_source = Some(source);
        self.last_frame = Some(frame.clone());
        if change != SourceChange::Same || self.src != src {
            self.backend = None;
            self.src = src;
        }
        if self.backend.is_none() && !self.open(false) {
            // The frame must survive an open failure just like frames that
            // arrived while already in backoff.
            if self.dead.is_none() {
                self.return_job(frame, flags);
            }
            return;
        }
        let rate_idr = std::mem::take(&mut self.rate_idr);
        let idr = self.key.take_for_frame() || flags.force_key || rate_idr || retry;
        let action = refine_action(
            RefineMode::Repeat,
            flags.refine && !retry,
            idr,
            self.boost,
            self.rate.current(),
        );
        if action == RefineAction::Skip {
            return;
        }
        let pts = self.pts.next_pts(FrameMeta {
            capture_ts_us: frame.capture_ts_us,
            refine: action == RefineAction::Repeat,
            submit_us: unix_micros(),
        });
        let result = match self.backend.as_mut() {
            Some(backend) => backend.encode(linux, pts, idr),
            None => return,
        };
        match result {
            Ok(packets) => self.deliver(packets),
            Err(error) => self.backend_error(error),
        }
    }

    fn deliver(&mut self, packets: Vec<EncPacket>) {
        for packet in packets {
            let meta = self.pts.take(packet.pts).unwrap_or_else(|| {
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
                lock(&self.shared.status).stats.gated += 1;
                self.key.request(self.now());
                continue;
            }
            let still_refine = meta.refine && !keyframe;
            let frame = EncodedFrame {
                data,
                keyframe,
                still_refine,
                capture_ts_us: meta.capture_ts_us,
                encoded_ts_us: unix_micros(),
            };
            let sent = match self.cfg.sink.try_send(frame) {
                Ok(()) => true,
                Err(TrySendError::Closed(_)) => {
                    self.die("encoder output closed");
                    break;
                }
                Err(TrySendError::Full(frame)) => {
                    if let Some(handle) = &self.handle {
                        // Build the timeout inside the entered runtime. This
                        // is a std worker thread, never a Tokio task thread.
                        match handle.block_on(async {
                            tokio::time::timeout(SINK_WAIT, self.cfg.sink.send(frame)).await
                        }) {
                            Ok(Ok(())) => true,
                            Ok(Err(_)) => {
                                self.die("encoder output closed");
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
                let mut status = lock(&self.shared.status);
                status.stats.encoded += 1;
                status.stats.keyframes += u64::from(keyframe);
                status.stats.refines += u64::from(still_refine);
                self.fail.on_success();
            } else {
                lock(&self.shared.status).stats.sink_dropped += 1;
                self.gate.on_sink_drop();
                self.key.request(self.now());
            }
        }
    }

    fn frames_ready(&self, now: u64) -> bool {
        self.backend.is_some() || self.fail.retry_at().is_none_or(|at| now >= at)
    }

    fn deadline(&self) -> Option<u64> {
        let key = if self.backend.is_some() && self.last_frame.is_some() {
            self.key.deadline()
        } else {
            None
        };
        let retry = if self.backend.is_none() {
            self.fail.retry_at()
        } else {
            None
        };
        [self.rate.deadline(), key, retry]
            .into_iter()
            .flatten()
            .min()
    }

    fn run(mut self) {
        loop {
            let deadline = self.deadline();
            let (stop, job, force, rate, boost) = {
                let mut inbox = lock(&self.shared.inbox);
                loop {
                    let now = self.now();
                    if inbox.stop
                        || !inbox.mailbox.is_empty() && self.frames_ready(now)
                        || inbox.force_key
                        || inbox.rate.is_some()
                        || inbox.boost != self.boost
                        || deadline.is_some_and(|at| now >= at)
                    {
                        break;
                    }
                    if let Some(at) = deadline {
                        let (guard, _) = self
                            .shared
                            .ready
                            .wait_timeout(inbox, Duration::from_micros(at.saturating_sub(now)))
                            .unwrap_or_else(|error| error.into_inner());
                        inbox = guard;
                    } else {
                        inbox = self
                            .shared
                            .ready
                            .wait(inbox)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                }
                let job = if self.frames_ready(self.now()) {
                    inbox.mailbox.take()
                } else {
                    None
                };
                (
                    inbox.stop,
                    job,
                    std::mem::take(&mut inbox.force_key),
                    inbox.rate.take(),
                    inbox.boost,
                )
            };
            if stop {
                break;
            }
            let now = self.now();
            if let Some((kbps, fps)) = rate {
                let bps = effective_bps(kbps, fps, self.params.fps);
                if let Some(bps) = self.rate.request(now, bps) {
                    self.apply_rate(bps);
                }
            }
            if let Some(bps) = self.rate.poll(now) {
                self.apply_rate(bps);
            }
            if force {
                self.key.request(now);
            }
            self.boost = boost;
            if self.dead.is_some() {
                break;
            }
            if let Some((frame, flags)) = job {
                self.process(frame, flags, false);
            } else if self.backend.is_none() && self.fail.retry_at().is_some_and(|at| now >= at) {
                // Reopen even without a queued capture; retain the last frame
                // to satisfy the pending key request once the backend recovers.
                if self.open(false) {
                    if let Some(frame) = self.last_frame.clone() {
                        self.process(frame, JobFlags::default(), true);
                    }
                }
            } else if self.backend.is_some() && self.last_frame.is_some() && self.key.poll(now) {
                if let Some(frame) = self.last_frame.clone() {
                    lock(&self.shared.status).stats.idle_key_retries += 1;
                    self.process(frame, JobFlags::default(), true);
                }
            }
            self.publish();
            if self.dead.is_some() {
                break;
            }
        }
        self.publish();
    }
}
