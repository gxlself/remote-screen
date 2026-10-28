//! Service-side control proxy for the active Windows session process.
//!
//! All sends are nonblocking. Synchronous requests release the state lock before
//! waiting on a standard-library channel, including when called from Tokio.

use std::collections::HashMap;
use std::sync::{mpsc as sync_mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use host_core::inject::win_plan::{modifier_usages, HeldInput, ScreenRect, WinPlanner};
use host_core::platform::{
    AudioCaptureConfig, AudioChunkSink, CaptureConfig, CaptureHandle, ClipboardItem, ConsoleState,
    CursorUpdate, DisplayInfo, Encoder, EncoderConfig, FrameSink, HostInfo, InputEvent, InputMsg,
    KeyStore, Nic, PermissionKind, Permissions, PixelRect, Platform, PlatformError, Result,
    SessionActionKind,
};
use host_core::sysauth::SystemAuthenticator;
use rsp_core::pb::clipboard_header::Kind;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::pipe_proto::{
    ctl_to_cursor, input_to_b64, BlobKind, BlobPacket, Ctl, PipeFrame, WatchKind,
};

/// Operations that must remain in the service rather than the session process.
pub trait ServiceLocal: Send + Sync + 'static {
    fn info(&self) -> HostInfo;
    fn console_state(&self) -> ConsoleState;
    fn nics(&self) -> Vec<Nic>;
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()>;
    fn keystore(&self) -> &dyn KeyStore;
    fn ctrl_alt_del(&self) -> Result<()>;
    fn system_authenticator(&self) -> Option<Arc<dyn SystemAuthenticator>> {
        None
    }
    fn virtual_displays(&self) -> Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProxyTimeouts {
    pub request: Duration,
    pub open: Duration,
}

impl Default for ProxyTimeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(2),
            open: Duration::from_secs(5),
        }
    }
}

type Reply = Result<Option<BlobPacket>>;

const SHADOW_SCREEN: ScreenRect = ScreenRect {
    left: 0,
    top: 0,
    width: 1,
    height: 1,
};

#[derive(Default)]
pub(crate) struct Inner {
    pub(crate) media: crate::proxy_media::MediaState,
    pub(crate) generation: u64,
    out: Option<mpsc::UnboundedSender<PipeFrame>>,
    pub(crate) next_req: u32,
    pub(crate) pending: HashMap<u32, sync_mpsc::Sender<Reply>>,
    displays: Vec<DisplayInfo>,
    desktop: Option<String>,
    display_watchers: Vec<mpsc::Sender<Vec<DisplayInfo>>>,
    cursor_watchers: Vec<mpsc::Sender<CursorUpdate>>,
    activity_watchers: Vec<mpsc::Sender<bool>>,
    clipboard_watchers: Vec<mpsc::Sender<()>>,
    keep_awake: bool,
    dropped_inputs: u64,
    shadow: WinPlanner,
    last_pointer: Option<(u32, f32, f32)>,
}

impl Inner {
    fn disconnect(&mut self) {
        self.out = None;
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(Err(platform_error("session process disconnected")));
        }
    }

    pub(crate) fn connected(&mut self) -> bool {
        if self.out.as_ref().is_some_and(|out| !out.is_closed()) {
            true
        } else {
            self.disconnect();
            false
        }
    }

    pub(crate) fn send(&mut self, frame: PipeFrame) -> Result<()> {
        if self.out.as_ref().is_some_and(|out| out.send(frame).is_ok()) {
            Ok(())
        } else {
            self.disconnect();
            Err(platform_error("session process not connected"))
        }
    }

    fn complete(&mut self, req: u32, reply: Reply) {
        if let Some(tx) = self.pending.remove(&req) {
            let _ = tx.send(reply);
        }
    }

    fn dropped_input(&mut self) {
        self.dropped_inputs = self.dropped_inputs.saturating_add(1);
        tracing::debug!(
            dropped_inputs = self.dropped_inputs,
            "discarding input unavailable to the session process"
        );
    }
}

pub struct PipePlatform<L: ServiceLocal> {
    local: L,
    inner: Arc<Mutex<Inner>>,
    timeouts: ProxyTimeouts,
}

impl<L: ServiceLocal> PipePlatform<L> {
    pub fn new(local: L) -> Self {
        Self::with_timeouts(local, ProxyTimeouts::default())
    }

    pub fn with_timeouts(local: L, t: ProxyTimeouts) -> Self {
        Self {
            local,
            inner: Arc::new(Mutex::new(Inner::default())),
            timeouts: t,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Called after the handshake; invalidates the previous connection and its requests.
    pub fn attach(&self, out: mpsc::UnboundedSender<PipeFrame>) -> u64 {
        let mut inner = self.lock();
        inner.disconnect();
        inner.generation = inner.generation.wrapping_add(1);
        inner.out = Some(out);
        let held = inner.shadow.held();
        if !held.is_empty() {
            tracing::info!(
                keys = held.keys.len(),
                modifiers = held.mods.bits().count_ones(),
                buttons = held.buttons.len(),
                "releasing inputs held by previous session process"
            );
            replay_held(&mut inner, &held);
        }
        inner.shadow = WinPlanner::new();
        inner.cursor_watchers.retain(|tx| !tx.is_closed());
        inner.activity_watchers.retain(|tx| !tx.is_closed());
        inner.clipboard_watchers.retain(|tx| !tx.is_closed());
        for (what, needed) in [
            (WatchKind::Cursor, !inner.cursor_watchers.is_empty()),
            (WatchKind::Activity, !inner.activity_watchers.is_empty()),
            (WatchKind::Clipboard, !inner.clipboard_watchers.is_empty()),
        ] {
            if needed {
                let _ = inner.send(PipeFrame::Ctl(Ctl::Watch { what }));
            }
        }
        if inner.keep_awake {
            let _ = inner.send(PipeFrame::Ctl(Ctl::KeepAwake { on: true }));
        }
        replay_media(&mut inner);
        inner.generation
    }

    /// Deliver a post-handshake frame; stale or disconnected generations are ignored.
    pub fn on_frame(&self, generation: u64, frame: PipeFrame) {
        let mut inner = self.lock();
        if generation != inner.generation || !inner.connected() {
            return;
        }
        match frame {
            PipeFrame::Ctl(Ctl::Done { req, error }) => {
                on_media(
                    &mut inner,
                    PipeFrame::Ctl(Ctl::Done {
                        req,
                        error: error.clone(),
                    }),
                );
                inner.complete(req, error.map_or(Ok(None), |error| Err(error.into())));
            }
            PipeFrame::Blob(blob) => inner.complete(blob.req, Ok(Some(blob))),
            PipeFrame::Ctl(Ctl::Displays { list }) => {
                let displays: Vec<DisplayInfo> = list.into_iter().map(Into::into).collect();
                if displays != inner.displays {
                    inner.displays = displays.clone();
                    distribute(&mut inner.display_watchers, displays);
                }
            }
            PipeFrame::Ctl(
                ctl @ (Ctl::CursorShape { .. }
                | Ctl::CursorUse { .. }
                | Ctl::CursorHidden { .. }
                | Ctl::CursorPos { .. }),
            ) => {
                if let Some(update) = ctl_to_cursor(&ctl) {
                    if distribute(&mut inner.cursor_watchers, update) {
                        let _ = inner.send(PipeFrame::Ctl(Ctl::Unwatch {
                            what: WatchKind::Cursor,
                        }));
                    }
                }
            }
            PipeFrame::Ctl(Ctl::Activity { active }) => {
                if distribute(&mut inner.activity_watchers, active) {
                    let _ = inner.send(PipeFrame::Ctl(Ctl::Unwatch {
                        what: WatchKind::Activity,
                    }));
                }
            }
            PipeFrame::Ctl(Ctl::ClipboardChanged) => {
                if distribute(&mut inner.clipboard_watchers, ()) {
                    let _ = inner.send(PipeFrame::Ctl(Ctl::Unwatch {
                        what: WatchKind::Clipboard,
                    }));
                }
            }
            PipeFrame::Ctl(Ctl::Desktop { name }) => inner.desktop = Some(name),
            PipeFrame::Ctl(Ctl::Pong { .. }) => {}
            frame @ (PipeFrame::Video(_)
            | PipeFrame::Audio(_)
            | PipeFrame::Ctl(
                Ctl::Frame { .. }
                | Ctl::CaptureEnded { .. }
                | Ctl::EncoderFailed { .. }
                | Ctl::EncoderInfo { .. },
            )) => on_media(&mut inner, frame),
            // Never log a frame: unexpected frames can contain input or clipboard data.
            _ => tracing::debug!("discarding unexpected session process message"),
        }
    }

    pub fn detach(&self, generation: u64) {
        let mut inner = self.lock();
        if generation == inner.generation {
            inner.disconnect();
            crate::proxy_media::detach(&mut inner);
        }
    }

    pub fn is_attached(&self) -> bool {
        self.lock().connected()
    }

    pub fn desktop(&self) -> Option<String> {
        self.lock().desktop.clone()
    }

    fn request(&self, make_frame: impl FnOnce(u32) -> PipeFrame, timeout: Duration) -> Reply {
        let (tx, rx) = sync_mpsc::channel();
        let (generation, req) = {
            let mut inner = self.lock();
            if !inner.connected() {
                return Err(platform_error("session process not connected"));
            }
            let req = loop {
                inner.next_req = inner.next_req.wrapping_add(1);
                if !inner.pending.contains_key(&inner.next_req) {
                    break inner.next_req;
                }
            };
            inner.pending.insert(req, tx);
            inner.send(make_frame(req))?;
            (inner.generation, req)
        };

        // No state lock is held while the synchronous Platform caller waits.
        match rx.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(error) => {
                let mut inner = self.lock();
                if inner.generation == generation {
                    inner.pending.remove(&req);
                }
                Err(platform_error(match error {
                    sync_mpsc::RecvTimeoutError::Timeout => "session process timeout",
                    sync_mpsc::RecvTimeoutError::Disconnected => "session process disconnected",
                }))
            }
        }
    }
}

impl<L: ServiceLocal> Platform for PipePlatform<L> {
    fn info(&self) -> HostInfo {
        self.local.info()
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.lock().displays.clone()
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        let mut inner = self.lock();
        inner.display_watchers.retain(|tx| !tx.is_closed());
        if !tx.is_closed() {
            inner.display_watchers.push(tx);
        }
    }

    fn capture(
        &self,
        _display: u32,
        _cfg: CaptureConfig,
        _sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        crate::proxy_media::capture(&self.inner, self.timeouts.open, _display, _cfg, _sink)
    }

    fn capture_region_supported(&self) -> bool {
        crate::viewport_crop::CROP_SUPPORTED
    }

    fn capture_region(
        &self,
        display: u32,
        rect: PixelRect,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        crate::proxy_media::capture_region(
            &self.inner,
            self.timeouts.open,
            display,
            rect,
            cfg,
            sink,
        )
    }

    fn encoder(&self, _cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        crate::proxy_media::encoder(&self.inner, self.timeouts.open, _cfg)
    }

    fn capture_audio(
        &self,
        _cfg: AudioCaptureConfig,
        _sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        crate::proxy_media::capture_audio(&self.inner, self.timeouts.open, _cfg, _sink)
    }

    fn audio_capture_supported(&self) -> bool {
        true
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.inject_input(&input_from_m0(ev), display);
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        let mut inner = self.lock();
        if !inner.connected() {
            inner.dropped_input();
            return;
        }
        let Ok(input) = input_to_b64(msg) else {
            inner.dropped_input();
            return;
        };
        let frame = PipeFrame::Ctl(Ctl::Inject {
            display_id: display.id,
            input,
        });
        if inner.send(frame).is_err() {
            inner.dropped_input();
            return;
        }
        inner.shadow.plan(msg, SHADOW_SCREEN, SHADOW_SCREEN);
        match msg {
            InputMsg::MouseMove { x, y } | InputMsg::MouseButton { x, y, .. } => {
                inner.last_pointer = Some((display.id, *x, *y));
            }
            _ => {}
        }
    }

    fn release_all(&self) {
        let mut inner = self.lock();
        if inner.send(PipeFrame::Ctl(Ctl::ReleaseAll)).is_ok() {
            inner.shadow.release_all();
        }
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        match kind {
            SessionActionKind::CtrlAltDel => self.local.ctrl_alt_del(),
            // Host-core implements this policy by issuing LockScreen on session end.
            SessionActionKind::LockOnEnd => Ok(()),
            SessionActionKind::Unspecified => Err(PlatformError::Unsupported),
            SessionActionKind::LockScreen | SessionActionKind::WakeDisplay => done(self.request(
                |req| {
                    PipeFrame::Ctl(Ctl::Action {
                        req,
                        kind: kind as i32,
                    })
                },
                self.timeouts.request,
            )),
        }
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        let mut inner = self.lock();
        if register(&mut inner.cursor_watchers, tx) {
            let _ = inner.send(PipeFrame::Ctl(Ctl::Watch {
                what: WatchKind::Cursor,
            }));
        }
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        let mut inner = self.lock();
        if register(&mut inner.activity_watchers, tx) {
            let _ = inner.send(PipeFrame::Ctl(Ctl::Watch {
                what: WatchKind::Activity,
            }));
        }
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        // host-core calls this synchronously from its async clipboard pump.
        let timeout = self.timeouts.request.min(Duration::from_millis(250));
        let blob = self
            .request(|req| PipeFrame::Ctl(Ctl::ClipboardGet { req }), timeout)
            .ok()??;
        Some(ClipboardItem {
            kind: match blob.kind {
                BlobKind::Text => Kind::Text,
                BlobKind::Png => Kind::Png,
            },
            sha256: Sha256::digest(&blob.data).into(),
            data: blob.data,
        })
    }

    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        if !self.is_attached() {
            return Err(platform_error("session process not connected"));
        }
        let kind = match item.kind {
            Kind::Text => BlobKind::Text,
            Kind::Png => BlobKind::Png,
            _ => return Err(PlatformError::Unsupported),
        };
        done(self.request(
            |req| {
                PipeFrame::Blob(BlobPacket {
                    req,
                    kind,
                    data: item.data,
                })
            },
            self.timeouts.request,
        ))
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        let mut inner = self.lock();
        if register(&mut inner.clipboard_watchers, tx) {
            let _ = inner.send(PipeFrame::Ctl(Ctl::Watch {
                what: WatchKind::Clipboard,
            }));
        }
    }

    fn power_keep_awake(&self, on: bool) {
        let mut inner = self.lock();
        inner.keep_awake = on;
        let _ = inner.send(PipeFrame::Ctl(Ctl::KeepAwake { on }));
    }

    fn console_state(&self) -> ConsoleState {
        self.local.console_state()
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            screen_recording: true,
            accessibility: true,
        }
    }

    fn request_permission(&self, _p: PermissionKind) {}

    fn nics(&self) -> Vec<Nic> {
        self.local.nics()
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        self.local.send_wol(mac, via)
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.local.keystore()
    }

    fn system_authenticator(&self) -> Option<Arc<dyn SystemAuthenticator>> {
        self.local.system_authenticator()
    }

    fn virtual_displays(&self) -> Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>> {
        self.local.virtual_displays()
    }
}

fn platform_error(message: &str) -> PlatformError {
    PlatformError::Platform(message.into())
}

fn done(reply: Reply) -> Result<()> {
    match reply? {
        None => Ok(()),
        Some(_) => Err(platform_error("unexpected session process response")),
    }
}

/// Returns true when this registration needs a remote subscription.
fn register<T>(watchers: &mut Vec<mpsc::Sender<T>>, tx: mpsc::Sender<T>) -> bool {
    if tx.is_closed() {
        return false;
    }
    watchers.retain(|tx| !tx.is_closed());
    let first = watchers.is_empty();
    watchers.push(tx);
    first
}

/// Full channels remain subscribed; only closed channels are removed.
/// Returns true if the last watcher was removed, requiring an Unwatch.
fn distribute<T: Clone>(watchers: &mut Vec<mpsc::Sender<T>>, value: T) -> bool {
    let had_watchers = !watchers.is_empty();
    watchers.retain(|tx| {
        !matches!(
            tx.try_send(value.clone()),
            Err(mpsc::error::TrySendError::Closed(_))
        )
    });
    had_watchers && watchers.is_empty()
}

fn input_from_m0(ev: InputEvent) -> InputMsg {
    use rsp_core::hid::{Button, ScrollPhase, ScrollUnit};
    use rsp_core::input::MouseButton;

    match ev {
        InputEvent::MouseMove { x, y } => InputMsg::MouseMove { x, y },
        InputEvent::MouseButton {
            button,
            down,
            x,
            y,
            clicks,
        } => InputMsg::MouseButton {
            button: match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            },
            down,
            x,
            y,
            clicks,
        },
        InputEvent::Scroll { dx, dy } => InputMsg::Scroll {
            dx,
            dy,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        },
    }
}

fn replay_held(inner: &mut Inner, held: &HeldInput) {
    for &usage in held.keys.iter().rev() {
        let msg = InputMsg::Key {
            usage,
            down: false,
            mods: rsp_core::hid::Mods::NONE,
        };
        send_replay_input(inner, msg, 0);
    }

    for usage in modifier_usages(held.mods) {
        let msg = InputMsg::Key {
            usage,
            down: false,
            mods: rsp_core::hid::Mods::NONE,
        };
        send_replay_input(inner, msg, 0);
    }

    let (display_id, x, y) = inner.last_pointer.unwrap_or((0, 0.5, 0.5));
    for button in &held.buttons {
        let msg = InputMsg::MouseButton {
            button: *button,
            down: false,
            x,
            y,
            clicks: 1,
        };
        send_replay_input(inner, msg, display_id);
    }
}

fn send_replay_input(inner: &mut Inner, msg: InputMsg, display_id: u32) {
    let Ok(input) = input_to_b64(&msg) else {
        return;
    };
    let _ = inner.send(PipeFrame::Ctl(Ctl::Inject { display_id, input }));
}

// Media hooks share the same replay and dispatch path as Platform methods.
fn replay_media(_inner: &mut Inner) {
    crate::proxy_media::replay_media(_inner);
}

fn on_media(_inner: &mut Inner, _frame: PipeFrame) {
    crate::proxy_media::on_media(_inner, _frame);
}

impl<L: ServiceLocal + Clone> Clone for PipePlatform<L> {
    fn clone(&self) -> Self {
        Self {
            local: self.local.clone(),
            inner: Arc::clone(&self.inner),
            timeouts: self.timeouts,
        }
    }
}
