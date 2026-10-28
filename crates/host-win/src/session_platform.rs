//! Session-process platform adapter.
//!
//! The session process has a deliberately small operating-system boundary.  In
//! particular, all input operations are serialized on one thread so that the
//! thread can follow the desktop that owns the input queue.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::platform::InputEvent;
use host_core::platform::{
    input_from_m0, AudioCaptureConfig, AudioChunkSink, CaptureConfig, CaptureHandle, ClipboardItem,
    ConsoleState, CursorUpdate, DisplayInfo, Encoder, EncoderConfig, FrameSink, HostInfo, KeyStore,
    Nic, PermissionKind, Permissions, Platform, PlatformError, Result, SessionActionKind,
};
use rsp_core::hid::InputMsg;
use tokio::sync::mpsc;

/// W3's encoder hook.  W3 can replace `None` with its encoder factory without
/// changing the session platform's threading and security rules.
pub type EncoderFactory = Arc<dyn Fn(EncoderConfig) -> Result<Box<dyn Encoder>> + Send + Sync>;

/// Operating-system operations needed by a session process.
pub trait SessionOs: Send + Sync + 'static {
    fn host_info(&self) -> HostInfo;
    fn displays(&self) -> Vec<DisplayInfo>;
    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>);
    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>>;
    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>);
    fn audio_capture_supported(&self) -> bool;
    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>>;
    /// Attach the current thread to the desktop receiving input.
    fn attach_input_desktop(&self) -> Result<String>;
    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo);
    fn release_all(&self);
    fn lock_workstation(&self) -> Result<()>;
    fn wake_display(&self) -> Result<()>;
    fn clipboard_get(&self) -> Option<ClipboardItem>;
    fn clipboard_set(&self, item: &ClipboardItem) -> Result<()>;
    fn watch_clipboard(&self, tx: mpsc::Sender<()>);
    fn watch_local_activity(&self, tx: mpsc::Sender<bool>);
    fn set_keep_awake(&self, on: bool);
    fn console_state(&self) -> ConsoleState;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionOptions {
    /// Input queue capacity.
    pub input_queue: usize,
    /// Deadline for release/action replies.
    pub reply_timeout: Duration,
    /// Minimum interval between attach attempts after a failure.
    pub attach_retry: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            input_queue: 1024,
            reply_timeout: Duration::from_secs(2),
            attach_retry: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionPlatformStats {
    pub dropped_inputs: u64,
    pub desktop_attaches: u64,
    pub desktop_attach_failures: u64,
    pub input_timeouts: u64,
}

#[derive(Default)]
struct AtomicStats {
    dropped_inputs: AtomicU64,
    desktop_attaches: AtomicU64,
    desktop_attach_failures: AtomicU64,
    input_timeouts: AtomicU64,
}

enum InputTask {
    Inject { msg: InputMsg, display: DisplayInfo },
    Release { reply: SyncSender<()> },
    Lock { reply: SyncSender<Result<()>> },
    Wake { reply: SyncSender<Result<()>> },
    Stop,
}

/// Session-side implementation of the host-core platform boundary.
pub struct SessionPlatform<O: SessionOs> {
    os: Arc<O>,
    encoder: Option<EncoderFactory>,
    input_tx: Mutex<Option<SyncSender<InputTask>>>,
    worker: Option<JoinHandle<()>>,
    generation: Arc<AtomicU64>,
    desktop: Mutex<Option<String>>,
    stats: Arc<AtomicStats>,
    opts: SessionOptions,
    null_keystore: NullKeyStore,
}

impl<O: SessionOs> SessionPlatform<O> {
    pub fn new(os: Arc<O>, encoder: Option<EncoderFactory>) -> Result<Self> {
        Self::with_options(os, encoder, SessionOptions::default())
    }

    pub fn with_options(
        os: Arc<O>,
        encoder: Option<EncoderFactory>,
        opts: SessionOptions,
    ) -> Result<Self> {
        let (input_tx, input_rx) = std_mpsc::sync_channel(opts.input_queue);
        let generation = Arc::new(AtomicU64::new(0));
        let stats = Arc::new(AtomicStats::default());
        let worker_generation = Arc::clone(&generation);
        let worker_stats = Arc::clone(&stats);
        let worker_os = Arc::clone(&os);
        let attach_retry = opts.attach_retry;
        let worker = thread::Builder::new()
            .name("rs-session-input".to_owned())
            .spawn(move || {
                input_loop(
                    worker_os,
                    input_rx,
                    worker_generation,
                    worker_stats,
                    attach_retry,
                )
            })
            .map_err(PlatformError::Io)?;

        Ok(Self {
            os,
            encoder,
            input_tx: Mutex::new(Some(input_tx)),
            worker: Some(worker),
            generation,
            desktop: Mutex::new(None),
            stats,
            opts,
            null_keystore: NullKeyStore,
        })
    }

    /// Set the current input desktop.  Repeating the same name does not cause
    /// an unnecessary re-attach.
    pub fn set_desktop(&self, name: Option<String>) {
        let mut desktop = self
            .desktop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *desktop != name {
            *desktop = name;
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn desktop(&self) -> Option<String> {
        self.desktop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn clipboard_allowed(&self) -> bool {
        self.desktop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_deref()
            .is_none_or(|name| name.eq_ignore_ascii_case("default"))
    }

    pub fn stats(&self) -> SessionPlatformStats {
        SessionPlatformStats {
            dropped_inputs: self.stats.dropped_inputs.load(Ordering::Relaxed),
            desktop_attaches: self.stats.desktop_attaches.load(Ordering::Relaxed),
            desktop_attach_failures: self.stats.desktop_attach_failures.load(Ordering::Relaxed),
            input_timeouts: self.stats.input_timeouts.load(Ordering::Relaxed),
        }
    }

    fn enqueue_input(&self, msg: InputMsg, display: &DisplayInfo) {
        let sender = self
            .input_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned();
        let Some(sender) = sender else {
            self.stats.dropped_inputs.fetch_add(1, Ordering::Relaxed);
            return;
        };
        match sender.try_send(InputTask::Inject {
            msg,
            display: display.clone(),
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.stats.dropped_inputs.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn control<R>(
        &self,
        make_task: impl FnOnce(SyncSender<R>) -> InputTask,
        interpret: impl FnOnce(R) -> Result<()>,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        let mut task = Some(make_task(reply_tx));
        let sender = self
            .input_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned();
        let Some(sender) = sender else {
            self.stats.input_timeouts.fetch_add(1, Ordering::Relaxed);
            return Err(PlatformError::Platform("input thread timed out".to_owned()));
        };

        let deadline = Instant::now() + self.opts.reply_timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                self.stats.input_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(PlatformError::Platform("input thread timed out".to_owned()));
            }
            let Some(current) = task.take() else {
                self.stats.input_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(PlatformError::Platform("input thread timed out".to_owned()));
            };
            match sender.try_send(current) {
                Ok(()) => break,
                Err(TrySendError::Full(task_back)) => {
                    task = Some(task_back);
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(Duration::from_millis(1)));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.stats.input_timeouts.fetch_add(1, Ordering::Relaxed);
                    return Err(PlatformError::Platform("input thread timed out".to_owned()));
                }
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        match reply_rx.recv_timeout(remaining) {
            Ok(reply) => interpret(reply),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                self.stats.input_timeouts.fetch_add(1, Ordering::Relaxed);
                Err(PlatformError::Platform("input thread timed out".to_owned()))
            }
        }
    }
}

impl<O: SessionOs> Platform for SessionPlatform<O> {
    fn info(&self) -> HostInfo {
        self.os.host_info()
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.os.displays()
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        self.os.watch_displays(tx)
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        blocking(|| self.os.capture(display, cfg, sink))
    }

    fn audio_capture_supported(&self) -> bool {
        self.os.audio_capture_supported()
    }

    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        blocking(|| self.os.capture_audio(cfg, sink))
    }

    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        let Some(factory) = self.encoder.clone() else {
            return Err(PlatformError::Unsupported);
        };
        blocking(|| factory(cfg))
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.enqueue_input(input_from_m0(ev), display)
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        if matches!(msg, InputMsg::ReleaseAll) {
            self.release_all();
        } else {
            self.enqueue_input(msg.clone(), display);
        }
    }

    fn release_all(&self) {
        let result = blocking(|| self.control(|reply| InputTask::Release { reply }, |_| Ok(())));
        let _ = result;
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        match kind {
            SessionActionKind::LockScreen => {
                blocking(|| self.control(|reply| InputTask::Lock { reply }, |result| result))
            }
            SessionActionKind::WakeDisplay => {
                blocking(|| self.control(|reply| InputTask::Wake { reply }, |result| result))
            }
            SessionActionKind::LockOnEnd => Ok(()),
            SessionActionKind::CtrlAltDel | SessionActionKind::Unspecified => {
                Err(PlatformError::Unsupported)
            }
        }
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        self.os.watch_cursor(tx)
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        self.os.watch_local_activity(tx)
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        if !self.clipboard_allowed() {
            return None;
        }
        blocking(|| self.os.clipboard_get())
    }

    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        if !self.clipboard_allowed() {
            return Err(PlatformError::Platform(
                "clipboard is unavailable on the secure desktop".to_owned(),
            ));
        }
        blocking(|| self.os.clipboard_set(&item))
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        self.os.watch_clipboard(tx)
    }

    fn power_keep_awake(&self, on: bool) {
        self.os.set_keep_awake(on)
    }

    fn console_state(&self) -> ConsoleState {
        self.os.console_state()
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            screen_recording: true,
            accessibility: true,
        }
    }

    fn request_permission(&self, _permission: PermissionKind) {}

    fn nics(&self) -> Vec<Nic> {
        Vec::new()
    }

    fn send_wol(&self, _mac: [u8; 6], _via: &Nic) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn keystore(&self) -> &dyn KeyStore {
        &self.null_keystore
    }
}

impl<O: SessionOs> Drop for SessionPlatform<O> {
    fn drop(&mut self) {
        let sender = self
            .input_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(sender) = sender {
            let deadline = Instant::now() + self.opts.reply_timeout;
            let mut task = Some(InputTask::Stop);
            while Instant::now() < deadline {
                let Some(current) = task.take() else {
                    break;
                };
                match sender.try_send(current) {
                    Ok(()) => {
                        task = None;
                        break;
                    }
                    Err(TrySendError::Full(task_back)) => {
                        task = Some(task_back);
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        thread::sleep(remaining.min(Duration::from_millis(1)));
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        task = None;
                        break;
                    }
                }
            }
            drop(task);
            drop(sender);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn input_loop<O: SessionOs>(
    os: Arc<O>,
    rx: Receiver<InputTask>,
    generation: Arc<AtomicU64>,
    stats: Arc<AtomicStats>,
    attach_retry: Duration,
) {
    let mut attached_generation = None;
    let mut last_attach_failure = None;

    while let Ok(task) = rx.recv() {
        if matches!(task, InputTask::Stop) {
            os.release_all();
            return;
        }

        let current_generation = generation.load(Ordering::Acquire);
        if attached_generation != Some(current_generation) {
            let retry_allowed = last_attach_failure
                .is_none_or(|failure: Instant| failure.elapsed() >= attach_retry);
            if retry_allowed {
                match os.attach_input_desktop() {
                    Ok(_) => {
                        attached_generation = Some(current_generation);
                        last_attach_failure = None;
                        stats.desktop_attaches.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        last_attach_failure = Some(Instant::now());
                        stats
                            .desktop_attach_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }

        match task {
            InputTask::Inject { msg, display } => os.inject_input(&msg, &display),
            InputTask::Release { reply } => {
                os.release_all();
                let _ = reply.send(());
            }
            InputTask::Lock { reply } => {
                let _ = reply.send(os.lock_workstation());
            }
            InputTask::Wake { reply } => {
                let _ = reply.send(os.wake_display());
            }
            InputTask::Stop => {
                os.release_all();
                return;
            }
        }
    }

    os.release_all();
}

/// The session process never owns the host's key material.
pub struct NullKeyStore;

impl KeyStore for NullKeyStore {
    fn load(&self, _name: &str) -> Result<Option<Vec<u8>>> {
        Err(PlatformError::Unsupported)
    }

    fn store(&self, _name: &str, _value: &[u8]) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn delete(&self, _name: &str) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

/// Run a possibly blocking platform operation without starving other tasks on
/// a multi-thread Tokio runtime.
pub fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}
