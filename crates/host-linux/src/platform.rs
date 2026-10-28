//! Linux implementation of the host platform boundary.
//!
//! The native backends in this crate deliberately have small, synchronous
//! public APIs.  This module owns the glue between those APIs and the
//! asynchronous services used by a desktop session (the portal and logind).

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use host_core::activity_model::{ActivityState, KeepAwakeLatch};
use host_core::platform::*;
use rsp_core::clock::unix_micros;
use rsp_core::pb;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::capture_model::CursorDeviation;
use crate::enc_worker::EncWorker;
use crate::ff_encoder::FfFactory;
use crate::ff_model::{self, GpuNode, LinuxBackend, LinuxEncoderPref};
use crate::ffmpeg::FfLib;
use crate::keystore::FileKeyStore;
use crate::logind::{keep_awake, KeepAwakeGuard, Logind};
use crate::portal::{PortalError, PortalOptions, PortalSession};
use crate::portal_input::{PortalInjector, PortalTarget};
use crate::portal_model::{self, PortalKind, StreamDisplay};
use crate::pw::PwLib;
use crate::sw_encoder::OpenH264Factory;
use crate::sys_model::{self, DesktopSession};
use crate::wl_capture::{RawPointer, WaylandCapture, WlStreamSource};
use crate::x11_capture::X11Capture;
use crate::x11_clip::X11Clipboard;
use crate::x11_input::{X11Activity, X11Injector};

/// Options used to construct a [`LinuxPlatform`].
#[derive(Debug, Clone)]
pub struct LinuxOptions {
    pub state_dir: PathBuf,
    pub view_only: bool,
    pub session: Option<DesktopSession>,
    pub x11_display: Option<String>,
    pub sys_root: PathBuf,
    pub encoder_pref: Option<LinuxEncoderPref>,
    pub openh264_path: Option<String>,
    pub portal_timeout: Duration,
}

impl LinuxOptions {
    /// Read the environment values used by the Linux platform.
    pub fn from_env(state_dir: PathBuf, view_only: bool) -> Self {
        let encoder_pref = match std::env::var("RS_VIDEO_ENCODER") {
            Ok(value) => match ff_model::parse_pref(&value) {
                Some(pref) => Some(pref),
                None => {
                    warn!(value = %value, "unknown RS_VIDEO_ENCODER; using auto");
                    Some(LinuxEncoderPref::Auto)
                }
            },
            Err(_) => None,
        };
        Self {
            state_dir,
            view_only,
            session: None,
            x11_display: std::env::var("DISPLAY").ok(),
            sys_root: PathBuf::from("/"),
            encoder_pref,
            openh264_path: std::env::var("RS_OPENH264_PATH").ok(),
            portal_timeout: Duration::from_secs(120),
        }
    }
}

/// Result of applying a session action after capability discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionPlan {
    Lock,
    Wake,
    Nothing,
    Unsupported,
}

/// Decide whether a session action can be performed by the available glue.
pub fn session_action_plan(
    kind: SessionActionKind,
    has_logind: bool,
    has_injector: bool,
) -> ActionPlan {
    match kind {
        SessionActionKind::LockScreen => {
            if has_logind {
                ActionPlan::Lock
            } else {
                ActionPlan::Unsupported
            }
        }
        SessionActionKind::WakeDisplay => {
            if has_injector {
                ActionPlan::Wake
            } else {
                ActionPlan::Unsupported
            }
        }
        SessionActionKind::LockOnEnd => ActionPlan::Nothing,
        SessionActionKind::CtrlAltDel | SessionActionKind::Unspecified => ActionPlan::Unsupported,
    }
}

/// Select the retry delay for a failed portal attempt.
pub fn portal_retry(error: &PortalError, attempt: u32) -> PortalRetry {
    match error {
        PortalError::Cancelled => PortalRetry::WaitForRequest,
        PortalError::Timeout => PortalRetry::After(Duration::from_secs(300)),
        PortalError::Unavailable => PortalRetry::After(Duration::from_secs(10)),
        PortalError::Closed | PortalError::Failed(_) | PortalError::DBus(_) => {
            let shift = attempt.min(6);
            let seconds = 1_u64.checked_shl(shift).unwrap_or(60).min(60);
            PortalRetry::After(Duration::from_secs(seconds))
        }
    }
}

/// Portal retry policy exposed for the first-layer tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalRetry {
    After(Duration),
    WaitForRequest,
}

/// Convert one portal stream into the input target used by RemoteDesktop.
pub fn portal_target(displays: &[StreamDisplay], display_id: u32) -> Option<PortalTarget> {
    let display = displays.iter().find(|display| display.id == display_id)?;
    if display.width == 0 || display.height == 0 {
        return None;
    }
    Some(PortalTarget {
        display_id: display.id,
        node_id: display.node_id,
        geom: crate::input_plan::DisplayGeom {
            width: f64::from(display.width),
            height: f64::from(display.height),
        },
    })
}

/// Convert a normalized portal pointer report into logical display pixels.
pub fn raw_to_logical(pointer: &RawPointer, displays: &[StreamDisplay]) -> Option<(u32, f64, f64)> {
    let display = displays
        .iter()
        .find(|display| display.id == pointer.display_id)?;
    if display.width == 0 || display.height == 0 {
        return None;
    }
    Some((
        display.id,
        pointer.nx * f64::from(display.width),
        pointer.ny * f64::from(display.height),
    ))
}

/// Pick the first existing OpenH264 blob, retaining a deterministic fallback.
pub fn pick_openh264(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| exists(candidate))
        .cloned()
        .or_else(|| candidates.first().cloned())
}

struct WlState {
    session: PortalSession,
    capture: WaylandCapture,
    injector: Option<PortalInjector>,
    displays: Vec<StreamDisplay>,
}

enum PortalControl {
    Reauthorize,
    Shutdown,
}

struct Inner {
    opts: LinuxOptions,
    handle: Handle,
    session: DesktopSession,
    keystore: FileKeyStore,
    session_bus: Option<zbus::Connection>,
    system_bus: Option<zbus::Connection>,
    logind: Option<Arc<Logind>>,
    console: Mutex<ConsoleState>,
    console_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pw: Option<Arc<PwLib>>,
    x11_capture: Option<Arc<X11Capture>>,
    x11_injector: Option<Arc<X11Injector>>,
    x11_clipboard: Option<Arc<X11Clipboard>>,
    x11_activity: Mutex<Vec<X11Activity>>,
    wl_state: Mutex<Option<WlState>>,
    wl_display_watchers: Mutex<Vec<mpsc::Sender<Vec<DisplayInfo>>>>,
    wl_cursor_watchers: Mutex<Vec<mpsc::Sender<CursorUpdate>>>,
    wl_pointer_watchers: Mutex<Vec<mpsc::Sender<RawPointer>>>,
    portal_control: Option<mpsc::Sender<PortalControl>>,
    portal_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    activity_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    info: Mutex<Option<HostInfo>>,
    power_latch: Mutex<KeepAwakeLatch>,
    power_guard: Mutex<Option<KeepAwakeGuard>>,
    power_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(task) = self.portal_task.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
        if let Ok(task) = self.console_task.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
        if let Ok(tasks) = self.activity_tasks.get_mut() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        if let Ok(tasks) = self.power_tasks.get_mut() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
    }
}

/// Linux desktop platform.  Native backends are shared by clones and are
/// stopped when the final platform value is dropped.
#[derive(Clone)]
pub struct LinuxPlatform {
    inner: Arc<Inner>,
}

impl LinuxPlatform {
    /// Construct a platform.  Optional subsystem failures are deliberately
    /// downgraded to warnings so headless hosts retain system capabilities.
    pub async fn new(opts: LinuxOptions) -> Result<Self> {
        fs::create_dir_all(&opts.state_dir)?;
        let handle = Handle::current();
        let keystore = FileKeyStore::in_state_dir(&opts.state_dir);

        let session_bus =
            match tokio::time::timeout(Duration::from_secs(2), zbus::Connection::session()).await {
                Ok(Ok(connection)) => Some(connection),
                Ok(Err(error)) => {
                    warn!(%error, "session D-Bus unavailable");
                    None
                }
                Err(_) => {
                    warn!("session D-Bus connection timed out");
                    None
                }
            };
        let system_bus =
            match tokio::time::timeout(Duration::from_secs(2), zbus::Connection::system()).await {
                Ok(Ok(connection)) => Some(connection),
                Ok(Err(error)) => {
                    warn!(%error, "system D-Bus unavailable");
                    None
                }
                Err(_) => {
                    warn!("system D-Bus connection timed out");
                    None
                }
            };

        let logind = if let Some(system) = system_bus.as_ref() {
            let connection = system.clone();
            let uid = unsafe {
                // SAFETY: getuid has no preconditions and only reads process credentials.
                libc::getuid()
            };
            match tokio::time::timeout(
                Duration::from_secs(2),
                Logind::connect(
                    connection,
                    uid,
                    std::process::id(),
                    std::env::var("XDG_SESSION_ID").ok().as_deref(),
                ),
            )
            .await
            {
                Ok(Ok(value)) => Some(Arc::new(value)),
                Ok(Err(error)) => {
                    warn!(%error, "logind unavailable");
                    None
                }
                Err(_) => {
                    warn!("logind connection timed out");
                    None
                }
            }
        } else {
            None
        };
        let initial_console = if let Some(logind) = logind.as_ref() {
            match tokio::time::timeout(Duration::from_secs(2), logind.console_state()).await {
                Ok(state) => state,
                Err(_) => ConsoleState::Unknown,
            }
        } else {
            ConsoleState::Unknown
        };

        let (session, source) = if let Some(explicit) = opts.session {
            (explicit, "options")
        } else {
            let detected = sys_model::detect_session(
                std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
                std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
                std::env::var("DISPLAY").ok().as_deref(),
            );
            if detected == DesktopSession::None {
                if let Some(logind) = logind.as_ref() {
                    match tokio::time::timeout(Duration::from_secs(2), logind.session_type()).await
                    {
                        Ok(Ok(kind)) if kind.eq_ignore_ascii_case("wayland") => {
                            (DesktopSession::Wayland, "logind")
                        }
                        Ok(Ok(kind)) if kind.eq_ignore_ascii_case("x11") => {
                            (DesktopSession::X11, "logind")
                        }
                        _ => (detected, "environment"),
                    }
                } else {
                    (detected, "environment")
                }
            } else {
                (detected, "environment")
            }
        };
        info!(
            session = ?session,
            source,
            desktop = %std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            "Linux desktop session selected"
        );

        let pw = match PwLib::load() {
            Ok(value) => Some(value),
            Err(error) => {
                debug!(%error, "PipeWire unavailable");
                None
            }
        };

        let x11_display_value = opts
            .x11_display
            .clone()
            .or_else(|| std::env::var("DISPLAY").ok());
        let x11_display = x11_display_value.as_deref();
        let (x11_capture, x11_injector, x11_clipboard) = if session == DesktopSession::X11 {
            let capture = match X11Capture::connect(x11_display) {
                Ok(value) => Some(Arc::new(value)),
                Err(error) => {
                    warn!(%error, "X11 capture unavailable");
                    None
                }
            };
            let injector = if !opts.view_only {
                match X11Injector::connect(x11_display) {
                    Ok(value) => Some(Arc::new(value)),
                    Err(error) => {
                        warn!(%error, "X11 input unavailable");
                        None
                    }
                }
            } else {
                None
            };
            let clipboard = match X11Clipboard::connect(x11_display) {
                Ok(value) => Some(Arc::new(value)),
                Err(error) => {
                    warn!(%error, "X11 clipboard unavailable");
                    None
                }
            };
            (capture, injector, clipboard)
        } else if session == DesktopSession::Wayland {
            let clipboard = match x11_display {
                Some(display) => match X11Clipboard::connect(Some(display)) {
                    Ok(value) => Some(Arc::new(value)),
                    Err(error) => {
                        debug!(%error, "XWayland clipboard unavailable");
                        None
                    }
                },
                None => match X11Clipboard::connect(None) {
                    Ok(value) => Some(Arc::new(value)),
                    Err(error) => {
                        debug!(%error, "XWayland clipboard unavailable");
                        None
                    }
                },
            };
            (None, None, clipboard)
        } else {
            (None, None, None)
        };

        let (portal_control, portal_rx) = mpsc::channel(8);
        let portal_control =
            if session == DesktopSession::Wayland && session_bus.is_some() && pw.is_some() {
                Some(portal_control)
            } else {
                None
            };
        let inner = Arc::new(Inner {
            opts,
            handle: handle.clone(),
            session,
            keystore,
            session_bus,
            system_bus,
            logind,
            console: Mutex::new(initial_console),
            console_task: Mutex::new(None),
            pw,
            x11_capture,
            x11_injector,
            x11_clipboard,
            x11_activity: Mutex::new(Vec::new()),
            wl_state: Mutex::new(None),
            wl_display_watchers: Mutex::new(Vec::new()),
            wl_cursor_watchers: Mutex::new(Vec::new()),
            wl_pointer_watchers: Mutex::new(Vec::new()),
            portal_control,
            portal_task: Mutex::new(None),
            activity_tasks: Mutex::new(Vec::new()),
            info: Mutex::new(None),
            power_latch: Mutex::new(KeepAwakeLatch::default()),
            power_guard: Mutex::new(None),
            power_tasks: Mutex::new(Vec::new()),
        });

        if let Some(logind) = inner.logind.clone() {
            let weak = Arc::downgrade(&inner);
            let task = handle.spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let Some(inner) = weak.upgrade() else { break };
                    let state = logind.console_state().await;
                    *lock(&inner.console) = state;
                }
            });
            *lock(&inner.console_task) = Some(task);
        }

        if let (Some(control), Some(session_bus), Some(pw)) = (
            inner.portal_control.clone(),
            inner.session_bus.clone(),
            inner.pw.clone(),
        ) {
            let weak = Arc::downgrade(&inner);
            let opts = inner.opts.clone();
            let task = handle.spawn(portal_loop(weak, session_bus, pw, opts, control, portal_rx));
            *lock(&inner.portal_task) = Some(task);
        } else if session == DesktopSession::Wayland {
            warn!("Wayland needs session D-Bus and PipeWire");
        }

        Ok(Self { inner })
    }

    pub fn session(&self) -> DesktopSession {
        self.inner.session
    }

    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.displays().is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn encoder_chain(&self, codec: pb::Codec) -> Vec<&'static str> {
        let nodes = sys_model::render_nodes(&self.inner.opts.sys_root)
            .into_iter()
            .map(|node| GpuNode {
                dev_path: node.dev_path,
                vendor: node.vendor,
                driver: node.driver,
            })
            .collect::<Vec<_>>();
        let ffmpeg_ok = FfLib::shared().is_ok();
        ff_model::candidates(
            &nodes,
            self.inner.opts.encoder_pref.unwrap_or_default(),
            codec,
            ffmpeg_ok,
        )
        .into_iter()
        .map(|candidate| candidate.backend.name())
        .collect()
    }

    pub async fn shutdown(&self) {
        if let Some(control) = self.inner.portal_control.as_ref() {
            let _ = control.send(PortalControl::Shutdown).await;
        }
        let portal_task = { lock(&self.inner.portal_task).take() };
        if let Some(task) = portal_task {
            let _ = task.await;
        }
        clear_wayland_state(&self.inner, true).await;
        let console_task = { lock(&self.inner.console_task).take() };
        if let Some(task) = console_task {
            task.abort();
            let _ = task.await;
        }
        let activity_tasks = {
            lock(&self.inner.activity_tasks)
                .drain(..)
                .collect::<Vec<_>>()
        };
        for task in activity_tasks {
            task.abort();
            let _ = task.await;
        }
        lock(&self.inner.x11_activity).clear();
        let power_tasks = { lock(&self.inner.power_tasks).drain(..).collect::<Vec<_>>() };
        for task in power_tasks {
            task.abort();
            let _ = task.await;
        }
        lock(&self.inner.power_guard).take();
    }
}

impl Platform for LinuxPlatform {
    fn info(&self) -> HostInfo {
        if let Some(value) = lock(&self.inner.info).clone() {
            return value;
        }
        let hostname = hostname();
        let os = sys_model::read_os_release(&self.inner.opts.sys_root);
        let kernel = fs::read_to_string(self.inner.opts.sys_root.join("proc/sys/kernel/osrelease"))
            .unwrap_or_default();
        let vendor =
            fs::read_to_string(self.inner.opts.sys_root.join("sys/class/dmi/id/sys_vendor")).ok();
        let product = fs::read_to_string(
            self.inner
                .opts
                .sys_root
                .join("sys/class/dmi/id/product_name"),
        )
        .ok();
        let value = sys_model::host_info(
            &hostname,
            &os,
            &kernel,
            vendor.as_deref(),
            product.as_deref(),
            std::env::consts::ARCH,
        );
        *lock(&self.inner.info) = Some(value.clone());
        value
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        match self.inner.session {
            DesktopSession::X11 => self
                .inner
                .x11_capture
                .as_ref()
                .map_or_else(Vec::new, |capture| capture.displays()),
            DesktopSession::Wayland => lock(&self.inner.wl_state)
                .as_ref()
                .map_or_else(Vec::new, |state| state.capture.displays()),
            DesktopSession::None => Vec::new(),
        }
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        match self.inner.session {
            DesktopSession::X11 => {
                if let Some(capture) = self.inner.x11_capture.as_ref() {
                    capture.watch_displays(tx);
                }
            }
            DesktopSession::Wayland => {
                if let Some(state) = lock(&self.inner.wl_state).as_ref() {
                    state.capture.watch_displays(tx);
                } else {
                    lock(&self.inner.wl_display_watchers).push(tx);
                }
            }
            DesktopSession::None => lock(&self.inner.wl_display_watchers).push(tx),
        }
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        match self.inner.session {
            DesktopSession::X11 => self
                .inner
                .x11_capture
                .as_ref()
                .ok_or(PlatformError::Unsupported)?
                .capture(display, cfg, sink),
            DesktopSession::Wayland => lock(&self.inner.wl_state)
                .as_ref()
                .ok_or_else(|| {
                    PlatformError::Platform("waiting for screen sharing permission".into())
                })?
                .capture
                .capture(display, cfg, sink),
            DesktopSession::None => Err(PlatformError::Unsupported),
        }
    }

    fn audio_capture_supported(&self) -> bool {
        self.inner.pw.is_some()
    }

    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let Some(pw) = self.inner.pw.clone() else {
            return Err(PlatformError::Unsupported);
        };
        Ok(Box::new(crate::audio::start_monitor_capture(
            pw, cfg, sink,
        )?))
    }

    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        let codec = cfg.codec;
        let nodes = sys_model::render_nodes(&self.inner.opts.sys_root)
            .into_iter()
            .map(|node| GpuNode {
                dev_path: node.dev_path,
                vendor: node.vendor,
                driver: node.driver,
            })
            .collect::<Vec<_>>();
        let ff = FfLib::shared().ok();
        let candidates = ff_model::candidates(
            &nodes,
            self.inner.opts.encoder_pref.unwrap_or_default(),
            cfg.codec,
            ff.is_some(),
        );
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let blob_candidates = OpenH264Factory::blob_candidates(
            &exe_dir,
            &self.inner.opts.state_dir,
            self.inner.opts.openh264_path.as_deref(),
        );
        let blob = pick_openh264(&blob_candidates, Path::exists);
        let mut factories: Vec<Arc<dyn crate::enc_worker::BackendFactory>> = Vec::new();
        for candidate in &candidates {
            match candidate.backend {
                LinuxBackend::Vaapi | LinuxBackend::Nvenc => {
                    if let Some(ff) = ff.as_ref() {
                        factories.push(Arc::new(FfFactory::new(ff.clone(), candidate)));
                    }
                }
                LinuxBackend::OpenH264 => {
                    if let Some(path) = blob.clone() {
                        factories.push(Arc::new(OpenH264Factory::from_blob(path)));
                    }
                }
            }
        }
        let worker = EncWorker::start(cfg, factories, Handle::try_current().ok())?;
        let backend = worker.status().backend;
        info!(codec = ?codec, backend = ?backend, "Linux encoder ready");
        Ok(Box::new(worker))
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.inject_input(&input_from_m0(ev), display);
    }

    fn inject_input(&self, msg: &InputMsg, display_info: &DisplayInfo) {
        match self.inner.session {
            DesktopSession::Wayland => {
                let state = lock(&self.inner.wl_state);
                let Some(state) = state.as_ref() else {
                    return;
                };
                let Some(injector) = state.injector.as_ref() else {
                    return;
                };
                if matches!(msg, InputMsg::ReleaseAll) {
                    injector.release_all();
                    return;
                }
                let Some(target) = portal_target(&state.displays, display_info.id) else {
                    let display_id = display_info.id;
                    debug!(display_id, "dropping input for unknown portal display");
                    return;
                };
                injector.inject(msg, target);
            }
            DesktopSession::X11 => {
                if let Some(injector) = self.inner.x11_injector.as_ref() {
                    injector.inject(msg, display_info);
                }
            }
            DesktopSession::None => {}
        }
    }

    fn release_all(&self) {
        match self.inner.session {
            DesktopSession::Wayland => {
                if let Some(injector) = lock(&self.inner.wl_state)
                    .as_ref()
                    .and_then(|state| state.injector.as_ref())
                {
                    injector.release_all();
                }
            }
            DesktopSession::X11 => {
                if let Some(injector) = self.inner.x11_injector.as_ref() {
                    injector.release_all();
                }
            }
            DesktopSession::None => {}
        }
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        let has_injector = self.inner.x11_injector.is_some()
            || lock(&self.inner.wl_state)
                .as_ref()
                .and_then(|state| state.injector.as_ref())
                .is_some();
        match session_action_plan(kind, self.inner.logind.is_some(), has_injector) {
            ActionPlan::Lock => {
                let Some(logind) = self.inner.logind.clone() else {
                    return Err(PlatformError::Unsupported);
                };
                self.inner.handle.spawn(async move {
                    if let Err(error) = logind.lock().await {
                        warn!(%error, "failed to lock Linux session");
                    }
                });
                Ok(())
            }
            ActionPlan::Wake => {
                if let Some(injector) = lock(&self.inner.wl_state)
                    .as_ref()
                    .and_then(|state| state.injector.as_ref())
                {
                    injector.wake();
                } else if let Some(injector) = self.inner.x11_injector.as_ref() {
                    injector.wake();
                } else {
                    return Err(PlatformError::Unsupported);
                }
                Ok(())
            }
            ActionPlan::Nothing => Ok(()),
            ActionPlan::Unsupported => Err(PlatformError::Unsupported),
        }
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        match self.inner.session {
            DesktopSession::Wayland => {
                if let Some(state) = lock(&self.inner.wl_state).as_ref() {
                    state.capture.watch_cursor(tx);
                } else {
                    lock(&self.inner.wl_cursor_watchers).push(tx);
                }
            }
            DesktopSession::X11 => {
                if let Some(capture) = self.inner.x11_capture.as_ref() {
                    capture.watch_cursor(tx);
                }
            }
            DesktopSession::None => {}
        }
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        match self.inner.session {
            DesktopSession::X11 => {
                match X11Activity::start(self.inner.opts.x11_display.as_deref(), tx) {
                    Ok(activity) => lock(&self.inner.x11_activity).push(activity),
                    Err(error) => warn!(%error, "X11 local activity unavailable"),
                }
            }
            DesktopSession::Wayland => {
                let (pointer_tx, mut pointer_rx) = mpsc::channel(64);
                lock(&self.inner.wl_pointer_watchers).push(pointer_tx.clone());
                if let Some(state) = lock(&self.inner.wl_state).as_ref() {
                    state.capture.watch_pointer(pointer_tx);
                }
                let weak = Arc::downgrade(&self.inner);
                let task = self.inner.handle.spawn(async move {
                    let mut deviation = CursorDeviation::new();
                    let mut activity = ActivityState::default();
                    let started = Instant::now();
                    let mut ticker = tokio::time::interval(Duration::from_millis(500));
                    loop {
                        tokio::select! {
                            pointer = pointer_rx.recv() => {
                                let Some(pointer) = pointer else { break };
                                let Some(inner) = weak.upgrade() else { break };
                                if let Some(injector) = lock(&inner.wl_state).as_ref().and_then(|state| state.injector.as_ref()) {
                                    if let Some(last) = injector.last_pointer() {
                                        let at = unix_micros().saturating_sub(last.at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
                                        deviation.on_injected(last.display_id, last.x, last.y, at);
                                    }
                                }
                                let displays = lock(&inner.wl_state)
                                    .as_ref()
                                    .map(|state| state.displays.clone())
                                    .unwrap_or_default();
                                if let Some((display, x, y)) = raw_to_logical(&pointer, &displays) {
                                    let now_us = unix_micros();
                                    if deviation.on_cursor(display, x, y, now_us) {
                                        if let Some(value) = activity.on_event(started.elapsed()) {
                                            let _ = tx.try_send(value);
                                        }
                                    }
                                }
                            }
                            _ = ticker.tick() => {
                                if let Some(value) = activity.on_tick(started.elapsed()) {
                                    let _ = tx.try_send(value);
                                }
                            }
                        }
                        if tx.is_closed() { break; }
                    }
                });
                lock(&self.inner.activity_tasks).push(task);
            }
            DesktopSession::None => {}
        }
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.inner
            .x11_clipboard
            .as_ref()
            .and_then(|clipboard| clipboard.get())
    }

    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        self.inner
            .x11_clipboard
            .as_ref()
            .ok_or(PlatformError::Unsupported)?
            .set(item)
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        if let Some(clipboard) = self.inner.x11_clipboard.as_ref() {
            clipboard.watch(tx);
        }
    }

    fn power_keep_awake(&self, on: bool) {
        let Some(changed) = lock(&self.inner.power_latch).set(on) else {
            return;
        };
        if !changed {
            lock(&self.inner.power_guard).take();
            return;
        }
        let Some(system) = self.inner.system_bus.clone() else {
            debug!("keep-awake requested without system D-Bus");
            return;
        };
        let session = self.inner.session_bus.clone();
        let weak = Arc::downgrade(&self.inner);
        let task = self.inner.handle.spawn(async move {
            match keep_awake(session.as_ref(), &system, "Remote Screen session").await {
                Ok(guard) => {
                    let Some(inner) = weak.upgrade() else { return };
                    let mut latch = lock(&inner.power_latch);
                    if latch.set(true).is_none() {
                        info!(kind = guard.kind(), "Linux keep-awake enabled");
                        *lock(&inner.power_guard) = Some(guard);
                    } else {
                        // The desired state changed to off while Inhibit was
                        // being negotiated. Restore the latch and release
                        // the just-created guard immediately.
                        let _ = latch.set(false);
                    }
                }
                Err(error) => warn!(%error, "failed to keep Linux session awake"),
            }
        });
        lock(&self.inner.power_tasks).push(task);
    }

    fn console_state(&self) -> ConsoleState {
        *lock(&self.inner.console)
    }

    fn permissions(&self) -> Permissions {
        match self.inner.session {
            DesktopSession::Wayland => {
                let state = lock(&self.inner.wl_state);
                Permissions {
                    screen_recording: state.is_some(),
                    accessibility: state
                        .as_ref()
                        .and_then(|value| value.injector.as_ref())
                        .is_some(),
                }
            }
            DesktopSession::X11 => Permissions {
                screen_recording: self.inner.x11_capture.is_some(),
                accessibility: self.inner.x11_injector.is_some(),
            },
            DesktopSession::None => Permissions::default(),
        }
    }

    fn request_permission(&self, permission: PermissionKind) {
        if self.inner.session != DesktopSession::Wayland {
            return;
        }
        if let Some(control) = self.inner.portal_control.as_ref() {
            let permissions = self.permissions();
            let granted = match permission {
                PermissionKind::ScreenRecording => permissions.screen_recording,
                PermissionKind::Accessibility => permissions.accessibility,
            };
            if granted {
                return;
            }
            let _ = control.try_send(PortalControl::Reauthorize);
        }
    }

    fn nics(&self) -> Vec<Nic> {
        let Ok(interfaces) = if_addrs::get_if_addrs() else {
            return Vec::new();
        };
        let records = interfaces
            .into_iter()
            .filter_map(|interface| {
                let loopback = interface.is_loopback();
                let name = interface.name;
                let (ip, netmask) = match interface.addr {
                    if_addrs::IfAddr::V4(value) => {
                        (IpAddr::V4(value.ip), IpAddr::V4(value.netmask))
                    }
                    if_addrs::IfAddr::V6(value) => {
                        (IpAddr::V6(value.ip), IpAddr::V6(value.netmask))
                    }
                };
                let prefix = sys_model::prefix_from_netmask(netmask)?;
                Some(sys_model::IfAddrRecord {
                    name,
                    ip,
                    prefix,
                    loopback,
                })
            })
            .collect::<Vec<_>>();
        sys_model::nics(&records, &self.inner.opts.sys_root)
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        let Some((source, broadcast)) = host_core::wol::route_of(via) else {
            return Err(PlatformError::Platform(
                "no IPv4 network on interface".into(),
            ));
        };
        host_core::wol::send_magic_udp(mac, source, broadcast, host_core::wol::WOL_PORT)
            .map_err(PlatformError::Io)
    }

    fn keystore(&self) -> &dyn KeyStore {
        &self.inner.keystore
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn hostname() -> String {
    let mut bytes = [0_u8; 256];
    let result = unsafe {
        // SAFETY: the buffer is valid for the stated length and gethostname writes at most it.
        libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len())
    };
    if result != 0 {
        return String::new();
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

async fn portal_loop(
    weak: Weak<Inner>,
    conn: zbus::Connection,
    pw: Arc<PwLib>,
    opts: LinuxOptions,
    _control_sender: mpsc::Sender<PortalControl>,
    mut control_rx: mpsc::Receiver<PortalControl>,
) {
    let mut attempt = 0_u32;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let mut portal_options = PortalOptions::new(opts.state_dir.clone(), !opts.view_only);
        portal_options.response_timeout = opts.portal_timeout;
        let open = PortalSession::open(&conn, &portal_options);
        tokio::pin!(open);
        let session = tokio::select! {
            result = &mut open => result,
            control = control_rx.recv() => {
                match control {
                    Some(PortalControl::Shutdown) | None => return,
                    Some(PortalControl::Reauthorize) => continue,
                }
            }
        };
        match session {
            Err(error) => {
                warn!(error = %portal_error_kind(&error), "portal authorization failed");
                let retry = portal_retry(&error, attempt);
                attempt = attempt.saturating_add(1);
                match retry {
                    PortalRetry::WaitForRequest => match control_rx.recv().await {
                        Some(PortalControl::Reauthorize) => {}
                        Some(PortalControl::Shutdown) | None => return,
                    },
                    PortalRetry::After(delay) => {
                        if wait_portal_control(&mut control_rx, delay).await {
                            return;
                        }
                    }
                }
            }
            Ok(session) => {
                attempt = 0;
                let grant = session.grant().clone();
                let mut sources = Vec::new();
                for display in &grant.displays {
                    match session.open_pipewire_remote().await {
                        Ok(fd) => sources.push(WlStreamSource {
                            display_id: display.id,
                            node_id: display.node_id,
                            fd,
                        }),
                        Err(error) => {
                            let display_id = display.id;
                            warn!(display_id, %error, "cannot open portal PipeWire remote")
                        }
                    }
                }
                let capture = WaylandCapture::new(pw.clone(), &grant, sources);
                let injector = if grant.kind == PortalKind::RemoteDesktop
                    && grant.devices
                        & (portal_model::device::KEYBOARD | portal_model::device::POINTER)
                        != 0
                {
                    Some(PortalInjector::new(
                        conn.clone(),
                        session.session_path().to_owned(),
                        grant.devices,
                        &inner.handle,
                    ))
                } else {
                    None
                };
                let state = WlState {
                    session,
                    capture,
                    injector,
                    displays: grant.displays.clone(),
                };
                install_wayland_state(&inner, state);
                let (mut closed, mut lost) = {
                    let state = lock(&inner.wl_state);
                    let Some(state) = state.as_ref() else {
                        continue;
                    };
                    (state.session.closed(), state.capture.lost())
                };
                let reason = loop {
                    if *closed.borrow() {
                        break PortalExit::Closed;
                    }
                    if *lost.borrow() {
                        break PortalExit::Lost;
                    }
                    tokio::select! {
                        result = closed.changed() => {
                            if result.is_err() || *closed.borrow() { break PortalExit::Closed; }
                        }
                        result = lost.changed() => {
                            if result.is_err() || *lost.borrow() { break PortalExit::Lost; }
                        }
                        control = control_rx.recv() => match control {
                            Some(PortalControl::Reauthorize) => break PortalExit::Reauthorize,
                            Some(PortalControl::Shutdown) | None => break PortalExit::Shutdown,
                        }
                    }
                };
                let shutdown = matches!(reason, PortalExit::Shutdown);
                clear_wayland_state(&inner, false).await;
                if shutdown {
                    return;
                }
                if matches!(reason, PortalExit::Closed | PortalExit::Lost)
                    && wait_portal_control(&mut control_rx, Duration::from_secs(1)).await
                {
                    return;
                }
            }
        }
    }
}

enum PortalExit {
    Closed,
    Lost,
    Reauthorize,
    Shutdown,
}

async fn wait_portal_control(rx: &mut mpsc::Receiver<PortalControl>, delay: Duration) -> bool {
    match tokio::time::timeout(delay, rx.recv()).await {
        Ok(Some(PortalControl::Shutdown)) | Ok(None) => true,
        Ok(Some(PortalControl::Reauthorize)) => false,
        Err(_) => false,
    }
}

fn install_wayland_state(inner: &Arc<Inner>, state: WlState) {
    let mut slot = lock(&inner.wl_state);
    *slot = Some(state);
    let Some(state) = slot.as_ref() else { return };
    {
        let mut watchers = lock(&inner.wl_display_watchers);
        watchers.retain(|tx| !tx.is_closed());
        for tx in watchers.iter() {
            state.capture.watch_displays(tx.clone());
        }
    }
    {
        let mut watchers = lock(&inner.wl_cursor_watchers);
        watchers.retain(|tx| !tx.is_closed());
        for tx in watchers.iter() {
            state.capture.watch_cursor(tx.clone());
        }
    }
    {
        let mut watchers = lock(&inner.wl_pointer_watchers);
        watchers.retain(|tx| !tx.is_closed());
        for tx in watchers.iter() {
            state.capture.watch_pointer(tx.clone());
        }
    }
}

async fn clear_wayland_state(inner: &Arc<Inner>, _shutdown: bool) {
    let state = lock(&inner.wl_state).take();
    if let Some(state) = state {
        if let Some(injector) = state.injector {
            injector.flush().await;
        }
        drop(state.capture);
        if let Err(error) = state.session.close().await {
            debug!(%error, "portal session close failed");
        }
    }
    let mut watchers = lock(&inner.wl_display_watchers);
    watchers.retain(|tx| {
        if tx.is_closed() {
            false
        } else {
            match tx.try_send(Vec::new()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        }
    });
}

fn portal_error_kind(error: &PortalError) -> &'static str {
    match error {
        PortalError::Unavailable => "unavailable",
        PortalError::Cancelled => "cancelled",
        PortalError::Failed(_) => "failed",
        PortalError::Timeout => "timeout",
        PortalError::Closed => "closed",
        PortalError::DBus(_) => "dbus",
    }
}
