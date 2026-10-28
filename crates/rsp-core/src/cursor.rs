//! Cursor messages, shape hashing, and the receiver-side shape cache. See
//! `docs/dev/03-protocol.md` §6.3.
//!
//! Each cursor-stream record is `u32 length + CursorMsg`; the transport frame
//! limit of 1 MiB therefore bounds each protobuf, including its PNG payload.

use std::collections::HashMap;
use std::sync::Arc;

use prost::Message;
use thiserror::Error;
use xxhash_rust::xxh3::xxh3_64;

use crate::pb::{cursor_msg, CursorMsg, CursorShape};
use crate::wire::{frame, DecodeError, FrameDecoder};

/// Maximum encoded `CursorMsg` payload, including the PNG.
pub const MAX_CURSOR_MESSAGE_LEN: usize = 1 << 20;
/// Number of cursor shapes retained by [`ShapeCache`].
pub const SHAPE_CACHE_CAPACITY: usize = 64;

/// Frames a `CursorMsg` as a cursor-stream record.
pub fn encode(msg: &CursorMsg) -> Vec<u8> {
    frame(&msg.encode_to_vec())
}

#[derive(Debug, Error)]
pub enum CursorError {
    #[error(transparent)]
    Wire(#[from] DecodeError),
    #[error("cursor protobuf is invalid: {0}")]
    Protobuf(#[from] prost::DecodeError),
    #[error("cursor message has no oneof value")]
    EmptyMessage,
}

/// Incremental decoder for a sequence of cursor-stream records.
#[derive(Default)]
pub struct CursorStreamDecoder {
    frames: FrameDecoder,
}

impl CursorStreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.frames.push(data);
    }

    /// Returns the next complete `CursorMsg`, or `None` when more bytes are
    /// needed. A zero-length or protobuf without a oneof value is rejected.
    pub fn next_msg(&mut self) -> Result<Option<CursorMsg>, CursorError> {
        let Some(payload) = self.frames.next_frame()? else {
            return Ok(None);
        };
        let msg = CursorMsg::decode(payload.as_slice())?;
        if msg.msg.is_none() {
            return Err(CursorError::EmptyMessage);
        }
        Ok(Some(msg))
    }
}

/// Returns the xxh3-64 content hash used as a cursor shape id.
pub fn shape_id(png: &[u8]) -> u64 {
    xxh3_64(png)
}

struct CacheEntry {
    shape: Arc<CursorShape>,
    stamp: u64,
}

/// Least-recently-used cache of at most 64 cursor shapes.
pub struct ShapeCache {
    capacity: usize,
    clock: u64,
    entries: HashMap<u64, CacheEntry>,
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeCache {
    pub fn new() -> Self {
        Self::with_capacity(SHAPE_CACHE_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            clock: 0,
            entries: HashMap::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Inserts or replaces a shape by its `id`, evicting the least-recently-used
    /// entry when full. Returns the cached shared shape.
    pub fn insert(&mut self, shape: CursorShape) -> Arc<CursorShape> {
        let id = shape.id;
        let shape = Arc::new(shape);
        if !self.entries.contains_key(&id) && self.entries.len() >= self.capacity {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.stamp)
                .map(|(id, _)| *id)
            {
                self.entries.remove(&oldest);
            }
        }
        let stamp = self.next_stamp();
        self.entries.insert(
            id,
            CacheEntry {
                shape: Arc::clone(&shape),
                stamp,
            },
        );
        shape
    }

    /// Looks up a shape and marks it as most recently used.
    pub fn get(&mut self, id: u64) -> Option<&CursorShape> {
        self.touch(id)?;
        self.entries.get(&id).map(|entry| entry.shape.as_ref())
    }

    fn get_shared(&mut self, id: u64) -> Option<Arc<CursorShape>> {
        self.touch(id)?;
        self.entries.get(&id).map(|entry| Arc::clone(&entry.shape))
    }

    fn touch(&mut self, id: u64) -> Option<()> {
        let stamp = self.next_stamp();
        let entry = self.entries.get_mut(&id)?;
        entry.stamp = stamp;
        Some(())
    }

    fn next_stamp(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }
}

/// Events emitted while applying cursor messages.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorEvent {
    Show(Arc<CursorShape>),
    Hide,
    Missing(u64),
}

/// Applies cursor messages while retaining visibility and the current shape.
#[derive(Default)]
pub struct CursorState {
    cache: ShapeCache,
    current: Option<Arc<CursorShape>>,
    hidden: bool,
}

impl CursorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, msg: CursorMsg) -> CursorEvent {
        match msg.msg {
            Some(cursor_msg::Msg::Shape(shape)) => {
                let shape = self.cache.insert(shape);
                self.current = Some(Arc::clone(&shape));
                self.hidden = false;
                CursorEvent::Show(shape)
            }
            Some(cursor_msg::Msg::Use(id)) => match self.cache.get_shared(id) {
                Some(shape) => {
                    self.current = Some(Arc::clone(&shape));
                    self.hidden = false;
                    CursorEvent::Show(shape)
                }
                None => CursorEvent::Missing(id),
            },
            Some(cursor_msg::Msg::Hidden(hidden)) => {
                self.hidden = hidden;
                if hidden {
                    CursorEvent::Hide
                } else if let Some(shape) = &self.current {
                    CursorEvent::Show(Arc::clone(shape))
                } else {
                    CursorEvent::Hide
                }
            }
            None => CursorEvent::Missing(0),
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    pub fn current(&self) -> Option<&CursorShape> {
        self.current.as_deref()
    }

    pub fn cache(&self) -> &ShapeCache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut ShapeCache {
        &mut self.cache
    }
}
