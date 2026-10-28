//! Reception metadata omitted from ReassemblyEvent, retained for Feedback.
use std::collections::{BTreeMap, VecDeque};

use rsp_core::{stats::FEEDBACK_WINDOW_US, video::VideoHeader};

struct FrameShards {
    data_shards: usize,
    seen: Vec<bool>,
}

#[derive(Default)]
pub(super) struct Reception {
    frames: BTreeMap<(u8, u32), FrameShards>,
    samples: VecDeque<(u64, u32, u32)>,
}

impl Reception {
    pub fn packet(&mut self, wire: &[u8], now_us: u64) {
        let Ok((header, _)) = VideoHeader::parse(wire) else {
            return;
        };
        self.sample(now_us, 1, 0);
        let key = (header.epoch, header.frame_id);
        // Bound metadata even when the reorderer discards non-keyframes or
        // stale epochs without emitting a Frame/Lost event.
        if !self.frames.contains_key(&key) && self.frames.len() >= 64 {
            self.frames.pop_first();
        }
        let frame = self.frames.entry(key).or_insert_with(|| FrameShards {
            data_shards: usize::from(header.data_shards),
            seen: vec![false; usize::from(header.data_shards + header.parity_shards)],
        });
        if frame.data_shards == usize::from(header.data_shards)
            && frame.seen.len() == usize::from(header.data_shards + header.parity_shards)
        {
            frame.seen[usize::from(header.shard_idx)] = true;
        }
    }

    /// Count missing original shards when a frame completes or expires.
    /// Unused parity is optional and must not be counted as packet loss.
    pub fn finish(&mut self, epoch: Option<u8>, frame_id: u32, now_us: u64) -> bool {
        let key = epoch
            .map(|e| (e, frame_id))
            .or_else(|| self.frames.keys().find(|(_, id)| *id == frame_id).copied());
        let Some(frame) = key.and_then(|key| self.frames.remove(&key)) else {
            return false;
        };
        let missing = frame.seen[..frame.data_shards]
            .iter()
            .filter(|seen| !**seen)
            .count() as u32;
        self.sample(now_us, 0, missing);
        missing > 0
    }

    fn sample(&mut self, now_us: u64, received: u32, lost: u32) {
        self.prune(now_us);
        self.samples.push_back((now_us, received, lost));
    }

    fn prune(&mut self, now_us: u64) {
        let cutoff = now_us.saturating_sub(FEEDBACK_WINDOW_US);
        while self.samples.front().is_some_and(|(at, _, _)| *at < cutoff) {
            self.samples.pop_front();
        }
    }

    pub fn counts(&mut self, now_us: u64) -> (u32, u32) {
        self.prune(now_us);
        self.samples
            .iter()
            .filter(|(at, _, _)| *at < now_us)
            .fold((0u32, 0u32), |(received, lost), (_, r, l)| {
                (received.saturating_add(*r), lost.saturating_add(*l))
            })
    }
}
