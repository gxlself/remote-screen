//! Raw BGRA framebuffer, dirty history, monitor view and registry.
//!
//! Lock order: the tap's callback mutex precedes the frame mutex. Pixel writes
//! and copies take only the frame mutex. No consumer runs with the frame mutex
//! held, so callbacks can query the frame and defer attach/detach safely.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::CString,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use crate::rscore::ffi_session::{
    empty_event, RsEvent, RS_EV_CLOSED, RS_EV_CURSOR_POS, RS_EV_STREAM_CONFIG,
};

use super::{
    cursor::CursorState,
    driver::{FrameSink, SrcPixels},
    ffi_raw::{RsRect, RS_PIX_BGRA8, RS_PIX_RGBA8},
};

const HISTORY_LEN: usize = 16;
const MAX_SIDE: u32 = 8192;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn union(a: RsRect, b: RsRect) -> RsRect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    RsRect {
        x,
        y,
        w: (a.x + a.w).max(b.x + b.w) - x,
        h: (a.y + a.h).max(b.y + b.h) - y,
    }
}

fn intersect(a: RsRect, b: RsRect) -> Option<RsRect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.w).min(b.x + b.w);
    let bottom = (a.y + a.h).min(b.y + b.h);
    (right > x && bottom > y).then_some(RsRect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    })
}

/// Keep a view inside a non-empty desktop, at least one pixel wide and high.
fn clamp_view(view: RsRect, width: u32, height: u32) -> RsRect {
    let x = view.x.min(width - 1);
    let y = view.y.min(height - 1);
    RsRect {
        x,
        y,
        w: view.w.clamp(1, width - x),
        h: view.h.clamp(1, height - y),
    }
}

/// Native consumer. Event pointers are valid only during `event`.
pub trait RawConsumer: Send + Sync {
    fn ready(&self, seq: u64, width: u32, height: u32);
    fn event(&self, e: &RsEvent);
}

pub struct RawFrame {
    state: Mutex<FrameState>,
    tap: RawTap,
    // FrameState::input_view for pointer input, so input never waits for a
    // copy. Written under the frame mutex; nothing else is locked while held.
    input_view: Mutex<Option<(RsRect, u32, u32)>>,
}

#[derive(Default)]
struct FrameState {
    width: u32,
    height: u32,
    seq: u64,
    epoch: u32,
    pixels: Vec<u8>,
    // Keep a published image so copies between put/fill calls cannot observe
    // part of the driver's next batch. Commit publishes only the dirty bounds.
    committed: Vec<u8>,
    pending: Option<RsRect>,
    history: VecDeque<(u64, RsRect)>,
    resized: bool,
    resize_seq: u64,
    // Selected monitor in desktop pixels ; None shows the whole
    // desktop. Clamped to the desktop once it has a size. Natives see this
    // region as the framebuffer.
    view: Option<RsRect>,
}

impl FrameState {
    fn input_view(&self) -> Option<(RsRect, u32, u32)> {
        let view = self.view.filter(|_| self.width != 0)?;
        Some((view, self.width, self.height))
    }

    fn shown(&self) -> RsRect {
        self.view.unwrap_or(RsRect {
            x: 0,
            y: 0,
            w: self.width,
            h: self.height,
        })
    }

    fn clip(&self, x: u32, y: u32, w: u32, h: u32) -> Option<RsRect> {
        let w = w.min(self.width.saturating_sub(x));
        let h = h.min(self.height.saturating_sub(y));
        (w != 0 && h != 0).then_some(RsRect { x, y, w, h })
    }

    fn dirty(&mut self, rect: RsRect) {
        self.pending = Some(self.pending.map_or(rect, |old| union(old, rect)));
    }

    fn commit(&mut self) -> Option<Ready> {
        let rect = self.pending.take()?;
        self.seq = self
            .seq
            .checked_add(1)
            .expect("raw frame sequence exhausted");
        if self.resized {
            self.resize_seq = self.seq;
            self.resized = false;
        }
        self.committed.resize(self.pixels.len(), 0);
        let stride = self.width as usize * 4;
        for y in rect.y..rect.y + rect.h {
            let start = y as usize * stride + rect.x as usize * 4;
            let end = start + rect.w as usize * 4;
            self.committed[start..end].copy_from_slice(&self.pixels[start..end]);
        }
        self.history.push_back((self.seq, rect));
        if self.history.len() > HISTORY_LEN {
            self.history.pop_front();
        }
        let shown = self.shown();
        Some(Ready(self.seq, shown.w, shown.h))
    }
}

impl RawFrame {
    pub fn new() -> Arc<RawFrame> {
        Arc::new(Self {
            state: Mutex::new(FrameState::default()),
            tap: RawTap::default(),
            input_view: Mutex::new(None),
        })
    }

    pub fn seq(&self) -> u64 {
        lock(&self.state).seq
    }

    pub fn size(&self) -> (u32, u32) {
        let state = lock(&self.state);
        (state.width, state.height)
    }

    pub fn epoch(&self) -> u32 {
        lock(&self.state).epoch
    }

    /// The shown monitor in desktop pixels and the desktop size; None while
    /// the whole desktop is shown or before the first resize.
    pub fn view(&self) -> Option<(RsRect, u32, u32)> {
        *lock(&self.input_view)
    }

    /// Show one monitor (desktop pixels) or, with None, the whole desktop.
    /// A change announces the new size in STREAM_CONFIG and makes the next
    /// copy a full one. Returns whether the view changed.
    pub fn set_view(&self, view: Option<RsRect>) -> bool {
        let mut changed = false;
        self.tap.publish(|tap| {
            let mut state = lock(&self.state);
            let (width, height) = (state.width, state.height);
            let view = if width == 0 {
                view
            } else {
                view.map(|view| clamp_view(view, width, height))
            };
            if view == state.view {
                return Vec::new();
            }
            changed = true;
            state.view = view;
            *lock(&self.input_view) = state.input_view();
            if width == 0 {
                // No STREAM_CONFIG yet; the first resize announces the view.
                return Vec::new();
            }
            state.epoch = state
                .epoch
                .checked_add(1)
                .expect("raw frame epoch exhausted");
            let picture = !state.committed.is_empty();
            if picture {
                state.seq = state
                    .seq
                    .checked_add(1)
                    .expect("raw frame sequence exhausted");
                state.resize_seq = state.seq;
            }
            let shown = state.shown();
            let config = Config(shown.w, shown.h, state.epoch);
            let ready = Ready(state.seq, shown.w, shown.h);
            drop(state);
            tap.config = Some(config);
            tap.ready = (ready.0 != 0).then_some(ready);
            let mut notifications = vec![Notification::Event(config.event())];
            notifications.extend(
                tap.cursor_event(view, width, height)
                    .map(Notification::Event),
            );
            if picture {
                notifications.push(Notification::Ready(ready));
            }
            notifications
        });
        changed
    }

    /// Copy the committed changes since `since_seq`, preserving all other
    /// destination bytes, including row padding. A pending resize has no
    /// committed image in its new size until the next commit.
    pub fn copy_into(
        &self,
        since_seq: u64,
        dst: &mut [u8],
        dst_stride: usize,
        dst_w: u32,
        dst_h: u32,
        pix: u8,
    ) -> (u64, RsRect) {
        let state = lock(&self.state);
        let invalid = (0, RsRect::default());
        if state.seq == 0 || state.committed.is_empty() {
            return invalid;
        }
        let shown = state.shown();
        if (dst_w, dst_h) != (shown.w, shown.h) || !matches!(pix, RS_PIX_BGRA8 | RS_PIX_RGBA8) {
            return invalid;
        }
        let row_bytes = dst_w as usize * 4; // Frame sides are bounded by MAX_SIDE.
        let required = dst_stride
            .checked_mul(dst_h as usize - 1)
            .and_then(|bytes| bytes.checked_add(row_bytes));
        if dst_stride < row_bytes || required.is_none_or(|len| dst.len() < len) {
            return invalid;
        }
        if since_seq >= state.seq {
            return (state.seq, RsRect::default());
        }
        let changed = if since_seq == 0
            || since_seq < state.resize_seq
            || state.seq - since_seq > HISTORY_LEN as u64
        {
            shown
        } else {
            state
                .history
                .iter()
                .filter(|(seq, _)| *seq > since_seq)
                .map(|(_, rect)| *rect)
                .reduce(union)
                .unwrap_or_default()
        };
        let Some(rect) = intersect(changed, shown) else {
            return (state.seq, RsRect::default());
        };
        for y in rect.y..rect.y + rect.h {
            let src_start = (y as usize * state.width as usize + rect.x as usize) * 4;
            let dst_start = (y - shown.y) as usize * dst_stride + (rect.x - shown.x) as usize * 4;
            let len = rect.w as usize * 4;
            let src = &state.committed[src_start..src_start + len];
            let dst = &mut dst[dst_start..dst_start + len];
            dst.copy_from_slice(src);
            if pix == RS_PIX_RGBA8 {
                for pixel in dst.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
            }
        }
        (
            state.seq,
            RsRect {
                x: rect.x - shown.x,
                y: rect.y - shown.y,
                ..rect
            },
        )
    }

    pub fn attach(&self, c: Arc<dyn RawConsumer>) -> bool {
        self.tap.attach(c)
    }

    pub fn detach(&self) {
        self.tap.detach();
    }

    pub fn close(&self, reason: &str) {
        self.tap.close(reason);
    }

    pub fn is_closed(&self) -> bool {
        self.tap.closed.load(Ordering::Acquire)
    }

    pub fn is_attached(&self) -> bool {
        self.tap.attached.load(Ordering::Acquire)
    }
}

impl FrameSink for RawFrame {
    fn resize(&self, width: u32, height: u32) {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            tracing::warn!(width, height, "ignoring invalid raw frame dimensions");
            return;
        }
        self.tap.publish(|tap| {
            let mut state = lock(&self.state);
            state.width = width;
            state.height = height;
            state.view = state.view.map(|view| clamp_view(view, width, height));
            *lock(&self.input_view) = state.input_view();
            state.epoch = state
                .epoch
                .checked_add(1)
                .expect("raw frame epoch exhausted");
            state.pixels.resize(width as usize * height as usize * 4, 0);
            for pixel in state.pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0, 0, 0, 255]);
            }
            state.committed.clear();
            state.history.clear();
            state.resized = true;
            state.pending = Some(RsRect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            });
            let shown = state.shown();
            let config = Config(shown.w, shown.h, state.epoch);
            tap.config = Some(config);
            tap.ready = (state.seq != 0).then_some(Ready(state.seq, shown.w, shown.h));
            vec![Notification::Event(config.event())]
        });
    }

    fn put(&self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize, pix: SrcPixels) {
        let mut state = lock(&self.state);
        let Some(rect) = state.clip(x, y, w, h) else {
            return;
        };
        let row_bytes = (w as usize).checked_mul(4);
        let required = row_bytes.and_then(|bytes| {
            stride
                .checked_mul(h as usize - 1)
                .and_then(|offset| offset.checked_add(bytes))
        });
        if row_bytes.is_none_or(|bytes| stride < bytes)
            || required.is_none_or(|len| src.len() < len)
        {
            tracing::warn!(w, h, stride, "ignoring invalid raw frame source buffer");
            return;
        }
        for row in 0..rect.h as usize {
            let dst_start = ((rect.y as usize + row) * state.width as usize + rect.x as usize) * 4;
            let len = rect.w as usize * 4;
            let dst = &mut state.pixels[dst_start..dst_start + len];
            dst.copy_from_slice(&src[row * stride..row * stride + len]);
            if pix == SrcPixels::Rgba8 {
                for pixel in dst.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
            }
        }
        state.dirty(rect);
    }

    fn fill(&self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        let mut state = lock(&self.state);
        let Some(rect) = state.clip(x, y, w, h) else {
            return;
        };
        for y in rect.y..rect.y + rect.h {
            let start = (y as usize * state.width as usize + rect.x as usize) * 4;
            for pixel in state.pixels[start..start + rect.w as usize * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&bgra);
            }
        }
        state.dirty(rect);
    }

    fn copy_within(&self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let mut state = lock(&self.state);
        let Some(src) = state.clip(sx, sy, w, h) else {
            return;
        };
        let Some(dst) = state.clip(dx, dy, src.w, src.h) else {
            return;
        };
        let stride = state.width as usize * 4;
        for row in 0..dst.h {
            let row = if dy > sy { dst.h - 1 - row } else { row };
            let start = (sy + row) as usize * stride + sx as usize * 4;
            let target = (dy + row) as usize * stride + dx as usize * 4;
            state
                .pixels
                .copy_within(start..start + dst.w as usize * 4, target);
        }
        state.dirty(dst);
    }

    fn commit(&self) {
        self.tap.publish(|tap| {
            let Some(ready) = lock(&self.state).commit() else {
                return Vec::new();
            };
            tap.ready = Some(ready);
            vec![Notification::Ready(ready)]
        });
    }

    fn cursor_shape(&self, w: u32, h: u32, hot_x: u32, hot_y: u32, rgba: &[u8]) {
        self.tap.publish(|tap| {
            tap.cursor
                .shape(w, h, hot_x, hot_y, rgba)
                .into_iter()
                .map(Notification::Event)
                .collect()
        });
    }

    fn cursor_pos(&self, x: u32, y: u32) {
        self.tap.publish(|tap| {
            let (view, width, height) = {
                let state = lock(&self.state);
                (state.view, state.width, state.height)
            };
            tap.cursor_px = Some((x, y));
            tap.cursor_event(view, width, height)
                .into_iter()
                .map(Notification::Event)
                .collect()
        });
    }
}

#[derive(Default)]
pub struct RawRegistry {
    frames: Mutex<HashMap<u64, Arc<RawFrame>>>,
}

impl RawRegistry {
    pub fn insert(&self, id: u64, f: Arc<RawFrame>) {
        lock(&self.frames).insert(id, f);
    }

    pub fn get(&self, id: u64) -> Option<Arc<RawFrame>> {
        lock(&self.frames).get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> Option<Arc<RawFrame>> {
        lock(&self.frames).remove(&id)
    }
}

#[derive(Clone, Copy)]
struct Config(u32, u32, u32);

impl Config {
    fn event(self) -> RsEvent {
        let mut event = empty_event(RS_EV_STREAM_CONFIG);
        event.width = self.0;
        event.height = self.1;
        event.epoch = self.2;
        event.cursor_visible = 1;
        event.cursor_x = 0.5;
        event.cursor_y = 0.5;
        event.src_w = 1.0;
        event.src_h = 1.0;
        event
    }
}

#[derive(Clone, Copy)]
struct Ready(u64, u32, u32);

enum Notification {
    Event(RsEvent),
    Ready(Ready),
}

#[derive(Default)]
struct TapState {
    consumer: Option<Arc<dyn RawConsumer>>,
    config: Option<Config>,
    cursor: CursorState,
    ready: Option<Ready>,
    // Last server pointer position in desktop pixels, and whether it lies
    // outside the view (CURSOR_POS then reports -1, -1).
    cursor_px: Option<(u32, u32)>,
    cursor_outside: bool,
}

impl TapState {
    /// CURSOR_POS for the last server pointer position, relative to the view.
    fn cursor_event(&mut self, view: Option<RsRect>, width: u32, height: u32) -> Option<RsEvent> {
        let (x, y) = self.cursor_px?;
        let Some(view) = view.filter(|_| width != 0 && height != 0) else {
            self.cursor_outside = false;
            return self.cursor.pos(x, y, width, height);
        };
        let inside = x
            .checked_sub(view.x)
            .filter(|x| *x < view.w)
            .zip(y.checked_sub(view.y).filter(|y| *y < view.h));
        self.cursor_outside = inside.is_none();
        let Some((x, y)) = inside else {
            let mut event = self.cursor.pos(0, 0, 1, 1)?;
            event.cursor_x = -1.0;
            event.cursor_y = -1.0;
            return Some(event);
        };
        self.cursor.pos(x, y, view.w, view.h)
    }

    fn cursor_replay(&self) -> Vec<RsEvent> {
        let mut events = self.cursor.replay();
        for event in &mut events {
            if event.kind == RS_EV_CURSOR_POS && self.cursor_outside {
                event.cursor_x = -1.0;
                event.cursor_y = -1.0;
            }
        }
        events
    }
}

enum Change {
    Attach(Arc<dyn RawConsumer>),
    Detach,
}

#[derive(Default)]
struct Deferred {
    change: Option<Change>,
    close: Option<CString>,
}

#[derive(Default)]
struct RawTap {
    // Held across every callback, including replay and CLOSED. Detach waits here.
    state: Mutex<TapState>,
    attached: AtomicBool,
    closed: AtomicBool,
    deferred: Mutex<Deferred>,
}

thread_local! {
    // A callback on tap A must still acquire tap B's lock. A stack also handles
    // nested callbacks that reenter A through B.
    static CALLBACKS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct CallbackScope;

impl Drop for CallbackScope {
    fn drop(&mut self) {
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().pop());
    }
}

impl RawTap {
    fn in_callback(&self) -> bool {
        CALLBACKS.with(|callbacks| callbacks.borrow().contains(&(self as *const Self as usize)))
    }

    fn call(&self, f: impl FnOnce()) {
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(self as *const Self as usize));
        let _scope = CallbackScope;
        f();
    }

    fn attach(&self, consumer: Arc<dyn RawConsumer>) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        if self.in_callback() {
            lock(&self.deferred).change = Some(Change::Attach(consumer));
            return true;
        }
        let mut state = lock(&self.state);
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        state.consumer = Some(consumer);
        self.attached.store(true, Ordering::Release);
        self.replay(&mut state);
        true
    }

    fn detach(&self) {
        if self.in_callback() {
            lock(&self.deferred).change = Some(Change::Detach);
            return;
        }
        let mut state = lock(&self.state);
        state.consumer = None;
        self.attached.store(false, Ordering::Release);
    }

    fn settle(&self, state: &mut TapState) -> bool {
        let deferred = std::mem::take(&mut *lock(&self.deferred));
        if let Some(reason) = deferred.close {
            self.finish_close(state, &reason);
            return true;
        }
        match deferred.change {
            Some(Change::Attach(consumer)) => {
                state.consumer = Some(consumer);
                self.attached.store(true, Ordering::Release);
            }
            Some(Change::Detach) => {
                state.consumer = None;
                self.attached.store(false, Ordering::Release);
            }
            None => return false,
        }
        true
    }

    // Returns true if a callback changed the consumer. Remaining notifications
    // belong to the old consumer; the replacement gets the complete replay.
    fn send(&self, state: &mut TapState, notifications: Vec<Notification>) -> bool {
        for notification in notifications {
            let Some(consumer) = state.consumer.clone() else {
                break;
            };
            self.call(|| match notification {
                Notification::Event(event) => consumer.event(&event),
                Notification::Ready(Ready(seq, width, height)) => {
                    consumer.ready(seq, width, height)
                }
            });
            if self.settle(state) {
                return true;
            }
        }
        false
    }

    fn replay(&self, state: &mut TapState) {
        while state.consumer.is_some() {
            let mut notifications = Vec::new();
            if let Some(config) = state.config {
                notifications.push(Notification::Event(config.event()));
            }
            notifications.extend(state.cursor_replay().into_iter().map(Notification::Event));
            if let Some(ready) = state.ready {
                notifications.push(Notification::Ready(ready));
            }
            if !self.send(state, notifications) {
                break;
            }
        }
    }

    fn publish(&self, update: impl FnOnce(&mut TapState) -> Vec<Notification>) {
        let mut state = lock(&self.state);
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let notifications = update(&mut state);
        if self.send(&mut state, notifications) {
            self.replay(&mut state);
        }
    }

    fn close(&self, reason: &str) {
        let reason = CString::new(reason.replace('\0', "?"))
            .expect("embedded NULs were replaced in the close reason");
        if self.in_callback() {
            if !self.closed.swap(true, Ordering::AcqRel) {
                lock(&self.deferred).close = Some(reason);
            }
            return;
        }
        let mut state = lock(&self.state);
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.finish_close(&mut state, &reason);
        }
    }

    fn finish_close(&self, state: &mut TapState, reason: &CString) {
        self.attached.store(false, Ordering::Release);
        if let Some(consumer) = state.consumer.take() {
            let mut event = empty_event(RS_EV_CLOSED);
            event.message = reason.as_ptr();
            self.call(|| consumer.event(&event));
        }
        *lock(&self.deferred) = Deferred::default();
        *state = TapState::default();
    }
}
