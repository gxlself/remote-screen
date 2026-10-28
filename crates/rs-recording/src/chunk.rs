//! Plaintext recording chunk builder (M3 V5).

use std::collections::VecDeque;

use crate::record::{
    Body, ConfigRec, CursorShapeRec, Record, RecordingEnd, RecordingMeta, HEADER_LEN,
};

pub const CHUNK_TARGET_BYTES: usize = 1 << 20;
pub const CHUNK_MAX_SPAN_US: u64 = 1_000_000;
pub const CHUNK_MAX_BYTES: usize = 16 << 20;

const MAX_RECORD_BYTES: usize = CHUNK_MAX_BYTES / 2;

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkOut {
    pub t_start_us: u64,
    pub keyframe_start: bool,
    pub plaintext: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkStats {
    pub frames: u64,
    pub keyframes: u64,
    pub audio_packets: u64,
    pub gaps: u64,
    pub dropped: u64,
    pub plain_bytes: u64,
    pub last_t_us: u64,
    pub width: u32,
    pub height: u32,
    pub codec: u8,
}

#[derive(Clone, Copy)]
enum CursorState {
    Use(u64),
    Hidden(bool),
}

pub struct ChunkBuilder {
    meta: Option<RecordingMeta>,
    current: Option<ChunkOut>,
    config: Option<ConfigRec>,
    cursor_pos: Option<(f32, f32, bool)>,
    shapes: VecDeque<CursorShapeRec>,
    cursor_state: Option<CursorState>,
    have_keyframe: bool,
    drop_until_keyframe: bool,
    last_input_t_us: u64,
    stats: ChunkStats,
}

impl ChunkBuilder {
    pub fn new(meta: RecordingMeta) -> Self {
        Self {
            meta: Some(meta),
            current: None,
            config: None,
            cursor_pos: None,
            shapes: VecDeque::with_capacity(8),
            cursor_state: None,
            have_keyframe: false,
            drop_until_keyframe: false,
            last_input_t_us: 0,
            stats: ChunkStats::default(),
        }
    }

    /// Return 0..=2 completed chunks: the old chunk and possibly a full new
    /// keyframe chunk. Dropped records still update the state cache and clock.
    pub fn push(&mut self, mut rec: Record) -> Vec<ChunkOut> {
        rec.t_us = rec.t_us.max(self.last_input_t_us);
        self.last_input_t_us = rec.t_us;
        self.update_state(&rec.body);

        let is_video = matches!(&rec.body, Body::Video(_));
        let is_keyframe = matches!(&rec.body, Body::Video(video) if video.keyframe);
        let encoded_len = rec.encoded_len();
        if encoded_len > MAX_RECORD_BYTES {
            self.stats.dropped += 1;
            if is_keyframe {
                self.drop_until_keyframe = true;
            }
            return Vec::new();
        }
        if !is_keyframe && (!self.have_keyframe || (is_video && self.drop_until_keyframe)) {
            if is_video {
                self.stats.dropped += 1;
            }
            return Vec::new();
        }

        let mut completed = Vec::new();
        if is_keyframe {
            let prefix = self.state_prefix(rec.t_us);
            // The prelude and keyframe must stay together for seeking. Even
            // individually valid records may exceed the chunk cap in total.
            let prefix_len = prefix
                .iter()
                .fold(0usize, |len, state| len.saturating_add(state.encoded_len()));
            if prefix
                .iter()
                .any(|state| state.encoded_len() > MAX_RECORD_BYTES)
                || prefix_len.saturating_add(encoded_len) > CHUNK_MAX_BYTES
            {
                self.stats.dropped += 1;
                self.drop_until_keyframe = true;
                return completed;
            }
            if let Some(chunk) = self.take_current() {
                completed.push(chunk);
            }
            self.current = Some(ChunkOut {
                t_start_us: rec.t_us,
                keyframe_start: true,
                plaintext: Vec::new(),
            });
            for state in prefix {
                self.append_record(&state);
            }
            self.meta = None;
            self.have_keyframe = true;
            self.drop_until_keyframe = false;
        }

        self.append_record(&rec);
        let full = self.current.as_ref().is_some_and(|chunk| {
            chunk.plaintext.len() >= CHUNK_TARGET_BYTES
                || rec.t_us.saturating_sub(chunk.t_start_us) >= CHUNK_MAX_SPAN_US
        });
        if full {
            if let Some(chunk) = self.take_current() {
                completed.push(chunk);
            }
        }
        completed
    }

    pub fn stats(&self) -> &ChunkStats {
        &self.stats
    }

    /// Flush the current data chunk and return a FINAL chunk containing End.
    pub fn finish(mut self, mut end: RecordingEnd) -> (Option<ChunkOut>, ChunkOut) {
        let pending = self.take_current();
        end.duration_us = self.stats.last_t_us;
        end.frames = self.stats.frames;
        end.keyframes = self.stats.keyframes;
        end.audio_packets = self.stats.audio_packets;
        end.gaps = self.stats.gaps;
        end.dropped = self.stats.dropped;
        end.plain_bytes = self.stats.plain_bytes;
        end.width = self.stats.width;
        end.height = self.stats.height;
        end.codec = self.stats.codec;

        let t_us = self.stats.last_t_us;
        let end_record = Record {
            t_us,
            body: Body::End(end),
        };
        let mut plaintext = Vec::with_capacity(end_record.encoded_len());
        end_record.encode_into(&mut plaintext);
        let final_chunk = ChunkOut {
            t_start_us: t_us,
            keyframe_start: false,
            plaintext,
        };
        (pending, final_chunk)
    }

    fn update_state(&mut self, body: &Body) {
        match body {
            Body::Config(config) => {
                self.stats.width = config.width;
                self.stats.height = config.height;
                self.stats.codec = config.codec;
                self.config = Some(config.clone());
            }
            Body::CursorPos { x, y, visible } => {
                self.cursor_pos = Some((*x, *y, *visible));
            }
            Body::CursorShape(shape) => {
                if let Some(index) = self.shapes.iter().position(|old| old.id == shape.id) {
                    self.shapes.remove(index);
                }
                self.shapes.push_back(shape.clone());
                while self.shapes.len() > 8 {
                    self.shapes.pop_front();
                }
                self.cursor_state = Some(CursorState::Use(shape.id));
            }
            Body::CursorUse(id) => self.cursor_state = Some(CursorState::Use(*id)),
            Body::CursorHidden(hidden) => self.cursor_state = Some(CursorState::Hidden(*hidden)),
            _ => {}
        }
    }

    fn state_prefix(&self, t_us: u64) -> Vec<Record> {
        let mut prefix = Vec::with_capacity(5);
        if let Some(meta) = &self.meta {
            prefix.push(Record {
                t_us: 0,
                body: Body::Meta(meta.clone()),
            });
        }
        if let Some(config) = &self.config {
            prefix.push(Record {
                t_us,
                body: Body::Config(config.clone()),
            });
        }
        if let Some((x, y, visible)) = self.cursor_pos {
            prefix.push(Record {
                t_us,
                body: Body::CursorPos { x, y, visible },
            });
        }
        match self.cursor_state {
            Some(CursorState::Use(id)) => {
                if let Some(shape) = self.shapes.iter().find(|shape| shape.id == id) {
                    // Oversized shapes are cached, but must not re-enter the
                    // output through a keyframe prelude after being dropped.
                    if shape.png.len() <= MAX_RECORD_BYTES - HEADER_LEN - 28 {
                        prefix.push(Record {
                            t_us,
                            body: Body::CursorShape(shape.clone()),
                        });
                    }
                }
                prefix.push(Record {
                    t_us,
                    body: Body::CursorUse(id),
                });
            }
            Some(CursorState::Hidden(hidden)) => prefix.push(Record {
                t_us,
                body: Body::CursorHidden(hidden),
            }),
            None => {}
        }
        prefix
    }

    fn append_record(&mut self, rec: &Record) {
        match &rec.body {
            Body::Video(video) => {
                self.stats.frames += 1;
                self.stats.keyframes += u64::from(video.keyframe);
            }
            Body::Audio(_) => self.stats.audio_packets += 1,
            Body::Gap { .. } => self.stats.gaps += 1,
            _ => {}
        }
        self.stats.last_t_us = self.stats.last_t_us.max(rec.t_us);
        let current = self.current.get_or_insert_with(|| ChunkOut {
            t_start_us: rec.t_us,
            keyframe_start: false,
            plaintext: Vec::new(),
        });
        rec.encode_into(&mut current.plaintext);
    }

    fn take_current(&mut self) -> Option<ChunkOut> {
        let current = self.current.take()?;
        self.stats.plain_bytes += current.plaintext.len() as u64;
        Some(current)
    }
}
