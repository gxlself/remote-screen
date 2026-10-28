//! Portable decisions for desktop duplication; all clocks are supplied by callers.

use std::{collections::HashSet, sync::Arc, time::Duration};

use crate::platform::CursorUpdate;
use rsp_core::pb;

use crate::{cursor_shape::PointerKind, display_model::display_id};

pub const ACQUIRE_TIMEOUT: Duration = Duration::from_millis(50);
pub const DESKTOP_POLL: Duration = Duration::from_millis(100);
pub const START_WAIT: Duration = Duration::from_secs(1);
pub const POOL_MAX: usize = 16;
pub const BACKOFF_MIN: Duration = Duration::from_millis(50);
pub const BACKOFF_MAX: Duration = Duration::from_secs(1);
pub const OUTPUT_GONE_GRACE: Duration = Duration::from_secs(3);
pub const MAX_HARD_FAILS: u32 = 10;
pub const MAX_DEVICE_FAILS: u32 = 5;
pub const FULL_RETRY: Duration = Duration::from_millis(5);
pub const KNOWN_SHAPES_MAX: usize = 256;
pub const MIN_FPS: u32 = 1;
pub const MAX_FPS: u32 = 120;

pub const DXGI_ERROR_WAIT_TIMEOUT: i32 = 0x887A0027_u32 as i32;
pub const DXGI_ERROR_ACCESS_LOST: i32 = 0x887A0026_u32 as i32;
pub const DXGI_ERROR_ACCESS_DENIED: i32 = 0x887A002B_u32 as i32;
pub const E_ACCESSDENIED: i32 = 0x80070005_u32 as i32;
pub const DXGI_ERROR_DEVICE_REMOVED: i32 = 0x887A0005_u32 as i32;
pub const DXGI_ERROR_DEVICE_HUNG: i32 = 0x887A0006_u32 as i32;
pub const DXGI_ERROR_DEVICE_RESET: i32 = 0x887A0007_u32 as i32;
pub const DXGI_ERROR_DRIVER_INTERNAL_ERROR: i32 = 0x887A0020_u32 as i32;
pub const DXGI_ERROR_INVALID_CALL: i32 = 0x887A0001_u32 as i32;
pub const E_INVALIDARG: i32 = 0x80070057_u32 as i32;
pub const DXGI_ERROR_NOT_CURRENTLY_AVAILABLE: i32 = 0x887A0022_u32 as i32;
pub const DXGI_ERROR_SESSION_DISCONNECTED: i32 = 0x887A0028_u32 as i32;
pub const DXGI_ERROR_UNSUPPORTED: i32 = 0x887A0004_u32 as i32;
pub const DXGI_ERROR_NOT_FOUND: i32 = 0x887A0002_u32 as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrClass {
    Ok,
    Timeout,
    AccessLost,
    AccessDenied,
    DeviceLost,
    InvalidCall,
    InvalidArg,
    NotAvailable,
    SessionDisconnected,
    Unsupported,
    NotFound,
    Other,
}

pub fn classify_hr(hr: i32) -> HrClass {
    match hr {
        0.. => HrClass::Ok,
        DXGI_ERROR_WAIT_TIMEOUT => HrClass::Timeout,
        DXGI_ERROR_ACCESS_LOST => HrClass::AccessLost,
        DXGI_ERROR_ACCESS_DENIED | E_ACCESSDENIED => HrClass::AccessDenied,
        DXGI_ERROR_DEVICE_REMOVED
        | DXGI_ERROR_DEVICE_HUNG
        | DXGI_ERROR_DEVICE_RESET
        | DXGI_ERROR_DRIVER_INTERNAL_ERROR => HrClass::DeviceLost,
        DXGI_ERROR_INVALID_CALL => HrClass::InvalidCall,
        E_INVALIDARG => HrClass::InvalidArg,
        DXGI_ERROR_NOT_CURRENTLY_AVAILABLE => HrClass::NotAvailable,
        DXGI_ERROR_SESSION_DISCONNECTED => HrClass::SessionDisconnected,
        DXGI_ERROR_UNSUPPORTED => HrClass::Unsupported,
        DXGI_ERROR_NOT_FOUND => HrClass::NotFound,
        _ => HrClass::Other,
    }
}

pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        Duration::ZERO
    } else {
        BACKOFF_MIN
            .saturating_mul(1 << (failures - 1).min(5))
            .min(BACKOFF_MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    CreateDevice,
    CreateDuplication,
    Acquire,
    Wait(Duration),
    GiveUp,
}

#[derive(Debug, Clone, Copy)]
enum Need {
    Device,
    Duplication,
    None,
}

pub struct Recovery {
    need: Need,
    failures: u32,
    hard_fails: u32,
    device_fails: u32,
    retry_at: Option<Duration>,
    missing_since: Option<Duration>,
    gave_up: bool,
}

impl Default for Recovery {
    fn default() -> Self {
        Self::new()
    }
}
impl Recovery {
    pub fn new() -> Self {
        Self {
            need: Need::Device,
            failures: 0,
            hard_fails: 0,
            device_fails: 0,
            retry_at: None,
            missing_since: None,
            gave_up: false,
        }
    }
    pub fn step(&self, now: Duration) -> Step {
        if self.gave_up {
            return Step::GiveUp;
        }
        if let Some(at) = self.retry_at.filter(|at| *at > now) {
            return Step::Wait(at - now);
        }
        match self.need {
            Need::Device => Step::CreateDevice,
            Need::Duplication => Step::CreateDuplication,
            Need::None => Step::Acquire,
        }
    }
    fn failed(&mut self, now: Duration) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now.saturating_add(backoff(self.failures)));
    }
    pub fn device_result(&mut self, now: Duration, r: Result<(), HrClass>) {
        self.need = Need::Device;
        match r {
            Ok(()) => {
                self.need = Need::Duplication;
                self.device_fails = 0;
                self.missing_since = None;
                self.retry_at = None;
            }
            Err(HrClass::NotFound) => {
                let since = *self.missing_since.get_or_insert(now);
                if now.saturating_sub(since) >= OUTPUT_GONE_GRACE {
                    self.gave_up = true;
                } else {
                    self.failed(now);
                }
            }
            Err(_) => {
                self.device_fails = self.device_fails.saturating_add(1);
                if self.device_fails >= MAX_DEVICE_FAILS {
                    self.gave_up = true;
                } else {
                    self.failed(now);
                }
            }
        }
    }
    pub fn duplication_result(&mut self, now: Duration, r: Result<(), HrClass>) {
        self.need = Need::Duplication;
        match r {
            Ok(()) => {
                self.need = Need::None;
                self.failures = 0;
                self.hard_fails = 0;
                self.retry_at = None;
            }
            Err(class) => {
                match class {
                    HrClass::DeviceLost | HrClass::NotFound => self.need = Need::Device,
                    HrClass::Unsupported | HrClass::InvalidArg | HrClass::Other => {
                        self.hard_fails = self.hard_fails.saturating_add(1);
                        if self.hard_fails >= MAX_HARD_FAILS {
                            self.gave_up = true;
                            return;
                        }
                    }
                    _ => {}
                }
                self.failed(now);
            }
        }
    }
    pub fn acquire_result(&mut self, now: Duration, class: HrClass) {
        match class {
            HrClass::Ok | HrClass::Timeout => {}
            HrClass::AccessLost | HrClass::AccessDenied | HrClass::InvalidCall => {
                self.need = Need::Duplication;
                self.retry_at = None;
            }
            HrClass::DeviceLost | HrClass::NotFound => {
                self.need = Need::Device;
                self.retry_at = None;
            }
            _ => {
                self.need = Need::Duplication;
                self.failed(now);
            }
        }
    }
    pub fn desktop_switched(&mut self) {
        if matches!(self.need, Need::None) {
            self.need = Need::Duplication;
            self.retry_at = None;
        }
    }
    pub fn desktop_attach_failed(&mut self, now: Duration) {
        self.need = Need::Device;
        self.failed(now);
    }
    pub fn failures(&self) -> u32 {
        self.failures
    }
}

pub struct Pacer {
    interval: Duration,
    next_due: Option<Duration>,
}
impl Pacer {
    pub fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_nanos(1_000_000_000 / u64::from(fps.clamp(MIN_FPS, MAX_FPS))),
            next_due: None,
        }
    }
    pub fn interval(&self) -> Duration {
        self.interval
    }
    pub fn until_due(&self, now: Duration) -> Duration {
        self.next_due
            .map_or(Duration::ZERO, |d| d.saturating_sub(now))
    }
    pub fn on_sent(&mut self, now: Duration) {
        self.next_due = Some(match self.next_due {
            Some(d) if now < d.saturating_add(self.interval) => d.saturating_add(self.interval),
            _ => now.saturating_add(self.interval),
        });
    }
    pub fn on_full(&mut self, now: Duration) {
        self.next_due = Some(now.saturating_add(FULL_RETRY.min(self.interval)));
    }
    pub fn reset(&mut self) {
        self.next_due = None;
    }
}

pub enum SendResult<T> {
    Sent,
    Full(T),
    Closed,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PollReport {
    pub sent: u32,
    pub full: u32,
    pub closed: u32,
}
struct Subscriber<S> {
    id: u64,
    sink: S,
    pacer: Pacer,
    sent_seq: u64,
}
pub struct Fanout<T, S> {
    latest: Option<(u64, T)>,
    next_seq: u64,
    subs: Vec<Subscriber<S>>,
}
impl<T: Clone, S> Default for Fanout<T, S> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Clone, S> Fanout<T, S> {
    pub fn new() -> Self {
        Self {
            latest: None,
            next_seq: 1,
            subs: Vec::new(),
        }
    }
    pub fn add(&mut self, id: u64, fps: u32, sink: S) {
        self.subs.push(Subscriber {
            id,
            sink,
            pacer: Pacer::new(fps),
            sent_seq: 0,
        });
    }
    pub fn remove(&mut self, id: u64) -> Option<S> {
        self.subs
            .iter()
            .position(|s| s.id == id)
            .map(|i| self.subs.remove(i).sink)
    }
    pub fn len(&self) -> usize {
        self.subs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }
    pub fn publish(&mut self, item: T) {
        // Keep ordering valid even if a process ever exhausts the sequence space.
        if self.next_seq == u64::MAX {
            self.next_seq = 1;
            for sub in &mut self.subs {
                sub.sent_seq = 0;
            }
        }
        self.latest = Some((self.next_seq, item));
        self.next_seq += 1;
    }
    pub fn clear(&mut self) {
        self.latest = None;
        for sub in &mut self.subs {
            sub.pacer.reset();
        }
    }
    pub fn drain(&mut self) -> Vec<S> {
        self.clear();
        self.subs.drain(..).map(|s| s.sink).collect()
    }
    pub fn wait_hint(&self, now: Duration, max: Duration) -> Duration {
        let Some((seq, _)) = self.latest.as_ref() else {
            return max;
        };
        self.subs
            .iter()
            .filter(|s| s.sent_seq < *seq)
            .map(|s| s.pacer.until_due(now))
            .fold(max, Duration::min)
    }
    pub fn poll(
        &mut self,
        now: Duration,
        mut send: impl FnMut(&S, T) -> SendResult<T>,
    ) -> PollReport {
        let mut report = PollReport::default();
        let Some((seq, item)) = &self.latest else {
            return report;
        };
        self.subs.retain_mut(|s| {
            if s.sent_seq < *seq && s.pacer.until_due(now).is_zero() {
                match send(&s.sink, item.clone()) {
                    SendResult::Sent => {
                        s.sent_seq = *seq;
                        s.pacer.on_sent(now);
                        report.sent += 1;
                    }
                    SendResult::Full(_) => {
                        s.pacer.on_full(now);
                        report.full += 1;
                    }
                    SendResult::Closed => {
                        report.closed += 1;
                        return false;
                    }
                }
            }
            true
        });
        report
    }
}

pub struct FramePool<T> {
    slots: Vec<Arc<T>>,
    max: usize,
}
impl<T> FramePool<T> {
    pub fn new(max: usize) -> Self {
        Self {
            slots: Vec::new(),
            max,
        }
    }
    pub fn checkout<E>(
        &mut self,
        alloc: impl FnOnce() -> Result<T, E>,
    ) -> Result<Option<Arc<T>>, E> {
        if let Some(slot) = self.slots.iter().find(|s| Arc::strong_count(s) == 1) {
            return Ok(Some(Arc::clone(slot)));
        }
        if self.slots.len() >= self.max {
            return Ok(None);
        }
        let slot = Arc::new(alloc()?);
        self.slots.push(Arc::clone(&slot));
        Ok(Some(slot))
    }
    pub fn clear(&mut self) {
        self.slots.clear();
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerReport {
    pub display_id: u32,
    pub update_time: i64,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
pub fn pointer_kind(shape_type: u32) -> Option<PointerKind> {
    match shape_type {
        1 => Some(PointerKind::Monochrome),
        2 => Some(PointerKind::Color),
        4 => Some(PointerKind::MaskedColor),
        _ => None,
    }
}
#[derive(Default)]
pub struct CursorModel {
    owner: Option<u32>,
    owner_time: i64,
    owner_visible: bool,
    hot: (u32, u32),
    current: Option<pb::CursorShape>,
    known: HashSet<u64>,
    last_pos: Option<(u32, f32, f32, bool)>,
    hidden: Option<bool>,
    seq: u32,
}
impl CursorModel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn on_shape(&mut self, shape: pb::CursorShape) -> Option<CursorUpdate> {
        self.hot = (shape.hot_x, shape.hot_y);
        let update = if self.current.as_ref().is_some_and(|s| s.id == shape.id) {
            None
        } else if self.known.contains(&shape.id) {
            Some(CursorUpdate::Use(shape.id))
        } else {
            if self.known.len() >= KNOWN_SHAPES_MAX {
                self.known.clear();
            }
            self.known.insert(shape.id);
            Some(CursorUpdate::Shape(shape.clone()))
        };
        self.current = Some(shape);
        update
    }
    fn position(&mut self, (display_id, x, y, visible): (u32, f32, f32, bool)) -> CursorUpdate {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        CursorUpdate::Position {
            display_id,
            seq,
            x,
            y,
            visible,
        }
    }
    pub fn on_report(&mut self, r: &PointerReport) -> Vec<CursorUpdate> {
        if r.update_time == 0
            || (!r.visible && self.owner != Some(r.display_id))
            || (r.visible
                && self.owner != Some(r.display_id)
                && self.owner_visible
                && self.owner_time > r.update_time)
        {
            return Vec::new();
        }
        self.owner = Some(r.display_id);
        self.owner_time = r.update_time;
        self.owner_visible = r.visible;
        let norm = |v: i32, hot: u32, size: u32| {
            if size == 0 {
                0.5
            } else {
                ((i64::from(v) + i64::from(hot)) as f64 / f64::from(size)).clamp(0.0, 1.0) as f32
            }
        };
        let pos = (
            r.display_id,
            norm(r.x, self.hot.0, r.width),
            norm(r.y, self.hot.1, r.height),
            r.visible,
        );
        let mut updates = Vec::new();
        if self.last_pos != Some(pos) {
            updates.push(self.position(pos));
            self.last_pos = Some(pos);
        }
        if self.hidden != Some(!r.visible) {
            updates.push(CursorUpdate::Hidden(!r.visible));
            self.hidden = Some(!r.visible);
        }
        updates
    }
    pub fn forget_shape(&mut self, id: u64) {
        self.known.remove(&id);
        if self.current.as_ref().is_some_and(|s| s.id == id) {
            self.current = None;
        }
    }
    pub fn reset_shapes(&mut self) {
        self.known.clear();
        if let Some(s) = &self.current {
            self.known.insert(s.id);
        }
    }
    pub fn snapshot(&mut self) -> Vec<CursorUpdate> {
        let mut updates = Vec::new();
        if let Some(s) = &self.current {
            updates.push(CursorUpdate::Shape(s.clone()));
        }
        if let Some(pos) = self.last_pos {
            updates.push(self.position(pos));
        }
        if self.hidden == Some(true) {
            updates.push(CursorUpdate::Hidden(true));
        }
        updates
    }
    pub fn display_stopped(&mut self, display_id: u32) {
        if self.owner == Some(display_id) {
            self.owner = None;
            self.owner_visible = false;
        }
    }
}

pub fn device_name_from_wide(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|v| *v == 0).unwrap_or(buf.len())])
}
pub fn find_output(outputs: &[(String, bool)], display: u32) -> Option<usize> {
    outputs
        .iter()
        .position(|(name, attached)| *attached && display_id(name) == display)
}
pub fn dxgi_rotation_degrees(rotation: i32) -> u32 {
    match rotation {
        2 => 90,
        3 => 180,
        4 => 270,
        _ => 0,
    }
}
pub fn present_to_unix_us(
    last_present_qpc: i64,
    qpc_now: i64,
    qpc_freq: i64,
    unix_now_us: u64,
) -> u64 {
    if last_present_qpc <= 0 || qpc_freq <= 0 || last_present_qpc > qpc_now {
        return unix_now_us;
    }
    let elapsed = (qpc_now - last_present_qpc) as u128 * 1_000_000 / qpc_freq as u128;
    unix_now_us.saturating_sub(elapsed.min(u128::from(u64::MAX)) as u64)
}
pub fn bgra_to_rgba(src: &[u8], width: u32, height: u32, row_pitch: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return Some(Vec::new());
    }
    let row = usize::try_from(width).ok()?.checked_mul(4)?;
    let height = usize::try_from(height).ok()?;
    let required = (height - 1).checked_mul(row_pitch)?.checked_add(row)?;
    if row_pitch < row || src.len() < required {
        return None;
    }
    let mut rgba = Vec::with_capacity(row.checked_mul(height)?);
    for y in 0..height {
        for p in src[y * row_pitch..y * row_pitch + row].chunks_exact(4) {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Some(rgba)
}
pub struct Periodic {
    period: Duration,
    last: Option<Duration>,
}
impl Periodic {
    pub fn new(period: Duration) -> Self {
        Self { period, last: None }
    }
    pub fn due(&mut self, now: Duration) -> bool {
        if self
            .last
            .is_none_or(|last| now.saturating_sub(last) >= self.period)
        {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureStats {
    pub acquired: u64,
    pub published: u64,
    pub sent: u64,
    pub skipped: u64,
    pub sink_full: u64,
    pub pool_exhausted: u64,
    pub dup_creates: u64,
    pub device_creates: u64,
    pub desktop_switches: u64,
    pub subscribers: u32,
}
