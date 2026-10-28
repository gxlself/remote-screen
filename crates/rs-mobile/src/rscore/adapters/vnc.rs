//! VNC driver over rs-vnc.

use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Instant,
};

use async_trait::async_trait;
use rs_vnc::{
    client::{
        self, AuthAnswer, ClientConfig, ClientEvent, ClientInput, Connected, CredentialPrompt,
    },
    keysym::{ctrl_alt_del, text_events, KeyTracker, PointerState},
    proto::to_latin1,
    security, Rect, Screen, Surface, VncError,
};
use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_transport::net::NetStream;
use tokio::{
    sync::{mpsc, watch},
    time::MissedTickBehavior,
};
use tracing::{debug, info, warn};

use super::driver::{
    AdapterError, DriverHost, DriverInput, DriverParams, FrameSink, ProtocolDriver, SrcPixels,
};
use super::dto::{AdapterProtocol, AdapterScreen, CredField};
use super::screen_guess::{Guesser, Verdict, CONFIRM_GAP};

/// Connections tried per session; each authentication failure dials again
/// and asks again.
pub const MAX_AUTH_ATTEMPTS: u32 = 3;

const CLIENT_INPUT_QUEUE: usize = 256;

#[derive(Debug, Default)]
pub struct VncDriver;

#[async_trait]
impl ProtocolDriver for VncDriver {
    fn protocol(&self) -> AdapterProtocol {
        AdapterProtocol::Vnc
    }

    async fn run(
        &self,
        params: DriverParams,
        host: Arc<dyn DriverHost>,
        sink: Arc<dyn FrameSink>,
        input: mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError> {
        let conn = connect(&params, host.as_ref()).await?;
        host.streaming(
            u32::from(conn.init.width),
            u32::from(conn.init.height),
            &conn.init.name,
        );
        stream(conn, host, sink, input).await
    }
}

pub fn map_vnc_error(e: VncError) -> AdapterError {
    match e {
        VncError::Io(message) => AdapterError::Io(message),
        VncError::Closed(reason) => AdapterError::ServerClosed(reason),
        VncError::Protocol(message) => AdapterError::Protocol(message),
        VncError::UnsupportedVersion(version) => {
            AdapterError::UnsupportedSecurity(format!("version {version}"))
        }
        VncError::UnsupportedSecurity(offered) => {
            AdapterError::UnsupportedSecurity(format!("{offered:?}"))
        }
        VncError::AuthFailed(_) => AdapterError::AuthFailed,
        VncError::Refused(reason) => AdapterError::ServerClosed(reason),
        VncError::Cancelled => AdapterError::Cancelled,
    }
}

async fn connect(
    params: &DriverParams,
    host: &dyn DriverHost,
) -> Result<Connected<NetStream>, AdapterError> {
    let mut failed_before = false;
    for attempt in 1..=MAX_AUTH_ATTEMPTS {
        let stream = params.dial.dial().await?;
        let mut prompt = HostPrompt {
            host,
            failed_before,
            asked: false,
        };
        match client::handshake(stream, &ClientConfig::default(), &mut prompt).await {
            Ok(conn) => {
                if prompt.asked {
                    host.auth_succeeded();
                }
                return Ok(conn);
            }
            Err(VncError::AuthFailed(reason)) => {
                debug!(attempt, %reason, "vnc authentication failed");
                failed_before = true;
            }
            Err(error) => return Err(map_vnc_error(error)),
        }
    }
    Err(AdapterError::AuthFailed)
}

struct HostPrompt<'a> {
    host: &'a dyn DriverHost,
    failed_before: bool,
    asked: bool,
}

#[async_trait]
impl CredentialPrompt for HostPrompt<'_> {
    async fn ask(&mut self, sec: u8) -> Option<AuthAnswer> {
        let fields: &[CredField] = match sec {
            security::ARD => &[CredField::User, CredField::Password],
            _ => &[CredField::Password],
        };
        self.asked = true;
        let credentials = self.host.credentials(fields, self.failed_before).await?;
        self.host.authenticating();
        Some(AuthAnswer {
            user: credentials.user.unwrap_or_default(),
            password: credentials.password,
        })
    }
}

fn pack_size(w: u16, h: u16) -> u32 {
    (u32::from(w) << 16) | u32::from(h)
}

fn unpack_size(packed: u32) -> (u16, u16) {
    ((packed >> 16) as u16, packed as u16)
}

/// Monitor layout of one connection, shared by the decode surface and the
/// session loop: the server's ExtendedDesktopSize screens, or else a guess
/// from the picture for every frame size.
struct Layout {
    host: Arc<dyn DriverHost>,
    /// Server banner version (e.g. `003.889`) for the screen layout logs.
    rfb: String,
    /// Last layout passed to `DriverHost::screens`.
    reported: Option<Vec<AdapterScreen>>,
    /// The server sent a layout; nothing is guessed from then on.
    from_server: bool,
    /// Guessing the current frame size.
    guesser: Option<Guesser>,
    require_full_frame: bool,
}

impl Layout {
    fn report(&mut self, screens: Vec<AdapterScreen>) -> bool {
        if self.reported.as_ref() == Some(&screens) {
            return false;
        }
        self.host.screens(&screens);
        self.reported = Some(screens);
        true
    }

    fn resized(&mut self, w: u16, h: u16) {
        if !self.from_server {
            self.guesser = Some(if self.require_full_frame {
                Guesser::after_resume(u32::from(w), u32::from(h))
            } else {
                Guesser::new(u32::from(w), u32::from(h))
            });
        }
    }

    fn server(&mut self, screens: Vec<AdapterScreen>) {
        self.from_server = true;
        self.guesser = None;
        let count = screens.len();
        if self.report(screens) {
            info!(count, rfb = %self.rfb, "VNC screens reported");
        }
    }

    fn commit(&mut self, painted: bool) {
        if let Some(guesser) = &mut self.guesser {
            let verdict = guesser.commit(painted, Instant::now());
            self.settle(verdict);
        }
    }

    fn poll(&mut self) {
        if let Some(guesser) = &mut self.guesser {
            let verdict = guesser.poll(Instant::now());
            self.settle(verdict);
        }
    }

    fn settle(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Wait => {}
            Verdict::GaveUp => {
                self.guesser = None;
                debug!(rfb = %self.rfb, "VNC screens not guessed: samples disagree");
            }
            Verdict::Agreed(screens) => {
                self.guesser = None;
                // One monitor only needs reporting to replace an earlier guess.
                if screens.is_empty() && self.reported.is_none() {
                    return;
                }
                let count = screens.len();
                let method = screens
                    .first()
                    .and_then(|screen| screen.guess)
                    .map_or("none", |guess| guess.as_str());
                if self.report(screens) {
                    info!(count, method, rfb = %self.rfb, "VNC screens guessed");
                }
            }
        }
    }
}

fn lock(layout: &Mutex<Layout>) -> MutexGuard<'_, Layout> {
    layout.lock().unwrap_or_else(|error| error.into_inner())
}

/// rs-vnc decode target writing into the session's FrameSink.
struct SinkSurface {
    sink: Arc<dyn FrameSink>,
    /// `w << 16 | h`, shared with the input translator.
    size: Arc<AtomicU32>,
    layout: Arc<Mutex<Layout>>,
    /// The first FramebufferUpdate, the answer to the full request sent
    /// right after SetEncodings, has been applied.
    first_update: bool,
    background: watch::Receiver<bool>,
    sink_size: (u16, u16),
}

impl SinkSurface {
    fn visible(&mut self) -> bool {
        if *self.background.borrow() {
            return false;
        }
        let size = self.size();
        if self.sink_size != size {
            self.sink.resize(u32::from(size.0), u32::from(size.1));
            self.sink_size = size;
        }
        true
    }
}

impl Surface for SinkSurface {
    fn resize(&mut self, w: u16, h: u16) {
        self.size.store(pack_size(w, h), Ordering::Relaxed);
        if !self.visible() {
            return;
        }
        lock(&self.layout).resized(w, h);
    }

    fn size(&self) -> (u16, u16) {
        unpack_size(self.size.load(Ordering::Relaxed))
    }

    fn put_bgra(&mut self, r: Rect, src: &[u8], stride: usize) {
        if !self.visible() {
            return;
        }
        let (x, y, w, h) = (
            u32::from(r.x),
            u32::from(r.y),
            u32::from(r.w),
            u32::from(r.h),
        );
        self.sink.put(x, y, w, h, src, stride, SrcPixels::Bgra8);
        if let Some(guesser) = &mut lock(&self.layout).guesser {
            guesser.put(x, y, w, h, src, stride);
        }
    }

    fn fill(&mut self, r: Rect, bgra: [u8; 4]) {
        if !self.visible() {
            return;
        }
        let (x, y, w, h) = (
            u32::from(r.x),
            u32::from(r.y),
            u32::from(r.w),
            u32::from(r.h),
        );
        self.sink.fill(x, y, w, h, bgra);
        if let Some(guesser) = &mut lock(&self.layout).guesser {
            guesser.fill(x, y, w, h, bgra);
        }
    }

    fn copy_rect(&mut self, sx: u16, sy: u16, r: Rect) {
        if !self.visible() {
            return;
        }
        let (sx, sy, x, y, w, h) = (
            u32::from(sx),
            u32::from(sy),
            u32::from(r.x),
            u32::from(r.y),
            u32::from(r.w),
            u32::from(r.h),
        );
        self.sink.copy_within(sx, sy, x, y, w, h);
        if let Some(guesser) = &mut lock(&self.layout).guesser {
            guesser.copy(sx, sy, x, y, w, h);
        }
    }

    fn commit(&mut self, dirty: &[Rect]) {
        if !self.visible() {
            return;
        }
        self.sink.commit();
        let mut layout = lock(&self.layout);
        // A server with ExtendedDesktopSize puts its layout in that answer.
        if !self.first_update {
            self.first_update = true;
            if !layout.from_server {
                info!(rfb = %layout.rfb, "VNC server sent no screen layout");
            }
        }
        layout.commit(!dirty.is_empty());
    }

    fn cursor(&mut self, w: u16, h: u16, hot_x: u16, hot_y: u16, bgra: &[u8]) {
        if !self.visible() {
            return;
        }
        let mut rgba = bgra.to_vec();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        self.sink.cursor_shape(
            u32::from(w),
            u32::from(h),
            u32::from(hot_x),
            u32::from(hot_y),
            &rgba,
        );
    }

    fn pointer_pos(&mut self, x: u16, y: u16) {
        if !self.visible() {
            return;
        }
        self.sink.cursor_pos(u32::from(x), u32::from(y));
    }

    fn screens(&mut self, screens: &[Screen]) {
        let screens: Vec<AdapterScreen> = screens
            .iter()
            .map(|screen| AdapterScreen {
                x: u32::from(screen.rect.x),
                y: u32::from(screen.rect.y),
                width: u32::from(screen.rect.w),
                height: u32::from(screen.rect.h),
                guess: None,
            })
            .collect();
        lock(&self.layout).server(screens);
    }
}

/// Streams until `client::run` ends: Ok when `input` closes, otherwise the
/// connection error.
async fn stream(
    conn: Connected<NetStream>,
    host: Arc<dyn DriverHost>,
    sink: Arc<dyn FrameSink>,
    input: mpsc::Receiver<DriverInput>,
) -> Result<(), AdapterError> {
    let mut background = host.background();
    let size = Arc::new(AtomicU32::new(pack_size(conn.init.width, conn.init.height)));
    let layout = Arc::new(Mutex::new(Layout {
        host: host.clone(),
        rfb: conn.server_version.clone(),
        reported: None,
        from_server: false,
        guesser: None,
        require_full_frame: false,
    }));
    let mut surface = SinkSurface {
        sink,
        size: size.clone(),
        layout: layout.clone(),
        first_update: false,
        background: background.clone(),
        sink_size: (0, 0),
    };
    let (client_tx, client_rx) = mpsc::channel(CLIENT_INPUT_QUEUE);
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let session =
        client::run_with_background(conn, &mut surface, client_rx, event_tx, background.clone());
    let pump = pump_input(
        input,
        client_tx,
        InputTranslator::new(size.clone()),
        background.clone(),
    );
    tokio::pin!(session, pump);
    // A still desktop sends no updates; confirm a guess between them.
    let mut confirm =
        tokio::time::interval_at(tokio::time::Instant::now() + CONFIRM_GAP, CONFIRM_GAP);
    confirm.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut pump_done = false;
    let mut visibility_open = true;
    let mut paused = false;
    let result = loop {
        let next = *background.borrow_and_update();
        if paused != next {
            paused = next;
            let mut layout = lock(&layout);
            layout.require_full_frame = true;
            layout.guesser = if !paused && !layout.from_server {
                let (w, h) = unpack_size(size.load(Ordering::Relaxed));
                Some(Guesser::after_resume(u32::from(w), u32::from(h)))
            } else {
                None
            };
            info!(
                session_id = host.session_id(),
                protocol = "vnc",
                method = "stop-requests",
                paused,
                "adapter background changed"
            );
        }
        tokio::select! {
            biased;
            changed = background.changed(), if visibility_open => visibility_open = changed.is_ok(),
            result = &mut session => break result,
            Some(event) = event_rx.recv() => on_event(host.as_ref(), event),
            () = &mut pump, if !pump_done => pump_done = true,
            _ = confirm.tick(), if !paused => lock(&layout).poll(),
        }
    };
    while let Ok(event) = event_rx.try_recv() {
        on_event(host.as_ref(), event);
    }
    result.map_err(map_vnc_error)
}

fn on_event(host: &dyn DriverHost, event: ClientEvent) {
    match event {
        ClientEvent::CutText(text) => host.clipboard(&text),
        ClientEvent::Bell => debug!("vnc bell"),
        ClientEvent::Resized { w, h } => debug!(w, h, "vnc desktop resized"),
    }
}

/// Translates driver input until `input` closes, then drops `out` so
/// `client::run` returns Ok.
async fn pump_input(
    mut input: mpsc::Receiver<DriverInput>,
    out: mpsc::Sender<ClientInput>,
    mut translator: InputTranslator,
    background: watch::Receiver<bool>,
) {
    while let Some(message) = input.recv().await {
        if *background.borrow() && !matches!(&message, DriverInput::Clipboard(_)) {
            continue;
        }
        for event in translator.translate(message) {
            if out.send(event).await.is_err() {
                return;
            }
        }
    }
}

struct InputTranslator {
    size: Arc<AtomicU32>,
    keys: KeyTracker,
    pointer: PointerState,
    /// The touch acting as the left button.
    touch: Option<u8>,
}

impl InputTranslator {
    fn new(size: Arc<AtomicU32>) -> Self {
        Self {
            size,
            keys: KeyTracker::new(),
            pointer: PointerState::new(),
            touch: None,
        }
    }

    fn translate(&mut self, input: DriverInput) -> Vec<ClientInput> {
        match input {
            DriverInput::Hid(message) => self.hid(message),
            DriverInput::Clipboard(text) => match to_latin1(&text) {
                Some(bytes) => vec![ClientInput::CutText(bytes)],
                None => {
                    warn!(
                        chars = text.chars().count(),
                        "clipboard text is not Latin-1; not sent to the vnc server"
                    );
                    Vec::new()
                }
            },
            DriverInput::TypeSecret(password) => keys(text_events(&password)),
            DriverInput::CtrlAltDel => keys(ctrl_alt_del()),
            DriverInput::Resize { .. } | DriverInput::Monitors(_) => Vec::new(),
        }
    }

    fn hid(&mut self, message: InputMsg) -> Vec<ClientInput> {
        let (w, h) = unpack_size(self.size.load(Ordering::Relaxed));
        match message {
            InputMsg::MouseMove { x, y } => vec![pointer(self.pointer.move_to(x, y, w, h))],
            InputMsg::MouseButton {
                button, down, x, y, ..
            } => pointers(self.pointer.button(button as u8, down, x, y, w, h)),
            InputMsg::Scroll {
                dx,
                dy,
                phase,
                unit,
                ..
            } => {
                if phase == ScrollPhase::Began {
                    self.pointer.reset_scroll();
                }
                pointers(self.pointer.scroll(dx, dy, unit == ScrollUnit::Line))
            }
            InputMsg::Key { usage, down, mods } => keys(self.keys.key(usage, down, mods.bits())),
            InputMsg::Text(text) => keys(text_events(&text)),
            InputMsg::ReleaseAll => {
                self.touch = None;
                let mut out = keys(self.keys.release_all());
                out.extend(self.pointer.release_all().map(pointer));
                out
            }
            InputMsg::Touch {
                id, phase, x, y, ..
            } => match phase {
                TouchPhase::Began if self.touch.is_none() => {
                    self.touch = Some(id);
                    pointers(self.pointer.button(0, true, x, y, w, h))
                }
                TouchPhase::Moved if self.touch == Some(id) => {
                    vec![pointer(self.pointer.move_to(x, y, w, h))]
                }
                TouchPhase::Ended | TouchPhase::Cancelled if self.touch == Some(id) => {
                    self.touch = None;
                    pointers(self.pointer.button(0, false, x, y, w, h))
                }
                _ => Vec::new(),
            },
        }
    }
}

fn keys(events: Vec<(bool, u32)>) -> Vec<ClientInput> {
    events
        .into_iter()
        .map(|(down, keysym)| ClientInput::Key { down, keysym })
        .collect()
}

fn pointer((mask, x, y): (u8, u16, u16)) -> ClientInput {
    ClientInput::Pointer { mask, x, y }
}

fn pointers(events: Vec<(u8, u16, u16)>) -> Vec<ClientInput> {
    events.into_iter().map(pointer).collect()
}
