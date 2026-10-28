//! Cursor pump: per-connection shape messages and pointer datagrams.
//!
//! This module is deliberately independent of the transport and of time
//! sources. Callers pass Unix microseconds to the methods that need a clock
//! and are responsible for writing [`CursorOut::Stream`] records after the
//! `0x02` cursor-stream type byte, or sending [`CursorOut::Datagram`] values as
//! pointer datagrams (03 sections 2, 4.1, and 6).

use rsp_core::cursor::{encode, ShapeCache, MAX_CURSOR_MESSAGE_LEN};
use rsp_core::features::{CURSOR_SHAPE, POINTER_DGRAM};
use rsp_core::pb::{cursor_msg, CursorMsg, CursorShape};
use rsp_core::pointer::PointerDatagram;
use tracing::warn;

use crate::platform::CursorUpdate;

/// First byte written by the caller when opening a cursor stream.
pub const CURSOR_STREAM_TYPE: u8 = 0x02;
/// Minimum interval between pointer datagrams (60 Hz).
pub const POINTER_INTERVAL_US: u64 = 16_667;
/// How long a controller's injected pointer position is treated as an echo.
pub const ECHO_WINDOW_US: u64 = 150_000;
/// Normalized-coordinate tolerance for recognizing an injected pointer echo.
pub const ECHO_EPSILON: f32 = 0.01;
/// Number of cursor shapes retained on the Host side.
pub const HOST_SHAPE_CAPACITY: usize = 256;

/// One item for the caller to write to a connection.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorOut {
    /// A complete length-prefixed cursor-stream record (without `0x02`).
    Stream(Vec<u8>),
    /// An encoded 15-byte pointer datagram.
    Datagram([u8; 15]),
}

/// A de-duplicated cursor state change from the Host platform stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorChange {
    Shape(u64),
    Hidden(bool),
    Position {
        display_id: u32,
        x: f32,
        y: f32,
        visible: bool,
    },
}

/// Host-wide cursor state; one instance is shared by all connections.
pub struct CursorHub {
    shapes: ShapeCache,
    current_shape: Option<u64>,
    hidden: bool,
    position: Option<(u32, f32, f32, bool)>,
}

impl Default for CursorHub {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorHub {
    pub fn new() -> Self {
        Self {
            shapes: ShapeCache::with_capacity(HOST_SHAPE_CAPACITY),
            current_shape: None,
            hidden: false,
            position: None,
        }
    }

    /// Applies a platform update and returns it only when it changes state.
    pub fn apply(&mut self, update: CursorUpdate) -> Option<CursorChange> {
        match update {
            CursorUpdate::Shape(shape) => self.apply_shape(shape),
            CursorUpdate::Use(id) => self.apply_use(id),
            CursorUpdate::Hidden(hidden) => {
                if self.hidden == hidden {
                    None
                } else {
                    self.hidden = hidden;
                    Some(CursorChange::Hidden(hidden))
                }
            }
            CursorUpdate::Position {
                display_id,
                seq: _,
                x,
                y,
                visible,
            } => {
                let position = (display_id, x, y, visible);
                if self.position == Some(position) {
                    None
                } else {
                    self.position = Some(position);
                    Some(CursorChange::Position {
                        display_id,
                        x,
                        y,
                        visible,
                    })
                }
            }
        }
    }

    /// Returns a clone of a stored shape and refreshes its LRU position.
    pub fn shape(&mut self, id: u64) -> Option<CursorShape> {
        self.shapes.get(id).cloned()
    }

    pub fn current_shape(&self) -> Option<u64> {
        self.current_shape
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn position(&self) -> Option<(u32, f32, f32, bool)> {
        self.position
    }

    fn apply_shape(&mut self, shape: CursorShape) -> Option<CursorChange> {
        let id = shape.id;
        let width = shape.width;
        let height = shape.height;
        let msg = CursorMsg {
            msg: Some(cursor_msg::Msg::Shape(shape)),
        };
        let encoded_len = encode(&msg).len().saturating_sub(4);
        if encoded_len > MAX_CURSOR_MESSAGE_LEN {
            warn!(id, width, height, "dropping oversized cursor shape");
            return None;
        }
        let Some(cursor_msg::Msg::Shape(shape)) = msg.msg else {
            return None;
        };
        self.shapes.insert(shape);
        let changed = self.current_shape != Some(id);
        self.current_shape = Some(id);
        changed.then_some(CursorChange::Shape(id))
    }

    fn apply_use(&mut self, id: u64) -> Option<CursorChange> {
        if self.shapes.get(id).is_none() {
            warn!(id, "cursor shape is not cached");
            return None;
        }
        let changed = self.current_shape != Some(id);
        self.current_shape = Some(id);
        changed.then_some(CursorChange::Shape(id))
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingPointer {
    display: u8,
    visible: bool,
    x: f32,
    y: f32,
}

/// Per-connection cursor pump.
pub struct ConnCursor {
    client_features: u64,
    is_controller: bool,
    mirror: ShapeCache,
    stream_display: Option<(u32, u8)>,
    last_in_display: Option<(f32, f32)>,
    pointer_on_display: bool,
    pending: Option<PendingPointer>,
    last_sent_us: Option<u64>,
    next_seq: u32,
    injected: Option<(f32, f32, u64)>,
}

impl ConnCursor {
    pub fn new(client_features: u64, is_controller: bool) -> Self {
        Self {
            client_features,
            is_controller,
            mirror: ShapeCache::new(),
            stream_display: None,
            last_in_display: None,
            pointer_on_display: false,
            pending: None,
            last_sent_us: None,
            next_seq: 1,
            injected: None,
        }
    }

    pub fn set_controller(&mut self, is_controller: bool) {
        self.is_controller = is_controller;
    }

    /// Selects the display this connection is watching. `wire_id` is the
    /// eight-bit display id used by pointer datagrams and video frames.
    pub fn set_stream_display(&mut self, platform_id: u32, wire_id: u8) {
        let display = Some((platform_id, wire_id));
        if self.stream_display != display {
            self.stream_display = display;
            self.last_in_display = None;
            self.pointer_on_display = false;
            self.pending = None;
        }
    }

    /// Starts (or restarts) the cursor stream by sending a complete snapshot.
    pub fn attach(&mut self, hub: &mut CursorHub) -> Vec<CursorOut> {
        self.mirror = ShapeCache::new();
        if !self.has(CURSOR_SHAPE) {
            return Vec::new();
        }

        let mut out = Vec::new();
        if let Some(id) = hub.current_shape() {
            if let Some(shape) = hub.shape(id) {
                if let Some((record, shape)) = shape_record(shape) {
                    self.mirror.insert(shape);
                    out.push(record);
                }
            }
        }
        if hub.hidden() {
            out.push(CursorOut::Stream(cursor_record(cursor_msg::Msg::Hidden(
                true,
            ))));
        }
        out
    }

    /// Converts a Hub change into records for this connection.
    pub fn on_change(
        &mut self,
        change: &CursorChange,
        hub: &mut CursorHub,
        now_us: u64,
    ) -> Vec<CursorOut> {
        match *change {
            CursorChange::Shape(id) => self.on_shape(id, hub),
            CursorChange::Hidden(hidden) => {
                if self.has(CURSOR_SHAPE) {
                    vec![CursorOut::Stream(cursor_record(cursor_msg::Msg::Hidden(
                        hidden,
                    )))]
                } else {
                    Vec::new()
                }
            }
            CursorChange::Position {
                display_id,
                x,
                y,
                visible,
            } => self.on_position(display_id, x, y, visible, now_us),
        }
    }

    /// Records a pointer position just injected by this controller.
    pub fn note_injected(&mut self, x: f32, y: f32, now_us: u64) {
        self.injected = Some((x, y, now_us));
    }

    /// Emits a rate-limited pointer datagram when its deadline has arrived.
    pub fn poll(&mut self, now_us: u64) -> Vec<CursorOut> {
        if !self.has(POINTER_DGRAM) {
            return Vec::new();
        }
        let Some(pending) = self.pending else {
            return Vec::new();
        };
        if self
            .last_sent_us
            .is_some_and(|last| now_us < next_after(last))
        {
            return Vec::new();
        }
        self.pending = None;
        vec![self.emit_pointer(pending, now_us)]
    }

    pub fn next_deadline(&self) -> Option<u64> {
        if !self.has(POINTER_DGRAM) || self.pending.is_none() {
            return None;
        }
        self.last_sent_us.map(next_after)
    }

    /// Current cursor values for `StreamConfig`, relative to this stream.
    pub fn stream_config_cursor(&self, hub: &CursorHub) -> (f32, f32, bool) {
        let Some((platform_id, _)) = self.stream_display else {
            return (0.5, 0.5, false);
        };
        match hub.position() {
            Some((display_id, x, y, visible)) if display_id == platform_id => (x, y, visible),
            _ => (0.5, 0.5, false),
        }
    }

    /// Sets the first sequence number. Intended for wraparound tests.
    #[doc(hidden)]
    pub fn with_initial_seq(mut self, seq: u32) -> Self {
        self.next_seq = seq;
        self
    }

    fn has(&self, feature: u64) -> bool {
        self.client_features & feature != 0
    }

    fn on_shape(&mut self, id: u64, hub: &mut CursorHub) -> Vec<CursorOut> {
        if !self.has(CURSOR_SHAPE) {
            return Vec::new();
        }
        if self.mirror.get(id).is_some() {
            return vec![CursorOut::Stream(cursor_record(cursor_msg::Msg::Use(id)))];
        }
        let Some(shape) = hub.shape(id) else {
            return Vec::new();
        };
        let Some((record, shape)) = shape_record(shape) else {
            return Vec::new();
        };
        self.mirror.insert(shape);
        vec![record]
    }

    fn on_position(
        &mut self,
        display_id: u32,
        x: f32,
        y: f32,
        visible: bool,
        now_us: u64,
    ) -> Vec<CursorOut> {
        if !self.has(POINTER_DGRAM) {
            return Vec::new();
        }
        let Some((stream_display, wire_id)) = self.stream_display else {
            return Vec::new();
        };

        if display_id == stream_display {
            self.last_in_display = Some((x, y));
            self.pointer_on_display = true;
            if self.is_echo(x, y, now_us) {
                return Vec::new();
            }
            return self.queue_pointer(
                PendingPointer {
                    display: wire_id,
                    visible,
                    x,
                    y,
                },
                now_us,
            );
        }

        if !std::mem::replace(&mut self.pointer_on_display, false) {
            return Vec::new();
        }
        let (x, y) = self.last_in_display.unwrap_or((0.5, 0.5));
        self.queue_pointer(
            PendingPointer {
                display: wire_id,
                visible: false,
                x,
                y,
            },
            now_us,
        )
    }

    fn is_echo(&self, x: f32, y: f32, now_us: u64) -> bool {
        if !self.is_controller {
            return false;
        }
        let Some((injected_x, injected_y, injected_at)) = self.injected else {
            return false;
        };
        now_us.saturating_sub(injected_at) < ECHO_WINDOW_US
            && (x - injected_x).abs() < ECHO_EPSILON
            && (y - injected_y).abs() < ECHO_EPSILON
    }

    fn queue_pointer(&mut self, pending: PendingPointer, now_us: u64) -> Vec<CursorOut> {
        if self
            .last_sent_us
            .is_some_and(|last| now_us < next_after(last))
        {
            self.pending = Some(pending);
            Vec::new()
        } else {
            self.pending = None;
            vec![self.emit_pointer(pending, now_us)]
        }
    }

    fn emit_pointer(&mut self, pending: PendingPointer, now_us: u64) -> CursorOut {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.last_sent_us = Some(now_us);
        CursorOut::Datagram(
            PointerDatagram {
                seq,
                display: pending.display,
                visible: pending.visible,
                x: pending.x,
                y: pending.y,
            }
            .encode(),
        )
    }
}

fn next_after(last_sent_us: u64) -> u64 {
    last_sent_us.saturating_add(POINTER_INTERVAL_US)
}

fn cursor_record(msg: cursor_msg::Msg) -> Vec<u8> {
    encode(&CursorMsg { msg: Some(msg) })
}

fn shape_record(shape: CursorShape) -> Option<(CursorOut, CursorShape)> {
    let msg = CursorMsg {
        msg: Some(cursor_msg::Msg::Shape(shape)),
    };
    let record = encode(&msg);
    let Some(cursor_msg::Msg::Shape(shape)) = msg.msg else {
        return None;
    };
    Some((CursorOut::Stream(record), shape))
}
