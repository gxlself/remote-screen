//! RSP/1 video datagrams (03 §5).
//!
//! This module is pure protocol logic. The host calls packetize and sends
//! the returned datagrams; the app feeds received datagrams to Reassembler.

use std::collections::BTreeMap;

use bytes::Bytes;

use crate::fec;
use crate::wire::{DecodeError, Reader};

pub const VIDEO_TYPE: u8 = 0x10;
pub const HEADER_LEN: usize = 28;
pub const FLAG_KEYFRAME: u8 = 0x01;
pub const FLAG_STILL: u8 = 0x02;
pub const FLAG_MASK: u8 = FLAG_KEYFRAME | FLAG_STILL;
pub const DEFAULT_MAX_DATAGRAM: usize = 1150;
pub const MAX_SLOTS: usize = 8;
/// A malicious peer cannot make the receiver retain more than this amount.
pub const MAX_REASSEMBLY_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_FPS: u32 = 60;
const KEYFRAME_RETRY_US: u64 = 500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoHeader {
    pub flags: u8,
    pub display_id: u8,
    pub epoch: u8,
    pub frame_id: u32,
    pub shard_idx: u16,
    pub data_shards: u16,
    pub parity_shards: u16,
    pub shard_len: u16,
    pub frame_len: u32,
    pub capture_ts_us: u64,
}

impl VideoHeader {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(VIDEO_TYPE);
        out.push(self.flags);
        out.push(self.display_id);
        out.push(self.epoch);
        out.extend_from_slice(&self.frame_id.to_be_bytes());
        out.extend_from_slice(&self.shard_idx.to_be_bytes());
        out.extend_from_slice(&self.data_shards.to_be_bytes());
        out.extend_from_slice(&self.parity_shards.to_be_bytes());
        out.extend_from_slice(&self.shard_len.to_be_bytes());
        out.extend_from_slice(&self.frame_len.to_be_bytes());
        out.extend_from_slice(&self.capture_ts_us.to_be_bytes());
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = Vec::with_capacity(HEADER_LEN);
        self.write(&mut out);
        out.try_into().expect("video header has fixed length")
    }

    pub fn parse(buf: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let mut r = Reader::new(buf);
        let typ = r.u8()?;
        if typ != VIDEO_TYPE {
            return Err(DecodeError::UnknownType(typ));
        }
        let h = Self {
            flags: r.u8()?,
            display_id: r.u8()?,
            epoch: r.u8()?,
            frame_id: r.u32()?,
            shard_idx: r.u16()?,
            data_shards: r.u16()?,
            parity_shards: r.u16()?,
            shard_len: r.u16()?,
            frame_len: r.u32()?,
            capture_ts_us: r.u64()?,
        };
        if h.flags & !FLAG_MASK != 0 {
            return Err(DecodeError::InvalidValue("video flags"));
        }
        let k = usize::from(h.data_shards);
        let m = usize::from(h.parity_shards);
        if k == 0 || k > fec::MAX_DATA_SHARDS {
            return Err(DecodeError::InvalidValue("data shards"));
        }
        if m > fec::MAX_PARITY_SHARDS {
            return Err(DecodeError::InvalidValue("parity shards"));
        }
        if usize::from(h.shard_idx) >= k + m {
            return Err(DecodeError::InvalidValue("shard index"));
        }
        let payload = r.remaining();
        if h.shard_len == 0 {
            if k != 1 || m != 0 || payload.len() != h.frame_len as usize {
                return Err(DecodeError::InvalidValue("tcp video record"));
            }
        } else {
            if !h.shard_len.is_multiple_of(2) {
                return Err(DecodeError::InvalidValue("shard length"));
            }
            let shard_len = usize::from(h.shard_len);
            let max_frame = k
                .checked_mul(shard_len)
                .ok_or(DecodeError::InvalidValue("frame length"))?;
            if h.frame_len as usize > max_frame || payload.len() != shard_len {
                return Err(DecodeError::InvalidValue("video payload"));
            }
        }
        Ok((h, payload))
    }

    pub fn is_keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    pub flags: u8,
    pub display_id: u8,
    pub epoch: u8,
    pub frame_id: u32,
    pub capture_ts_us: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PacketizeError {
    #[error("datagram MTU {0} is too small for a video header and shard")]
    DatagramTooSmall(usize),
    #[error("frame is too large: requires {k} data shards")]
    FrameTooLarge { k: usize },
    #[error("frame length exceeds the wire type")]
    FrameLengthOverflow,
    #[error("invalid video flags")]
    InvalidFlags,
    #[error("FEC: {0}")]
    Fec(#[from] fec::FecError),
}

pub fn packetize(
    frame: &[u8],
    meta: FrameMeta,
    max_datagram: usize,
    redundancy: f32,
) -> Result<Vec<Bytes>, PacketizeError> {
    if meta.flags & !FLAG_MASK != 0 {
        return Err(PacketizeError::InvalidFlags);
    }
    if frame.len() > u32::MAX as usize {
        return Err(PacketizeError::FrameLengthOverflow);
    }
    if max_datagram <= HEADER_LEN {
        return Err(PacketizeError::DatagramTooSmall(max_datagram));
    }
    let shard_len = ((max_datagram - HEADER_LEN) / 2) * 2;
    if shard_len == 0 || shard_len > usize::from(u16::MAX) {
        return Err(PacketizeError::DatagramTooSmall(max_datagram));
    }
    let k = frame.len().div_ceil(shard_len).max(1);
    if k > fec::MAX_DATA_SHARDS {
        return Err(PacketizeError::FrameTooLarge { k });
    }
    // 03 §5.2: keyframes keep at least 0.25 redundancy while redundancy is on.
    let redundancy = if meta.flags & FLAG_KEYFRAME != 0 && redundancy > 0.0 {
        redundancy.max(fec::KEYFRAME_MIN_REDUNDANCY)
    } else {
        redundancy
    };
    let m = fec::parity_count(k, redundancy);
    let mut data = Vec::with_capacity(k);
    for i in 0..k {
        let start = i * shard_len;
        let end = (start + shard_len).min(frame.len());
        let mut shard = vec![0; shard_len];
        if start < end {
            shard[..end - start].copy_from_slice(&frame[start..end]);
        }
        data.push(shard);
    }
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let parity = fec::encode(&refs, m)?;
    let frame_len = frame.len() as u32;
    let mut result = Vec::with_capacity(k + m);
    for (idx, shard) in data.iter().chain(parity.iter()).enumerate() {
        let header = VideoHeader {
            flags: meta.flags,
            display_id: meta.display_id,
            epoch: meta.epoch,
            frame_id: meta.frame_id,
            shard_idx: idx as u16,
            data_shards: k as u16,
            parity_shards: m as u16,
            shard_len: shard_len as u16,
            frame_len,
            capture_ts_us: meta.capture_ts_us,
        };
        let mut out = Vec::with_capacity(HEADER_LEN + shard_len);
        header.write(&mut out);
        out.extend_from_slice(shard);
        result.push(Bytes::from(out));
    }
    Ok(result)
}

pub fn packetize_tcp(frame: &[u8], meta: FrameMeta) -> Result<Vec<Bytes>, PacketizeError> {
    if meta.flags & !FLAG_MASK != 0 {
        return Err(PacketizeError::InvalidFlags);
    }
    if frame.len() > u32::MAX as usize {
        return Err(PacketizeError::FrameLengthOverflow);
    }
    let header = VideoHeader {
        flags: meta.flags,
        display_id: meta.display_id,
        epoch: meta.epoch,
        frame_id: meta.frame_id,
        shard_idx: 0,
        data_shards: 1,
        parity_shards: 0,
        shard_len: 0,
        frame_len: frame.len() as u32,
        capture_ts_us: meta.capture_ts_us,
    };
    let mut out = Vec::with_capacity(HEADER_LEN + frame.len());
    header.write(&mut out);
    out.extend_from_slice(frame);
    Ok(vec![Bytes::from(out)])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_id: u32,
    pub epoch: u8,
    pub display_id: u8,
    pub flags: u8,
    pub keyframe: bool,
    pub capture_ts_us: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeReason {
    Loss,
    Backlog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyEvent {
    Frame(Frame),
    Lost {
        frame_id: u32,
    },
    NeedKeyframe {
        reason: KeyframeReason,
        last_good_frame: u32,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReassemblyStats {
    pub datagrams: u64,
    pub bytes: u64,
    pub frames: u64,
    pub keyframes: u64,
    pub frames_received: u64,
    pub frames_recovered: u64,
    pub frames_lost: u64,
    pub shards_received: u64,
    pub shards_recovered: u64,
    pub shards_discarded: u64,
    pub discarded: u64,
    pub lost_frames: u64,
    pub dropped_undecodable: u64,
    pub stale: u64,
    pub inconsistent: u64,
}

struct Slot {
    header: VideoHeader,
    last_progress_us: u64,
    shards: Vec<Option<Vec<u8>>>,
    received: usize,
    capacity: usize,
}

impl Slot {
    fn new(header: VideoHeader, now_us: u64) -> Option<Self> {
        let total = usize::from(header.data_shards) + usize::from(header.parity_shards);
        let shard_len = if header.shard_len == 0 {
            usize::try_from(header.frame_len).ok()?
        } else {
            usize::from(header.shard_len)
        };
        let capacity = total.checked_mul(shard_len)?;
        Some(Self {
            header,
            last_progress_us: now_us,
            shards: (0..total).map(|_| None).collect(),
            received: 0,
            capacity,
        })
    }

    fn complete(&self) -> bool {
        self.received >= usize::from(self.header.data_shards)
    }
}

fn serial_newer(a: u8, b: u8) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < 128
}

fn frame_newer(a: u32, b: u32) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < (1u32 << 31)
}

fn newest_frame(ids: impl Iterator<Item = u32>) -> Option<u32> {
    ids.reduce(|a, b| if frame_newer(b, a) { b } else { a })
}

fn oldest_frame(ids: impl Iterator<Item = u32>) -> Option<u32> {
    ids.reduce(|a, b| if frame_newer(a, b) { b } else { a })
}

pub struct Reassembler {
    epoch: Option<u8>,
    slots: BTreeMap<u32, Slot>,
    ready: BTreeMap<u32, Frame>,
    ready_capacity: BTreeMap<u32, usize>,
    completed: BTreeMap<u32, ()>,
    last_emitted: Option<u32>,
    waiting_keyframe: bool,
    fps: u32,
    last_need_us: Option<u64>,
    current_bytes: usize,
    pub stats: ReassemblyStats,
}

impl Reassembler {
    pub fn new(_max_reorder: u32) -> Self {
        Self {
            epoch: None,
            slots: BTreeMap::new(),
            ready: BTreeMap::new(),
            ready_capacity: BTreeMap::new(),
            completed: BTreeMap::new(),
            last_emitted: None,
            waiting_keyframe: true,
            fps: DEFAULT_FPS,
            last_need_us: None,
            current_bytes: 0,
            stats: ReassemblyStats::default(),
        }
    }

    pub fn with_fps(fps: u32) -> Self {
        let mut r = Self::new(2);
        r.set_fps(fps);
        r
    }

    pub fn set_fps(&mut self, fps: u32) {
        self.fps = fps.clamp(1, 240);
    }

    pub fn waiting_keyframe(&self) -> bool {
        self.waiting_keyframe
    }

    pub fn has_pending(&self) -> bool {
        !self.slots.is_empty() || !self.ready.is_empty()
    }

    /// Number of frame slots currently retained, including complete frames
    /// waiting for in-order delivery.
    pub fn slot_count(&self) -> usize {
        self.slots.len() + self.ready.len()
    }

    /// Bytes reserved by all retained slots and ready frames.
    pub fn buffered_bytes(&self) -> usize {
        self.current_bytes
    }

    pub fn last_good_frame(&self) -> Option<u32> {
        self.last_emitted
    }

    pub fn push(
        &mut self,
        datagram: &[u8],
        now_us: u64,
    ) -> Result<Vec<ReassemblyEvent>, DecodeError> {
        let (header, payload) = VideoHeader::parse(datagram)?;
        self.stats.datagrams += 1;
        self.stats.bytes += datagram.len() as u64;
        self.stats.shards_received += 1;

        match self.epoch {
            Some(current) if header.epoch != current && !serial_newer(header.epoch, current) => {
                self.discard();
                return Ok(Vec::new());
            }
            Some(current) if header.epoch != current => self.reset_epoch(header.epoch),
            None => self.epoch = Some(header.epoch),
            _ => {}
        }

        if let Some(last) = self.last_emitted {
            if !frame_newer(header.frame_id, last) {
                self.stats.stale += 1;
                self.discard();
                return Ok(Vec::new());
            }
        }

        if self.ready.contains_key(&header.frame_id) {
            self.stats.stale += 1;
            self.discard();
            return Ok(Vec::new());
        }
        if self.completed.contains_key(&header.frame_id) {
            self.stats.stale += 1;
            self.discard();
            return Ok(Vec::new());
        }

        if !self.slots.contains_key(&header.frame_id) {
            if self.slots.len() >= MAX_SLOTS {
                if header.is_keyframe() {
                    let stale: Vec<u32> = self
                        .slots
                        .iter()
                        .filter(|(_, slot)| !slot.header.is_keyframe())
                        .map(|(id, _)| *id)
                        .collect();
                    for id in stale {
                        if let Some(slot) = self.slots.remove(&id) {
                            self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                            self.stats.dropped_undecodable += 1;
                        }
                    }
                }
                if self.slots.len() >= MAX_SLOTS {
                    self.stats.frames_lost += 1;
                    self.stats.lost_frames += 1;
                    self.discard();
                    return Ok(self.loss_event_for(header.frame_id, now_us));
                }
            }
            let Some(slot) = Slot::new(header, now_us) else {
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                self.discard();
                return Ok(self.loss_event_for(header.frame_id, now_us));
            };
            if slot.capacity > MAX_REASSEMBLY_BYTES.saturating_sub(self.current_bytes) {
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                self.discard();
                return Ok(self.loss_event_for(header.frame_id, now_us));
            }
            self.current_bytes += slot.capacity;
            self.slots.insert(header.frame_id, slot);
        }

        let Some(slot) = self.slots.get_mut(&header.frame_id) else {
            return Ok(Vec::new());
        };
        if slot.header.data_shards != header.data_shards
            || slot.header.parity_shards != header.parity_shards
            || slot.header.shard_len != header.shard_len
            || slot.header.frame_len != header.frame_len
            || slot.header.flags != header.flags
            || slot.header.display_id != header.display_id
            || slot.header.capture_ts_us != header.capture_ts_us
        {
            self.stats.inconsistent += 1;
            self.discard();
            return Ok(Vec::new());
        }
        let idx = usize::from(header.shard_idx);
        if slot.shards[idx].is_some() {
            self.stats.stale += 1;
            self.discard();
            return Ok(Vec::new());
        }
        slot.shards[idx] = Some(payload.to_vec());
        slot.received += 1;
        slot.last_progress_us = now_us;
        let slot_complete = slot.complete();
        let _ = slot;

        let mut events = Vec::new();
        if slot_complete {
            self.stats.frames_received += 1;
            let slot = self
                .slots
                .remove(&header.frame_id)
                .expect("slot inserted above");
            self.remember_finished(header.frame_id);
            match self.recover_slot(&slot) {
                Some((frame, recovered)) => {
                    self.stats.frames_recovered += u64::from(recovered);
                    self.stats.shards_recovered += recovered as u64;
                    self.ready_capacity.insert(frame.frame_id, slot.capacity);
                    self.ready.insert(frame.frame_id, frame);
                }
                None => {
                    self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                    self.stats.frames_lost += 1;
                    self.stats.lost_frames += 1;
                    events.extend(self.loss_event_for(header.frame_id, now_us));
                }
            }
        }
        // `now_us` is the caller's processing time, not the datagram's arrival
        // time. A scheduler pause can leave the rest of this frame (including
        // its FEC shards) queued past the wall-clock deadline. Consume queued
        // shards before applying that deadline in tick(); frame-id progress
        // still expires old incomplete slots while datagrams are flowing.
        if slot_complete {
            events.extend(self.expire_old_incomplete(now_us));
        }
        events.extend(self.drain(now_us));
        Ok(events)
    }

    fn recover_slot(&self, slot: &Slot) -> Option<(Frame, bool)> {
        let k = usize::from(slot.header.data_shards);
        let m = usize::from(slot.header.parity_shards);
        if slot.header.shard_len == 0 {
            let data = slot.shards.first()?.as_ref()?.clone();
            return Some((self.frame_from(&slot.header, data), false));
        }
        let supplied: Vec<(usize, &[u8])> = slot
            .shards
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_deref().map(|s| (i, s)))
            .collect();
        let missing_data = slot.shards[..k].iter().any(Option::is_none);
        let originals = if missing_data {
            fec::recover(k, m, usize::from(slot.header.shard_len), &supplied).ok()?
        } else {
            slot.shards[..k]
                .iter()
                .cloned()
                .collect::<Option<Vec<_>>>()?
        };
        let data = originals
            .into_iter()
            .flatten()
            .take(slot.header.frame_len as usize)
            .collect();
        Some((self.frame_from(&slot.header, data), missing_data))
    }

    fn frame_from(&self, h: &VideoHeader, data: Vec<u8>) -> Frame {
        Frame {
            frame_id: h.frame_id,
            epoch: h.epoch,
            display_id: h.display_id,
            flags: h.flags,
            keyframe: h.is_keyframe(),
            capture_ts_us: h.capture_ts_us,
            data,
        }
    }

    fn reset_epoch(&mut self, epoch: u8) {
        self.epoch = Some(epoch);
        self.slots.clear();
        self.ready.clear();
        self.ready_capacity.clear();
        self.completed.clear();
        self.current_bytes = 0;
        self.last_emitted = None;
        self.waiting_keyframe = true;
    }

    fn discard(&mut self) {
        self.stats.discarded += 1;
        self.stats.shards_discarded += 1;
    }

    fn remember_finished(&mut self, frame_id: u32) {
        // Both delivered and lost frames are final. Late shards must not
        // recreate an expired slot and count the same frame as lost again.
        self.completed.insert(frame_id, ());
        while self.completed.len() > MAX_SLOTS * 2 {
            let Some(oldest) = oldest_frame(self.completed.keys().copied()) else {
                break;
            };
            self.completed.remove(&oldest);
        }
    }

    fn timeout_us(&self) -> u64 {
        (2_000_000 / u64::from(self.fps)).max(50_000)
    }

    fn expire(&mut self, now_us: u64, timeout: u64) -> Vec<ReassemblyEvent> {
        let expired: Vec<u32> = self
            .slots
            .iter()
            .filter(|(_, s)| !s.complete() && now_us.saturating_sub(s.last_progress_us) >= timeout)
            .map(|(id, _)| *id)
            .collect();
        let mut events = Vec::new();
        for id in expired {
            if let Some(slot) = self.slots.remove(&id) {
                self.remember_finished(id);
                self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                events.push(ReassemblyEvent::Lost { frame_id: id });
            }
        }
        if !events.is_empty() {
            events.push(self.need_event(KeyframeReason::Loss, now_us));
        }
        events
    }

    fn expire_old_incomplete(&mut self, now_us: u64) -> Vec<ReassemblyEvent> {
        let Some(newest) = newest_frame(self.ready.keys().copied()) else {
            return Vec::new();
        };
        let old: Vec<u32> = self
            .slots
            .keys()
            .copied()
            .filter(|id| frame_newer(newest, *id) && newest.wrapping_sub(*id) >= 2)
            .collect();
        let mut events = Vec::new();
        for id in old {
            if let Some(slot) = self.slots.remove(&id) {
                self.remember_finished(id);
                self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                events.push(ReassemblyEvent::Lost { frame_id: id });
            }
        }
        if !events.is_empty() {
            events.push(self.need_event(KeyframeReason::Loss, now_us));
        }
        events
    }

    fn loss_event_for(&mut self, frame_id: u32, now_us: u64) -> Vec<ReassemblyEvent> {
        self.remember_finished(frame_id);
        vec![
            ReassemblyEvent::Lost { frame_id },
            self.need_event(KeyframeReason::Loss, now_us),
        ]
    }

    fn need_event(&mut self, reason: KeyframeReason, now_us: u64) -> ReassemblyEvent {
        self.waiting_keyframe = true;
        self.last_need_us = Some(now_us);
        ReassemblyEvent::NeedKeyframe {
            reason,
            last_good_frame: self.last_emitted.unwrap_or(0),
        }
    }

    fn remove_ready(&mut self, frame_id: u32) -> Option<Frame> {
        let frame = self.ready.remove(&frame_id);
        if let Some(capacity) = self.ready_capacity.remove(&frame_id) {
            self.current_bytes = self.current_bytes.saturating_sub(capacity);
        }
        frame
    }

    fn drop_ready_before(&mut self, first_id: u32) {
        let stale: Vec<u32> = self
            .ready
            .keys()
            .copied()
            .filter(|id| *id != first_id && !frame_newer(*id, first_id))
            .collect();
        self.stats.dropped_undecodable += stale.len() as u64;
        for id in stale {
            self.remove_ready(id);
        }
    }

    fn drop_stale_after(&mut self, last_id: u32) -> Vec<ReassemblyEvent> {
        let mut events = Vec::new();
        let stale_ready: Vec<u32> = self
            .ready
            .keys()
            .copied()
            .filter(|id| !frame_newer(*id, last_id))
            .collect();
        for id in stale_ready {
            self.remove_ready(id);
            self.stats.frames_lost += 1;
            self.stats.lost_frames += 1;
            events.push(ReassemblyEvent::Lost { frame_id: id });
        }
        let stale_slots: Vec<u32> = self
            .slots
            .keys()
            .copied()
            .filter(|id| !frame_newer(*id, last_id))
            .collect();
        for id in stale_slots {
            if let Some(slot) = self.slots.remove(&id) {
                self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                events.push(ReassemblyEvent::Lost { frame_id: id });
            }
        }
        events
    }

    /// Exclude a receiver scheduling pause from incomplete slots' deadlines.
    /// Shards processed during recovery are anchored at `resumed_at_us`, never
    /// beyond it; older slots retain the age accumulated before the pause.
    pub fn pause_deadlines(&mut self, paused_at_us: u64, resumed_at_us: u64) {
        for slot in self.slots.values_mut() {
            let paused_us = resumed_at_us.saturating_sub(paused_at_us.max(slot.last_progress_us));
            slot.last_progress_us = slot.last_progress_us.saturating_add(paused_us);
        }
    }

    /// Apply the strict deadline since the last distinct, consistent shard.
    /// Call after draining available datagrams and compensating receiver pauses.
    pub fn tick(&mut self, now_us: u64) -> Vec<ReassemblyEvent> {
        let mut events = self.expire(now_us, self.timeout_us());
        if self.waiting_keyframe
            && self
                .last_need_us
                .is_some_and(|at| now_us.saturating_sub(at) >= KEYFRAME_RETRY_US)
        {
            events.push(self.need_event(KeyframeReason::Loss, now_us));
        }
        events.extend(self.drain(now_us));
        events
    }

    pub fn flush_backlog(&mut self, now_us: u64) -> Vec<ReassemblyEvent> {
        self.slots.clear();
        self.ready.clear();
        self.ready_capacity.clear();
        self.current_bytes = 0;
        vec![self.need_event(KeyframeReason::Backlog, now_us)]
    }

    fn drain(&mut self, now_us: u64) -> Vec<ReassemblyEvent> {
        let mut events = Vec::new();
        loop {
            if self.waiting_keyframe {
                let key = newest_frame(
                    self.ready
                        .iter()
                        .filter(|(_, frame)| frame.keyframe)
                        .filter(|(id, _)| {
                            self.last_emitted.is_none_or(|last| frame_newer(**id, last))
                        })
                        .map(|(id, _)| *id),
                );
                let Some(key) = key else {
                    self.stats.dropped_undecodable += self.ready.len() as u64;
                    let dropped = self.ready_capacity.values().sum::<usize>();
                    self.ready.clear();
                    self.ready_capacity.clear();
                    self.current_bytes = self.current_bytes.saturating_sub(dropped);
                    return events;
                };
                self.drop_ready_before(key);
                let frame = self.remove_ready(key).expect("keyframe present");
                self.waiting_keyframe = false;
                self.last_emitted = Some(frame.frame_id);
                self.stats.frames += 1;
                self.stats.keyframes += 1;
                let frame_id = frame.frame_id;
                events.push(ReassemblyEvent::Frame(frame));
                events.extend(self.drop_stale_after(frame_id));
                continue;
            }
            let Some(last) = self.last_emitted else {
                self.waiting_keyframe = true;
                continue;
            };
            let expected = last.wrapping_add(1);
            if let Some(frame) = self.remove_ready(expected) {
                self.last_emitted = Some(frame.frame_id);
                self.stats.frames += 1;
                let frame_id = frame.frame_id;
                events.push(ReassemblyEvent::Frame(frame));
                events.extend(self.drop_stale_after(frame_id));
                continue;
            }
            let has_two_newer = self
                .ready
                .keys()
                .copied()
                .any(|id| frame_newer(id, expected) && id.wrapping_sub(expected) >= 2);
            if has_two_newer {
                self.remember_finished(expected);
                self.stats.frames_lost += 1;
                self.stats.lost_frames += 1;
                events.push(ReassemblyEvent::Lost { frame_id: expected });
                events.push(self.need_event(KeyframeReason::Loss, now_us));
                // Frames older than the gap can no longer be decoded in order.
                // Keep newer ready frames, especially a keyframe recovered in
                // this same input batch, for the next wait-keyframe iteration.
                let dropped: Vec<u32> = self
                    .ready
                    .keys()
                    .copied()
                    .filter(|id| !frame_newer(*id, expected))
                    .collect();
                self.stats.dropped_undecodable += dropped.len() as u64;
                for id in dropped {
                    self.remove_ready(id);
                }
                let slots: Vec<u32> = self
                    .slots
                    .keys()
                    .copied()
                    .filter(|id| !frame_newer(*id, expected))
                    .collect();
                for id in slots {
                    if let Some(slot) = self.slots.remove(&id) {
                        self.current_bytes = self.current_bytes.saturating_sub(slot.capacity);
                    }
                }
                continue;
            }
            return events;
        }
    }
}

/// Compatibility name retained while downstream M0 code migrates.
pub type FragmentHeader = VideoHeader;
