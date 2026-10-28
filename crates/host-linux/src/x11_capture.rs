//! X11 screen capture using XDamage, XShm and XFixes.
//!
//! The X connection is owned by one worker thread. Public methods enqueue
//! commands and read snapshots, keeping x11rb resources in that thread.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use host_core::capture_model::{CursorModel, Pacer};
use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, CursorUpdate, DisplayInfo, FrameSink,
    PlatformError,
};
use tokio::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::{damage, randr, shm, xfixes, xproto};
use x11rb::rust_connection::RustConnection;

use crate::capture_model::{
    argb32_to_bgra, cursor_to_shape, display_id_for_name, pointer_report, CaptureHub, CursorPixels,
    HubHandle,
};
use crate::frame::{copy_to_bgrx, FramePool, SrcFormat};

const MAX_SLEEP: Duration = Duration::from_millis(100);
const CURSOR_PERIOD: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct X11Stats {
    pub ticks: u64,
    pub idle_ticks: u64,
    pub encoded: u64,
    pub pool_full: u64,
    pub errors: u64,
    pub shm: bool,
}

struct Inner {
    commands: std_mpsc::Sender<Command>,
    displays: Mutex<Vec<DisplayInfo>>,
    stats: Mutex<HashMap<u32, X11Stats>>,
}

pub struct X11Capture {
    inner: Arc<Inner>,
    worker: Option<JoinHandle<()>>,
}

enum Command {
    Add {
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
        reply: std_mpsc::Sender<Result<u64, String>>,
    },
    Remove {
        display: u32,
        id: u64,
    },
    WatchDisplays(mpsc::Sender<Vec<DisplayInfo>>),
    WatchCursor(mpsc::Sender<CursorUpdate>),
    Stop,
}

impl X11Capture {
    /// Connect to an X11 display. `None` uses the process's `$DISPLAY`.
    pub fn connect(display: Option<&str>) -> Result<Self, PlatformError> {
        let (conn, screen_num) = RustConnection::connect(display).map_err(platform_error)?;
        let setup = conn.setup().clone();
        let screen = setup
            .roots
            .get(screen_num)
            .ok_or(PlatformError::Unsupported)?;

        // A 24-bit root visual is stored in 32 bits on little-endian servers.
        let bpp32 = setup
            .pixmap_formats
            .iter()
            .any(|format| format.depth == screen.root_depth && format.bits_per_pixel == 32);
        if !bpp32 || setup.image_byte_order != xproto::ImageOrder::LSB_FIRST {
            return Err(PlatformError::Unsupported);
        }

        let damage_version = damage::query_version(&conn, 1, 1)
            .map_err(platform_error)?
            .reply()
            .map_err(platform_error)?;
        if damage_version.major_version == 0 {
            return Err(PlatformError::Unsupported);
        }
        let xfixes_version = xfixes::query_version(&conn, 2, 0)
            .map_err(platform_error)?
            .reply()
            .map_err(platform_error)?;
        if xfixes_version.major_version < 2 {
            return Err(PlatformError::Unsupported);
        }

        let randr_version = randr::query_version(&conn, 1, 5)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        if randr_version.is_some() {
            let mask = randr::NotifyMask::SCREEN_CHANGE
                | randr::NotifyMask::CRTC_CHANGE
                | randr::NotifyMask::OUTPUT_CHANGE;
            if let Ok(cookie) = randr::select_input(&conn, screen.root, mask) {
                let _ = cookie.check();
            }
        }

        let damage_id = conn.generate_id().map_err(platform_error)?;
        damage::create(
            &conn,
            damage_id,
            screen.root,
            damage::ReportLevel::NON_EMPTY,
        )
        .map_err(platform_error)?
        .check()
        .map_err(platform_error)?;
        let parts = conn.generate_id().map_err(platform_error)?;
        xfixes::create_region(&conn, parts, &[])
            .map_err(platform_error)?
            .check()
            .map_err(platform_error)?;

        let displays = match read_displays(&conn, screen.root, screen, randr_version) {
            Ok(displays) => displays,
            Err(error) => {
                let _ = damage::destroy(&conn, damage_id);
                let _ = xfixes::destroy_region(&conn, parts);
                return Err(error);
            }
        };

        let mut shm_segment = None;
        let shm_supported = std::env::var_os("RS_X11_NO_SHM").is_none()
            && shm::query_version(&conn)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|version| {
                    version.major_version > 1
                        || (version.major_version == 1 && version.minor_version >= 2)
                });
        if shm_supported {
            let max_bytes = displays
                .iter()
                .filter_map(|info| {
                    usize::try_from(info.width_px)
                        .ok()?
                        .checked_mul(usize::try_from(info.height_px).ok()?)
                        .and_then(|n| n.checked_mul(4))
                })
                .max()
                .unwrap_or(0);
            if max_bytes != 0 {
                shm_segment = ShmSegment::new(&conn, max_bytes).ok();
            }
        }

        let shm_enabled = shm_segment.is_some();
        let (command_tx, command_rx) = std_mpsc::channel();
        let stats = displays
            .iter()
            .map(|info| {
                (
                    info.id,
                    X11Stats {
                        shm: shm_enabled,
                        ..X11Stats::default()
                    },
                )
            })
            .collect();
        let inner = Arc::new(Inner {
            commands: command_tx,
            displays: Mutex::new(displays.clone()),
            stats: Mutex::new(stats),
        });
        let runtime = Runtime {
            conn,
            root: screen.root,
            screen: screen.clone(),
            damage: damage_id,
            parts,
            randr_version,
            shm: shm_segment,
            shm_enabled,
            hub: CaptureHub::new(),
            states: displays
                .into_iter()
                .map(|info| {
                    let id = info.id;
                    (id, DisplayState::new(info))
                })
                .collect(),
            commands: command_rx,
            display_watchers: Vec::new(),
            cursor_watchers: Vec::new(),
            cursor_enabled: false,
            cursor: CursorModel::new(),
            hot: (0, 0),
            pointer: None,
            pointer_seq: 1,
            next_cursor: Instant::now(),
            started: Instant::now(),
            inner: Arc::clone(&inner),
        };
        let worker = thread::Builder::new()
            .name("rs-x11-cap".to_owned())
            .spawn(move || runtime.run())
            .map_err(PlatformError::Io)?;

        Ok(Self {
            inner,
            worker: Some(worker),
        })
    }

    pub fn displays(&self) -> Vec<DisplayInfo> {
        lock(&self.inner.displays).clone()
    }

    pub fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        let _ = self.inner.commands.send(Command::WatchDisplays(tx));
    }

    pub fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, PlatformError> {
        let (reply_tx, reply_rx) = std_mpsc::channel();
        self.inner
            .commands
            .send(Command::Add {
                display,
                cfg,
                sink,
                reply: reply_tx,
            })
            .map_err(|_| PlatformError::Platform("X11 capture worker stopped".to_owned()))?;
        let id = reply_rx
            .recv()
            .map_err(|_| PlatformError::Platform("X11 capture worker stopped".to_owned()))?
            .map_err(PlatformError::Platform)?;
        let commands = self.inner.commands.clone();
        Ok(Box::new(HubHandle::new(move || {
            let _ = commands.send(Command::Remove { display, id });
        })))
    }

    pub fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        let _ = self.inner.commands.send(Command::WatchCursor(tx));
    }

    pub fn stats(&self, display: u32) -> Option<X11Stats> {
        lock(&self.inner.stats).get(&display).copied()
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        let _ = self.inner.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct DisplayState {
    info: DisplayInfo,
    generation: u64,
    dirty: bool,
    fps: u32,
    pacer: Pacer,
    pool: FramePool,
}

impl DisplayState {
    fn new(info: DisplayInfo) -> Self {
        Self {
            info,
            generation: 0,
            dirty: false,
            fps: 0,
            pacer: Pacer::new(1),
            pool: FramePool::new(4),
        }
    }
}

struct Runtime {
    conn: RustConnection,
    root: xproto::Window,
    screen: xproto::Screen,
    damage: damage::Damage,
    parts: xfixes::Region,
    randr_version: Option<randr::QueryVersionReply>,
    shm: Option<ShmSegment>,
    shm_enabled: bool,
    hub: CaptureHub<u32>,
    states: HashMap<u32, DisplayState>,
    commands: std_mpsc::Receiver<Command>,
    display_watchers: Vec<mpsc::Sender<Vec<DisplayInfo>>>,
    cursor_watchers: Vec<mpsc::Sender<CursorUpdate>>,
    cursor_enabled: bool,
    cursor: CursorModel,
    hot: (u16, u16),
    pointer: Option<(i16, i16, bool)>,
    pointer_seq: i64,
    next_cursor: Instant,
    started: Instant,
    inner: Arc<Inner>,
}

impl Runtime {
    fn run(mut self) {
        loop {
            if self.process_commands() {
                break;
            }
            if !self.process_events() {
                break;
            }
            let now = Instant::now();
            let now_duration = now.saturating_duration_since(self.started);
            if self.cursor_enabled && now >= self.next_cursor {
                self.query_pointer();
                self.next_cursor = now + CURSOR_PERIOD;
            }
            self.capture_due(now_duration);
            match self.commands.recv_timeout(self.next_wait(Instant::now())) {
                Ok(Command::Stop) => break,
                Ok(command) => self.handle_command(command),
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        self.cleanup();
    }

    fn process_commands(&mut self) -> bool {
        loop {
            match self.commands.try_recv() {
                Ok(Command::Stop) => return true,
                Ok(command) => self.handle_command(command),
                Err(std_mpsc::TryRecvError::Empty) => return false,
                Err(std_mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    fn process_events(&mut self) -> bool {
        loop {
            let event = match self.conn.poll_for_event() {
                Ok(Some(event)) => event,
                Ok(None) => return true,
                Err(error) => {
                    self.bump_all_errors();
                    tracing::debug!(error = %error, "X11 event polling stopped");
                    return false;
                }
            };
            match event {
                x11rb::protocol::Event::DamageNotify(notify) if notify.damage == self.damage => {
                    self.mark_damage();
                }
                x11rb::protocol::Event::RandrNotify(_)
                | x11rb::protocol::Event::RandrScreenChangeNotify(_) => {
                    self.reload_displays();
                }
                x11rb::protocol::Event::XfixesCursorNotify(_) if self.cursor_enabled => {
                    self.read_cursor_shape();
                }
                _ => {}
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Add {
                display,
                cfg,
                sink,
                reply,
            } => {
                let result = if let Some(state) = self.states.get_mut(&display) {
                    let (id, first) = self.hub.add(display, sink, cfg);
                    if first {
                        state.dirty = true;
                    }
                    state.fps = self.hub.effective(&display).map_or(1, |c| c.fps);
                    state.pacer = Pacer::new(state.fps);
                    Ok(id)
                } else {
                    Err("unknown X11 display".to_owned())
                };
                let _ = reply.send(result);
            }
            Command::Remove { display, id } => {
                let _ = self.hub.remove(&display, id);
            }
            Command::WatchDisplays(tx) => {
                if !tx.is_closed() {
                    self.display_watchers.push(tx);
                }
            }
            Command::WatchCursor(tx) => {
                self.cursor_enabled = true;
                let _ = xfixes::select_cursor_input(
                    &self.conn,
                    self.root,
                    xfixes::CursorNotifyMask::DISPLAY_CURSOR,
                );
                self.cursor_watchers.push(tx);
                self.read_cursor_shape();
            }
            Command::Stop => {}
        }
    }

    fn mark_damage(&mut self) {
        if damage::subtract(&self.conn, self.damage, 0_u32, self.parts).is_err() {
            self.bump_all_errors();
            self.mark_all_dirty();
            return;
        }
        let rectangles = match xfixes::fetch_region(&self.conn, self.parts)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            Some(reply) => reply.rectangles,
            None => {
                self.bump_all_errors();
                self.mark_all_dirty();
                return;
            }
        };
        for rect in rectangles {
            let x0 = i32::from(rect.x);
            let y0 = i32::from(rect.y);
            let x1 = x0.saturating_add(i32::from(rect.width));
            let y1 = y0.saturating_add(i32::from(rect.height));
            for state in self.states.values_mut() {
                let mx0 = state.info.x;
                let my0 = state.info.y;
                let mx1 = mx0.saturating_add(state.info.width_px as i32);
                let my1 = my0.saturating_add(state.info.height_px as i32);
                if x0 < mx1 && x1 > mx0 && y0 < my1 && y1 > my0 {
                    state.dirty = true;
                }
            }
        }
    }

    fn mark_all_dirty(&mut self) {
        for state in self.states.values_mut() {
            state.dirty = true;
        }
    }

    fn capture_due(&mut self, now: Duration) {
        let ids: Vec<u32> = self.hub.keys();
        for display in ids {
            let Some(config) = self.hub.effective(&display) else {
                continue;
            };
            let Some(state) = self.states.get_mut(&display) else {
                continue;
            };
            if config.fps != state.fps {
                state.fps = config.fps;
                state.pacer = Pacer::new(config.fps);
            }
            if !state.pacer.until_due(now).is_zero() {
                continue;
            }
            state.pacer.on_sent(now);
            let dirty = state.dirty;
            let width = state.info.width_px;
            let height = state.info.height_px;
            let generation = state.generation;
            let x = state.info.x;
            let y = state.info.y;
            if !dirty {
                bump_stats(&self.inner, display, |stats| stats.ticks += 1);
                bump_stats(&self.inner, display, |stats| stats.idle_ticks += 1);
                continue;
            }
            bump_stats(&self.inner, display, |stats| stats.ticks += 1);
            let image = match self.read_image(x, y, width, height) {
                Ok(image) => image,
                Err(error) => {
                    bump_stats(&self.inner, display, |stats| stats.errors += 1);
                    tracing::debug!(error = %error, "X11 GetImage failed");
                    continue;
                }
            };
            let stride = width.saturating_mul(4);
            let Some(state) = self.states.get_mut(&display) else {
                continue;
            };
            let Some(frame) = state
                .pool
                .acquire(width, height, stride, generation, |dst| {
                    let _ = copy_to_bgrx(
                        &image,
                        stride as usize,
                        SrcFormat::Bgrx,
                        width,
                        height,
                        dst,
                        stride as usize,
                    );
                })
            else {
                bump_stats(&self.inner, display, |stats| stats.pool_full += 1);
                continue;
            };
            let captured = CapturedFrame {
                display_id: display,
                width,
                height,
                capture_ts_us: unix_now_us(),
                native: frame,
            };
            let _ = self.hub.deliver(&display, &captured);
            state.dirty = false;
            bump_stats(&self.inner, display, |stats| stats.encoded += 1);
        }
    }

    fn read_image(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, PlatformError> {
        let x = i16::try_from(x).map_err(|_| PlatformError::Unsupported)?;
        let y = i16::try_from(y).map_err(|_| PlatformError::Unsupported)?;
        let width_u16 = u16::try_from(width).map_err(|_| PlatformError::Unsupported)?;
        let height_u16 = u16::try_from(height).map_err(|_| PlatformError::Unsupported)?;
        let stride = usize::try_from(width)
            .ok()
            .and_then(|w| w.checked_mul(4))
            .ok_or(PlatformError::Unsupported)?;
        let len = stride
            .checked_mul(usize::try_from(height).map_err(|_| PlatformError::Unsupported)?)
            .ok_or(PlatformError::Unsupported)?;
        if self.shm_enabled {
            if self.shm.as_ref().is_none_or(|segment| segment.len < len) {
                if let Some(old) = self.shm.take() {
                    let _ = shm::detach(&self.conn, old.seg);
                }
                self.shm = ShmSegment::new(&self.conn, len).ok();
                self.shm_enabled = self.shm.is_some();
                self.set_shm_stat();
            }
            if let Some(segment) = self.shm.as_ref() {
                let cookie = shm::get_image(
                    &self.conn,
                    self.root,
                    x,
                    y,
                    width_u16,
                    height_u16,
                    u32::MAX,
                    u8::from(xproto::ImageFormat::Z_PIXMAP),
                    segment.seg,
                    0,
                )
                .map_err(platform_error)?;
                cookie.reply().map_err(platform_error)?;
                return segment.copy_bytes(len);
            }
        }
        let reply = xproto::get_image(
            &self.conn,
            xproto::ImageFormat::Z_PIXMAP,
            self.root,
            x,
            y,
            width_u16,
            height_u16,
            u32::MAX,
        )
        .map_err(platform_error)?
        .reply()
        .map_err(platform_error)?;
        if reply.data.len() < len {
            return Err(PlatformError::Platform("short X11 image reply".to_owned()));
        }
        Ok(reply.data[..len].to_vec())
    }

    fn set_shm_stat(&self) {
        let enabled = self.shm_enabled;
        let mut stats = lock(&self.inner.stats);
        for value in stats.values_mut() {
            value.shm = enabled;
        }
    }

    fn reload_displays(&mut self) {
        let Ok(new_displays) =
            read_displays(&self.conn, self.root, &self.screen, self.randr_version)
        else {
            self.bump_all_errors();
            return;
        };
        let old_displays = lock(&self.inner.displays).clone();
        if old_displays == new_displays {
            return;
        }
        let old_by_id: HashMap<u32, DisplayInfo> = old_displays
            .into_iter()
            .map(|info| (info.id, info))
            .collect();
        for info in &new_displays {
            if let Some(old) = old_by_id.get(&info.id) {
                if old.width_px != info.width_px
                    || old.height_px != info.height_px
                    || old.x != info.x
                    || old.y != info.y
                {
                    if let Some(state) = self.states.get_mut(&info.id) {
                        state.generation = state.generation.saturating_add(1);
                        state.pool.clear();
                        state.dirty = true;
                    }
                }
            }
            if let Some(state) = self.states.get_mut(&info.id) {
                state.info = info.clone();
            } else {
                self.states.insert(info.id, DisplayState::new(info.clone()));
            }
        }
        let present: HashSet<u32> = new_displays.iter().map(|info| info.id).collect();
        self.states
            .retain(|id, _| present.contains(id) || self.hub.effective(id).is_some());
        *lock(&self.inner.displays) = new_displays.clone();
        {
            let mut stats = lock(&self.inner.stats);
            for info in &new_displays {
                stats.entry(info.id).or_insert(X11Stats {
                    shm: self.shm_enabled,
                    ..X11Stats::default()
                });
            }
        }
        self.display_watchers
            .retain(|tx| match tx.try_send(new_displays.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
        if self.shm_enabled {
            let max_len = new_displays
                .iter()
                .filter_map(|info| {
                    usize::try_from(info.width_px)
                        .ok()?
                        .checked_mul(usize::try_from(info.height_px).ok()?)
                        .and_then(|n| n.checked_mul(4))
                })
                .max()
                .unwrap_or(0);
            if self
                .shm
                .as_ref()
                .is_none_or(|segment| segment.len < max_len)
            {
                if let Some(old) = self.shm.take() {
                    let _ = shm::detach(&self.conn, old.seg);
                }
                self.shm = ShmSegment::new(&self.conn, max_len).ok();
                self.shm_enabled = self.shm.is_some();
                self.set_shm_stat();
            }
        }
    }

    fn read_cursor_shape(&mut self) {
        let Ok(reply) = xfixes::get_cursor_image(&self.conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .ok_or(())
        else {
            self.bump_all_errors();
            return;
        };
        let pixels = argb32_to_bgra(&reply.cursor_image);
        let Ok(shape) = cursor_to_shape(
            CursorPixels::Bgra,
            u32::from(reply.width),
            u32::from(reply.height),
            u32::from(reply.width).saturating_mul(4),
            &pixels,
            i32::from(reply.xhot),
            i32::from(reply.yhot),
            1.0,
            true,
        ) else {
            self.bump_all_errors();
            return;
        };
        self.hot = (reply.xhot, reply.yhot);
        if let Some(update) = self.cursor.on_shape(shape) {
            self.broadcast_cursor(update);
        }
    }

    fn query_pointer(&mut self) {
        let Ok(reply) = xproto::query_pointer(&self.conn, self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .ok_or(())
        else {
            self.bump_all_errors();
            return;
        };
        let current = (reply.root_x, reply.root_y, reply.same_screen);
        if self.pointer == Some(current) {
            return;
        }
        self.pointer = Some(current);
        let found = if reply.same_screen {
            let x = i32::from(reply.root_x);
            let y = i32::from(reply.root_y);
            self.states.values().find(|state| {
                x >= state.info.x
                    && y >= state.info.y
                    && x < state.info.x.saturating_add(state.info.width_px as i32)
                    && y < state.info.y.saturating_add(state.info.height_px as i32)
            })
        } else {
            None
        };
        let Some(state) = found else {
            return;
        };
        let report = pointer_report(
            state.info.id,
            self.pointer_seq,
            true,
            (
                i32::from(reply.root_x).saturating_sub(state.info.x),
                i32::from(reply.root_y).saturating_sub(state.info.y),
            ),
            (i32::from(self.hot.0), i32::from(self.hot.1)),
            (state.info.width_px, state.info.height_px),
        );
        self.pointer_seq = self.pointer_seq.saturating_add(1);
        for update in self.cursor.on_report(&report) {
            self.broadcast_cursor(update);
        }
    }

    fn broadcast_cursor(&mut self, update: CursorUpdate) {
        self.cursor_watchers
            .retain(|tx| match tx.try_send(update.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
    }

    fn next_wait(&self, now: Instant) -> Duration {
        let now = now.saturating_duration_since(self.started);
        let mut wait = MAX_SLEEP;
        for display in self.hub.keys() {
            if let Some(state) = self.states.get(&display) {
                wait = wait.min(state.pacer.until_due(now));
            }
        }
        if self.cursor_enabled {
            wait = wait.min(self.next_cursor.saturating_duration_since(Instant::now()));
        }
        wait
    }

    fn bump_all_errors(&self) {
        let mut stats = lock(&self.inner.stats);
        for value in stats.values_mut() {
            value.errors = value.errors.saturating_add(1);
        }
    }

    fn cleanup(&mut self) {
        let _ = damage::destroy(&self.conn, self.damage);
        let _ = xfixes::destroy_region(&self.conn, self.parts);
        if let Some(segment) = self.shm.take() {
            let _ = shm::detach(&self.conn, segment.seg);
        }
        let _ = self.conn.flush();
    }
}

struct ShmSegment {
    _fd: OwnedFd,
    ptr: usize,
    len: usize,
    seg: shm::Seg,
}

impl ShmSegment {
    fn new(conn: &RustConnection, len: usize) -> Result<Self, PlatformError> {
        if len == 0 || len > u32::MAX as usize {
            return Err(PlatformError::Unsupported);
        }
        let name = CString::new("rs-x11-shm").map_err(|_| PlatformError::Unsupported)?;
        // SAFETY: memfd_create receives a valid NUL-terminated name.
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd was returned by memfd_create and is now owned by OwnedFd.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: ftruncate operates on this owned memfd.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the memfd is `len` bytes and the mapping is shared with the X server.
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let ptr = match NonNull::new(mapped.cast::<u8>()) {
            Some(ptr) => ptr.as_ptr() as usize,
            None => {
                // SAFETY: mapped is a successful mapping of exactly `len` bytes.
                unsafe { libc::munmap(mapped, len) };
                return Err(PlatformError::Unsupported);
            }
        };
        let seg = conn.generate_id().map_err(platform_error)?;
        let send_fd = fd.try_clone().map_err(PlatformError::Io)?;
        if let Err(error) = shm::attach_fd(conn, seg, send_fd, false)
            .map_err(platform_error)
            .and_then(|cookie| cookie.check().map_err(platform_error))
        {
            // SAFETY: ptr is the successful mapping created above.
            unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
            return Err(error);
        }
        Ok(Self {
            _fd: fd,
            ptr,
            len,
            seg,
        })
    }

    fn copy_bytes(&self, len: usize) -> Result<Vec<u8>, PlatformError> {
        if len > self.len {
            return Err(PlatformError::Unsupported);
        }
        // SAFETY: ptr/len describe the live MAP_SHARED mapping owned by this segment.
        let bytes = unsafe { std::slice::from_raw_parts(self.ptr as *const u8, len) };
        Ok(bytes.to_vec())
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // SAFETY: ptr/len are exactly the mapping returned by mmap.
        unsafe {
            let _ = libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

fn read_displays(
    conn: &RustConnection,
    root: xproto::Window,
    screen: &xproto::Screen,
    randr_version: Option<randr::QueryVersionReply>,
) -> Result<Vec<DisplayInfo>, PlatformError> {
    let mut displays = Vec::new();
    if randr_version.is_some_and(|version| version.major_version > 1 || version.minor_version >= 5)
    {
        if let Some(reply) = randr::get_monitors(conn, root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            for monitor in reply.monitors {
                let name = xproto::get_atom_name(conn, monitor.name)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
                    .unwrap_or_else(|| format!("monitor-{}", monitor.name));
                displays.push(DisplayInfo {
                    id: display_id_for_name(&name),
                    name,
                    width_px: u32::from(monitor.width),
                    height_px: u32::from(monitor.height),
                    scale: 1.0,
                    x: i32::from(monitor.x),
                    y: i32::from(monitor.y),
                    refresh_hz: 60,
                    primary: monitor.primary,
                    rotation: 0,
                });
            }
        }
    }
    if displays.is_empty() {
        displays.push(DisplayInfo {
            id: display_id_for_name("screen"),
            name: "screen".to_owned(),
            width_px: u32::from(screen.width_in_pixels),
            height_px: u32::from(screen.height_in_pixels),
            scale: 1.0,
            x: 0,
            y: 0,
            refresh_hz: 60,
            primary: true,
            rotation: 0,
        });
    } else if !displays.iter().any(|info| info.primary) {
        if let Some(first) = displays.first_mut() {
            first.primary = true;
        }
    }
    Ok(displays)
}

fn bump_stats(inner: &Inner, display: u32, update: impl FnOnce(&mut X11Stats)) {
    if let Some(stats) = lock(&inner.stats).get_mut(&display) {
        update(stats);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn platform_error<E: std::fmt::Display>(error: E) -> PlatformError {
    PlatformError::Platform(format!("X11: {error}"))
}

fn unix_now_us() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}
