//! Per-session adapter audio : PCM from the protocol driver, pulled
//! through rs_audio_* (ffi_audio.rs). Only the render methods run on the audio
//! thread.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use rs_audio::{pcm::PcmPlayout, playout::PlayoutStats};

use super::driver::PcmSink;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct AdapterAudio {
    playout: Mutex<PcmPlayout>,
    closed: AtomicBool,
    contended: AtomicU32,
}

impl AdapterAudio {
    fn new() -> Self {
        Self {
            playout: Mutex::new(PcmPlayout::new()),
            closed: AtomicBool::new(false),
            contended: AtomicU32::new(0),
        }
    }

    /// Non-realtime control operation; the player supplies the 5 ms ramp.
    pub fn set_muted(&self, muted: bool) {
        lock(&self.playout).set_muted(muted);
    }

    /// Non-realtime statistics snapshot.
    pub fn stats(&self) -> PlayoutStats {
        let mut stats = lock(&self.playout).stats();
        stats.playing &= !self.closed.load(Ordering::Acquire);
        stats
    }

    /// Fill both buffers without waiting for the driver thread. Closed,
    /// contended, or poisoned players produce silence.
    pub fn render_planar(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        if !self.closed.load(Ordering::Acquire) {
            if let Ok(mut playout) = self.playout.try_lock() {
                playout.pull_planar(left, right);
                if !self.closed.load(Ordering::Acquire) {
                    return left.len();
                }
            } else {
                self.contended.fetch_add(1, Ordering::Relaxed);
            }
        }
        left.fill(0.0);
        right.fill(0.0);
        left.len()
    }

    /// Interleaved stereo equivalent; an odd trailing sample is zeroed.
    pub fn render_interleaved(&self, lr: &mut [f32]) -> usize {
        if !self.closed.load(Ordering::Acquire) {
            if let Ok(mut playout) = self.playout.try_lock() {
                let frames = playout.pull_interleaved(lr);
                if !self.closed.load(Ordering::Acquire) {
                    return frames;
                }
            } else {
                self.contended.fetch_add(1, Ordering::Relaxed);
            }
        }
        lr.fill(0.0);
        lr.len() / 2
    }

    pub fn contended(&self) -> u32 {
        self.contended.load(Ordering::Relaxed)
    }
}

impl PcmSink for AdapterAudio {
    fn format(&self, sample_rate: u32, channels: u16) {
        lock(&self.playout).set_format(sample_rate, channels);
    }

    fn samples(&self, pcm: &[i16]) {
        let mut playout = lock(&self.playout);
        if !self.closed.load(Ordering::Acquire) {
            playout.push(pcm);
        }
    }

    fn stop(&self) {
        lock(&self.playout).stop();
    }
}

/// Adapter sessions' audio, keyed by adapter session id (session.rs creates
/// an entry when a session opens and removes it when the session ends).
#[derive(Default)]
pub struct AdapterAudioTable {
    entries: Mutex<BTreeMap<u64, Arc<AdapterAudio>>>,
}

impl AdapterAudioTable {
    pub fn get_or_create(&self, session_id: u64) -> Arc<AdapterAudio> {
        lock(&self.entries)
            .entry(session_id)
            .or_insert_with(|| Arc::new(AdapterAudio::new()))
            .clone()
    }

    pub fn get(&self, session_id: u64) -> Option<Arc<AdapterAudio>> {
        lock(&self.entries).get(&session_id).cloned()
    }

    /// Open handles stay valid and render silence from now on.
    pub fn remove(&self, session_id: u64) {
        if let Some(entry) = lock(&self.entries).remove(&session_id) {
            entry.closed.store(true, Ordering::Release);
        }
    }
}

impl Drop for AdapterAudioTable {
    fn drop(&mut self) {
        for entry in self
            .entries
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .values()
        {
            entry.closed.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_never_waits_for_the_playout_lock() {
        let audio = AdapterAudio::new();
        audio.samples(&[8_192; 9_600]);
        // Hold the lock on this very thread: a blocking render would deadlock.
        let held = lock(&audio.playout);
        let mut left = [1.0; 480];
        let mut right = [1.0; 480];
        assert_eq!(audio.render_planar(&mut left, &mut right), 480);
        assert!(left.iter().chain(&right).all(|&x| x == 0.0));
        assert_eq!(audio.contended(), 1);
        let mut lr = [1.0; 960];
        assert_eq!(audio.render_interleaved(&mut lr), 480);
        assert!(lr.iter().all(|&x| x == 0.0));
        assert_eq!(audio.contended(), 2);
        drop(held);
        assert_eq!(audio.render_interleaved(&mut lr), 480);
        assert!(lr.iter().all(|&x| x == 0.25));
        assert_eq!(audio.contended(), 2);
    }

    #[test]
    fn removed_entries_render_silence_and_ignore_samples() {
        let table = AdapterAudioTable::default();
        let audio = table.get_or_create(7);
        assert!(Arc::ptr_eq(&audio, &table.get_or_create(7)));
        audio.samples(&[8_192; 9_600]);
        table.remove(7);
        assert!(table.get(7).is_none());
        audio.samples(&[8_192; 9_600]);
        let mut lr = [1.0; 960];
        assert_eq!(audio.render_interleaved(&mut lr), 480);
        assert!(lr.iter().all(|&x| x == 0.0));
        let stats = audio.stats();
        assert!(!stats.playing);
        assert_eq!(stats.jitter.received, 1);
    }
}
