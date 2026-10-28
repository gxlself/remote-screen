//! X11 input injection and local activity detection.
//!
//! The X11 backend deliberately keeps one connection per object. XTest input
//! and the keyboard mapping cache are serialized because a mapping change must
//! be followed by a round trip before the newly assigned keycode is used.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::activity_model::ActivityState;
use host_core::platform::{DisplayInfo, PlatformError};
use rsp_core::hid::InputMsg;
use tokio::sync::mpsc;
use tracing::debug;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::dpms;
use x11rb::protocol::xinput;
use x11rb::protocol::xproto;
use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::input_plan::{DisplayGeom, LinuxInputOp, LinuxInputStats, LinuxPlanner, ScrollMode};
use crate::keymap::{
    button_to_x11, evdev_to_x11, BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE,
};

/// Number of keycodes retained for temporary keysym mappings.
pub const SPARE_KEYCODES: usize = 8;

const NO_SYMBOL: u32 = 0;
const SHIFT_L: u32 = 0xFFE1;

/// Counters for XTest requests and temporary key mappings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct X11InputStats {
    pub sent: u64,
    pub no_keycode: u64,
    pub remapped: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PressedKeysym {
    keycode: u8,
    shifted: bool,
}

#[derive(Debug, Clone)]
struct KeyboardMapping {
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
}

impl KeyboardMapping {
    fn row(&self, keycode: u8) -> Option<&[u32]> {
        if keycode < self.min_keycode || keycode > self.max_keycode {
            return None;
        }
        let index = usize::from(keycode - self.min_keycode);
        let start = index.checked_mul(self.keysyms_per_keycode)?;
        let end = start.checked_add(self.keysyms_per_keycode)?;
        self.keysyms.get(start..end)
    }

    fn set_two_column_row(&mut self, keycode: u8, sym: u32) {
        if keycode < self.min_keycode || keycode > self.max_keycode {
            return;
        }
        let index = usize::from(keycode - self.min_keycode);
        let Some(start) = index.checked_mul(self.keysyms_per_keycode) else {
            return;
        };
        if self.keysyms_per_keycode < 2 || start + 1 >= self.keysyms.len() {
            return;
        }
        self.keysyms[start] = sym;
        self.keysyms[start + 1] = sym;
    }

    fn find(&self, sym: u32) -> Option<(u8, bool)> {
        for keycode in self.min_keycode..=self.max_keycode {
            let Some(row) = self.row(keycode) else {
                continue;
            };
            if row.first().copied() == Some(sym) {
                return Some((keycode, false));
            }
            if row.get(1).copied() == Some(sym) {
                return Some((keycode, true));
            }
        }
        None
    }

    fn spare_keycodes(&self) -> Vec<u8> {
        let mut spare = Vec::new();
        for keycode in self.min_keycode..=self.max_keycode {
            let Some(row) = self.row(keycode) else {
                continue;
            };
            if row.iter().all(|sym| *sym == NO_SYMBOL) {
                spare.push(keycode);
            }
        }
        spare.sort_unstable_by(|left, right| right.cmp(left));
        spare.truncate(SPARE_KEYCODES);
        spare
    }
}

struct InjectorState {
    conn: RustConnection,
    root: xproto::Window,
    mapping: KeyboardMapping,
    spare_lru: VecDeque<u8>,
    remapped: HashMap<u8, u32>,
    original_mappings: HashMap<u8, Vec<u32>>,
    pressed: HashMap<u32, PressedKeysym>,
    shift_keycode: u8,
    our_shift_count: u32,
    planner: LinuxPlanner,
    started: Instant,
    stats: X11InputStats,
}

/// X11 XTest input injector.
pub struct X11Injector {
    state: Mutex<InjectorState>,
}

impl X11Injector {
    /// Connect to X11, verify XTest 2.2, and read the keyboard mapping once.
    pub fn connect(display: Option<&str>) -> Result<Self, PlatformError> {
        let (conn, screen_num) = x11rb::connect(display)
            .map_err(|error| PlatformError::Platform(format!("X11 connection failed: {error}")))?;
        let Some(screen) = conn.setup().roots.get(screen_num) else {
            return Err(PlatformError::Platform(
                "X11 screen is unavailable".to_owned(),
            ));
        };
        let root = screen.root;

        let version = xtest::get_version(&conn, 2, 2)
            .map_err(map_connection_error)?
            .reply()
            .map_err(map_reply_error)?;
        if version.major_version < 2 || (version.major_version == 2 && version.minor_version < 2) {
            return Err(PlatformError::Unsupported);
        }

        let min_keycode = conn.setup().min_keycode;
        let max_keycode = conn.setup().max_keycode;
        let mapping = read_keyboard_mapping(&conn, min_keycode, max_keycode)?;
        let shift_keycode = mapping
            .find(SHIFT_L)
            .map(|(keycode, _)| keycode)
            .unwrap_or(50);
        let spare_lru = mapping.spare_keycodes().into_iter().collect();

        Ok(Self {
            state: Mutex::new(InjectorState {
                conn,
                root,
                mapping,
                spare_lru,
                remapped: HashMap::new(),
                original_mappings: HashMap::new(),
                pressed: HashMap::new(),
                shift_keycode,
                our_shift_count: 0,
                planner: LinuxPlanner::new(ScrollMode::X11),
                started: Instant::now(),
                stats: X11InputStats::default(),
            }),
        })
    }

    /// Plan and send one input message. Requests are flushed once at the end.
    pub fn inject(&self, msg: &InputMsg, display: &DisplayInfo) {
        let mut state = lock(&self.state);
        drain_mapping_events(&mut state);
        let now_us = elapsed_us(state.started);
        let geom = DisplayGeom {
            width: f64::from(display.width_px),
            height: f64::from(display.height_px),
        };
        let ops = state.planner.plan(msg, geom, now_us);
        execute_ops(&mut state, &ops, display);
        flush(&mut state);
    }

    /// Release every key, modifier and pointer button tracked by the planner.
    pub fn release_all(&self) {
        let mut state = lock(&self.state);
        drain_mapping_events(&mut state);
        let ops = state.planner.release_all();
        execute_ops(
            &mut state,
            &ops,
            &DisplayInfo {
                id: 0,
                name: String::new(),
                width_px: 0,
                height_px: 0,
                scale: 1.0,
                x: 0,
                y: 0,
                refresh_hz: 0,
                primary: false,
                rotation: 0,
            },
        );
        flush(&mut state);
    }

    /// Turn the display on when DPMS is available and reset the screen saver.
    pub fn wake(&self) {
        let mut state = lock(&self.state);
        if let Ok(Some(_)) = state.conn.extension_information(dpms::X11_EXTENSION_NAME) {
            let error = match dpms::force_level(&state.conn, dpms::DPMSMode::ON) {
                Ok(cookie) => {
                    drop(cookie);
                    None
                }
                Err(error) => Some(error),
            };
            if let Some(error) = error {
                record_error(&mut state, &error);
            }
        }
        let error = match xproto::force_screen_saver(&state.conn, xproto::ScreenSaver::RESET) {
            Ok(cookie) => {
                drop(cookie);
                None
            }
            Err(error) => Some(error),
        };
        if let Some(error) = error {
            record_error(&mut state, &error);
        }
        flush(&mut state);
    }

    /// Return XTest and mapping counters accumulated since construction.
    pub fn stats(&self) -> X11InputStats {
        lock(&self.state).stats
    }

    /// Return planner counters accumulated since construction.
    pub fn planner_stats(&self) -> LinuxInputStats {
        lock(&self.state).planner.stats()
    }
}

impl Drop for X11Injector {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let remapped: Vec<u8> = state.remapped.keys().copied().collect();
        for keycode in remapped {
            let keysyms = state
                .original_mappings
                .get(&keycode)
                .map(Vec::as_slice)
                .unwrap_or(&[NO_SYMBOL, NO_SYMBOL]);
            let Ok(keysyms_per_keycode) = u8::try_from(keysyms.len()) else {
                continue;
            };
            if let Ok(cookie) = xproto::change_keyboard_mapping(
                &state.conn,
                1,
                keycode,
                keysyms_per_keycode,
                keysyms,
            ) {
                let _ = cookie.check();
            }
        }
        let _ = state.conn.flush();
    }
}

fn execute_ops(state: &mut InjectorState, ops: &[LinuxInputOp], display: &DisplayInfo) {
    for op in ops {
        match *op {
            LinuxInputOp::Key { code, down } => {
                if let Some(keycode) = evdev_to_x11(code) {
                    let _ = fake(
                        state,
                        if down {
                            xproto::KEY_PRESS_EVENT
                        } else {
                            xproto::KEY_RELEASE_EVENT
                        },
                        keycode,
                        0,
                        0,
                        0,
                    );
                } else {
                    state.stats.no_keycode = state.stats.no_keycode.saturating_add(1);
                }
            }
            LinuxInputOp::Keysym { sym, down } => keysym(state, sym, down),
            LinuxInputOp::MoveAbs { x, y } => {
                let root_x = (f64::from(display.x) + x).round() as i16;
                let root_y = (f64::from(display.y) + y).round() as i16;
                let _ = fake(
                    state,
                    xproto::MOTION_NOTIFY_EVENT,
                    0,
                    state.root,
                    root_x,
                    root_y,
                );
            }
            LinuxInputOp::Button { code, down } => {
                if let Some(button) = button_code_to_x11(code) {
                    let _ = fake(
                        state,
                        if down {
                            xproto::BUTTON_PRESS_EVENT
                        } else {
                            xproto::BUTTON_RELEASE_EVENT
                        },
                        button,
                        state.root,
                        0,
                        0,
                    );
                }
            }
            LinuxInputOp::Wheel { axis, steps } => {
                let button = match (axis, steps.cmp(&0)) {
                    (crate::input_plan::WheelAxis::Vertical, std::cmp::Ordering::Greater) => 5,
                    (crate::input_plan::WheelAxis::Vertical, std::cmp::Ordering::Less) => 4,
                    (crate::input_plan::WheelAxis::Horizontal, std::cmp::Ordering::Greater) => 7,
                    (crate::input_plan::WheelAxis::Horizontal, std::cmp::Ordering::Less) => 6,
                    (_, std::cmp::Ordering::Equal) => 0,
                };
                for _ in 0..steps.unsigned_abs() {
                    if button != 0 {
                        let _ = fake(state, xproto::BUTTON_PRESS_EVENT, button, state.root, 0, 0);
                        let _ = fake(
                            state,
                            xproto::BUTTON_RELEASE_EVENT,
                            button,
                            state.root,
                            0,
                            0,
                        );
                    }
                }
            }
            LinuxInputOp::Axis { .. } | LinuxInputOp::AxisStop => {}
        }
    }
}

fn keysym(state: &mut InjectorState, sym: u32, down: bool) {
    if down {
        let Some((keycode, shifted)) = resolve_keysym(state, sym) else {
            state.stats.no_keycode = state.stats.no_keycode.saturating_add(1);
            return;
        };
        if shifted && state.our_shift_count == 0 {
            let _ = fake(state, xproto::KEY_PRESS_EVENT, state.shift_keycode, 0, 0, 0);
        }
        if shifted {
            state.our_shift_count = state.our_shift_count.saturating_add(1);
        }
        let _ = fake(state, xproto::KEY_PRESS_EVENT, keycode, 0, 0, 0);
        state
            .pressed
            .insert(sym, PressedKeysym { keycode, shifted });
    } else if let Some(pressed) = state.pressed.remove(&sym) {
        let _ = fake(state, xproto::KEY_RELEASE_EVENT, pressed.keycode, 0, 0, 0);
        if pressed.shifted {
            state.our_shift_count = state.our_shift_count.saturating_sub(1);
            if state.our_shift_count == 0 {
                let _ = fake(
                    state,
                    xproto::KEY_RELEASE_EVENT,
                    state.shift_keycode,
                    0,
                    0,
                    0,
                );
            }
        }
    } else {
        state.stats.no_keycode = state.stats.no_keycode.saturating_add(1);
    }
}

fn resolve_keysym(state: &mut InjectorState, sym: u32) -> Option<(u8, bool)> {
    if let Some(found) = state.mapping.find(sym) {
        return Some(found);
    }

    let keycode = state.spare_lru.pop_front()?;
    if state.mapping.keysyms_per_keycode < 2 {
        state.spare_lru.push_front(keycode);
        return None;
    }
    let keysyms = [sym, sym];
    let changed = {
        let mapping_result = xproto::change_keyboard_mapping(&state.conn, 1, keycode, 2, &keysyms);
        match mapping_result {
            Ok(cookie) => cookie.check().is_ok(),
            Err(_) => false,
        }
    };
    if !changed {
        record_error(state, &());
    };
    if !changed {
        state.spare_lru.push_front(keycode);
        return None;
    }

    match state
        .conn
        .get_input_focus()
        .map_err(ReplyError::from)
        .and_then(|cookie| cookie.reply())
    {
        Ok(_) => {}
        Err(error) => {
            record_error(state, &error);
            state.spare_lru.push_front(keycode);
            return None;
        }
    }
    let original = state.mapping.row(keycode).unwrap_or_default().to_vec();
    state.original_mappings.entry(keycode).or_insert(original);
    state.mapping.set_two_column_row(keycode, sym);
    state.remapped.insert(keycode, sym);
    state.spare_lru.push_back(keycode);
    state.stats.remapped = state.stats.remapped.saturating_add(1);
    Some((keycode, false))
}

fn button_code_to_x11(code: u16) -> Option<u8> {
    match code {
        BTN_LEFT => Some(button_to_x11(rsp_core::hid::Button::Left)),
        BTN_RIGHT => Some(button_to_x11(rsp_core::hid::Button::Right)),
        BTN_MIDDLE => Some(button_to_x11(rsp_core::hid::Button::Middle)),
        BTN_SIDE => Some(button_to_x11(rsp_core::hid::Button::Back)),
        BTN_EXTRA => Some(button_to_x11(rsp_core::hid::Button::Forward)),
        _ => None,
    }
}

fn fake(
    state: &mut InjectorState,
    type_: u8,
    detail: u8,
    root: xproto::Window,
    root_x: i16,
    root_y: i16,
) -> bool {
    let error = match xtest::fake_input(&state.conn, type_, detail, 0, root, root_x, root_y, 0) {
        Ok(cookie) => {
            state.stats.sent = state.stats.sent.saturating_add(1);
            drop(cookie);
            None
        }
        Err(error) => Some(error),
    };
    if let Some(error) = error {
        record_error(state, &error);
        false
    } else {
        true
    }
}

fn flush(state: &mut InjectorState) {
    if let Err(error) = state.conn.flush() {
        record_error(state, &error);
    }
}

fn drain_mapping_events(state: &mut InjectorState) {
    loop {
        match state.conn.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::MappingNotify(_event))) => {
                match read_keyboard_mapping(
                    &state.conn,
                    state.mapping.min_keycode,
                    state.mapping.max_keycode,
                ) {
                    Ok(mapping) => state.mapping = mapping,
                    Err(error) => record_error(state, &error),
                }
            }
            Ok(Some(x11rb::protocol::Event::Error(error))) => record_error(state, &error),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                record_error(state, &error);
                break;
            }
        }
    }
}

fn read_keyboard_mapping(
    conn: &RustConnection,
    min_keycode: u8,
    max_keycode: u8,
) -> Result<KeyboardMapping, PlatformError> {
    let count = max_keycode
        .checked_sub(min_keycode)
        .and_then(|difference| difference.checked_add(1))
        .ok_or_else(|| PlatformError::Platform("invalid X11 keycode range".to_owned()))?;
    let reply = xproto::get_keyboard_mapping(conn, min_keycode, count)
        .map_err(map_connection_error)?
        .reply()
        .map_err(map_reply_error)?;
    Ok(KeyboardMapping {
        min_keycode,
        max_keycode,
        keysyms_per_keycode: usize::from(reply.keysyms_per_keycode),
        keysyms: reply.keysyms,
    })
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn record_error(state: &mut InjectorState, _error: &impl std::fmt::Debug) {
    state.stats.errors = state.stats.errors.saturating_add(1);
    debug!("X11 input request failed");
}

fn map_connection_error(error: ConnectionError) -> PlatformError {
    if matches!(error, ConnectionError::UnsupportedExtension) {
        PlatformError::Unsupported
    } else {
        PlatformError::Platform(format!("X11 request failed: {error}"))
    }
}

fn map_reply_error(error: ReplyError) -> PlatformError {
    if matches!(
        error,
        ReplyError::ConnectionError(ConnectionError::UnsupportedExtension)
    ) {
        PlatformError::Unsupported
    } else {
        PlatformError::Platform(format!("X11 reply failed: {error}"))
    }
}

/// Counters for raw events observed by the XI2 activity listener.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct X11ActivityStats {
    pub ours: u64,
    pub local: u64,
}

/// XI2 raw local activity listener.
pub struct X11Activity {
    stats: Arc<Mutex<X11ActivityStats>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl X11Activity {
    /// Start listening to raw keyboard, pointer and hierarchy events.
    pub fn start(display: Option<&str>, tx: mpsc::Sender<bool>) -> Result<Self, PlatformError> {
        let (conn, screen_num) = x11rb::connect(display)
            .map_err(|error| PlatformError::Platform(format!("X11 connection failed: {error}")))?;
        let Some(screen) = conn.setup().roots.get(screen_num) else {
            return Err(PlatformError::Platform(
                "X11 screen is unavailable".to_owned(),
            ));
        };
        let root = screen.root;
        let version = xinput::xi_query_version(&conn, 2, 2)
            .map_err(map_connection_error)?
            .reply()
            .map_err(map_reply_error)?;
        if version.major_version < 2 || (version.major_version == 2 && version.minor_version < 2) {
            return Err(PlatformError::Unsupported);
        }

        let mask = xinput::XIEventMask::RAW_KEY_PRESS
            | xinput::XIEventMask::RAW_KEY_RELEASE
            | xinput::XIEventMask::RAW_BUTTON_PRESS
            | xinput::XIEventMask::RAW_BUTTON_RELEASE
            | xinput::XIEventMask::RAW_MOTION
            | xinput::XIEventMask::HIERARCHY;
        let masks = [xinput::EventMask {
            deviceid: xinput::Device::ALL.into(),
            mask: vec![mask],
        }];
        xinput::xi_select_events(&conn, root, &masks)
            .map_err(map_connection_error)?
            .check()
            .map_err(map_reply_error)?;
        let ours = query_ours(&conn)?;
        conn.flush().map_err(map_connection_error)?;

        let stats = Arc::new(Mutex::new(X11ActivityStats::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_stats = Arc::clone(&stats);
        let thread = thread::Builder::new()
            .name("rs-x11-act".to_owned())
            .spawn(move || activity_thread(conn, ours, thread_stats, thread_stop, tx))
            .map_err(|error| PlatformError::Platform(format!("X11 activity thread: {error}")))?;

        Ok(Self {
            stats,
            stop,
            thread: Some(thread),
        })
    }

    /// Return raw-event counters accumulated by the listener.
    pub fn stats(&self) -> X11ActivityStats {
        *lock(&self.stats)
    }
}

impl Drop for X11Activity {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn query_ours(conn: &RustConnection) -> Result<HashSet<u16>, PlatformError> {
    let reply = xinput::xi_query_device(conn, xinput::Device::ALL)
        .map_err(map_connection_error)?
        .reply()
        .map_err(map_reply_error)?;
    let mut ours = HashSet::new();
    for info in reply.infos {
        if info.name.as_slice() == b"Virtual core XTEST pointer"
            || info.name.as_slice() == b"Virtual core XTEST keyboard"
        {
            ours.insert(info.deviceid);
        }
    }
    Ok(ours)
}

fn activity_thread(
    conn: RustConnection,
    mut ours: HashSet<u16>,
    stats: Arc<Mutex<X11ActivityStats>>,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<bool>,
) {
    let started = Instant::now();
    let mut state = ActivityState::default();
    let mut next_tick = Duration::from_millis(500);
    while !stop.load(Ordering::Acquire) {
        loop {
            let event = match conn.poll_for_event() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => {
                    debug!("X11 activity event polling failed: {error}");
                    return;
                }
            };
            let now = started.elapsed();
            match event {
                x11rb::protocol::Event::XinputHierarchy(_) => match query_ours(&conn) {
                    Ok(updated) => ours = updated,
                    Err(error) => debug!("X11 activity device query failed: {error}"),
                },
                x11rb::protocol::Event::XinputRawKeyPress(event)
                | x11rb::protocol::Event::XinputRawKeyRelease(event) => {
                    raw_activity(event.sourceid, &ours, &stats, &mut state, now, &tx);
                }
                x11rb::protocol::Event::XinputRawButtonPress(event)
                | x11rb::protocol::Event::XinputRawButtonRelease(event)
                | x11rb::protocol::Event::XinputRawMotion(event) => {
                    raw_activity(event.sourceid, &ours, &stats, &mut state, now, &tx);
                }
                _ => {}
            }
        }

        let now = started.elapsed();
        if now >= next_tick {
            if let Some(marker) = state.on_tick(now) {
                let _ = tx.try_send(marker);
            }
            next_tick = now.saturating_add(Duration::from_millis(500));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn raw_activity(
    sourceid: u16,
    ours: &HashSet<u16>,
    stats: &Mutex<X11ActivityStats>,
    state: &mut ActivityState,
    now: Duration,
    tx: &mpsc::Sender<bool>,
) {
    let is_ours = ours.contains(&sourceid);
    {
        let mut counters = lock(stats);
        if is_ours {
            counters.ours = counters.ours.saturating_add(1);
        } else {
            counters.local = counters.local.saturating_add(1);
        }
    }
    if !is_ours {
        if let Some(marker) = state.on_event(now) {
            let _ = tx.try_send(marker);
        }
    }
}
