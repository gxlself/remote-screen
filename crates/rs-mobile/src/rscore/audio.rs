//! Per-session audio storage. Only the render methods run on the audio thread.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use rs_audio::playout::{AudioPlayout, PlayoutConfig, PlayoutStats};
use rsp_client::audio::AudioSink;

use super::{api_sessions::session_entry, error::CoreError, runtime::Core};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct AudioEntry {
    playout: Mutex<AudioPlayout>,
    closed: AtomicBool,
    contended: AtomicU32,
}

impl AudioEntry {
    fn new() -> Result<Self, CoreError> {
        Ok(Self {
            playout: Mutex::new(
                AudioPlayout::new(PlayoutConfig::default()).map_err(CoreError::internal)?,
            ),
            closed: AtomicBool::new(false),
            contended: AtomicU32::new(0),
        })
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

    /// Fill both buffers without waiting for the network/control thread.
    /// Closed, contended, or poisoned players produce silence.
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

impl AudioSink for AudioEntry {
    fn push(&self, seq: u16, capture_ts_us: u64, payload: &[u8], recv_us: u64) {
        let mut playout = lock(&self.playout);
        if !self.closed.load(Ordering::Acquire) {
            playout.push(seq, capture_ts_us, payload, recv_us);
        }
    }

    fn reset(&self) {
        lock(&self.playout).reset();
    }
}

#[derive(Default)]
pub struct AudioTable {
    entries: Mutex<BTreeMap<u64, Arc<AudioEntry>>>,
}

impl AudioTable {
    pub fn get_or_create(&self, session_id: u64) -> Result<Arc<AudioEntry>, CoreError> {
        let mut entries = lock(&self.entries);
        if let Some(entry) = entries.get(&session_id) {
            return Ok(entry.clone());
        }
        let entry = Arc::new(AudioEntry::new()?);
        entries.insert(session_id, entry.clone());
        Ok(entry)
    }

    pub fn get(&self, session_id: u64) -> Option<Arc<AudioEntry>> {
        lock(&self.entries).get(&session_id).cloned()
    }

    pub fn remove(&self, session_id: u64) {
        if let Some(entry) = lock(&self.entries).remove(&session_id) {
            entry.closed.store(true, Ordering::Release);
        }
    }
}

impl Drop for AudioTable {
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

pub fn set_audio(core: &Arc<Core>, id: u64, enabled: bool) -> Result<(), CoreError> {
    let session = session_entry(core, id)?;
    // The sink is created with SessionParams, before the handle is published.
    // Never recreate a sink removed by a concurrent close/end operation.
    if let Some(entry) = core.state::<AudioTable>().get(id) {
        entry.set_muted(!enabled);
    }
    if let Some(handle) = session.handle() {
        handle.set_audio(enabled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rscore::ffi_audio::{
        rs_audio_close, rs_audio_render, rs_audio_render_interleaved, RsAudio,
    };

    #[test]
    fn ffi_render_never_waits_for_the_playout_lock() {
        let entry = Arc::new(AudioEntry::new().unwrap());
        let handle = Box::into_raw(Box::new(RsAudio(entry.clone())));
        // Hold the lock on this very thread: a blocking render would deadlock.
        let held = lock(&entry.playout);
        let mut left = [1.0; 480];
        let mut right = [1.0; 480];
        // SAFETY: handle is live and both arrays are disjoint and large enough.
        assert_eq!(
            unsafe { rs_audio_render(handle, left.as_mut_ptr(), right.as_mut_ptr(), 480) },
            480
        );
        assert!(left.iter().chain(&right).all(|&x| x == 0.0));
        assert_eq!(entry.contended(), 1);
        let mut lr = [1.0; 960];
        // SAFETY: handle is live and lr holds 480 stereo frames.
        assert_eq!(
            unsafe { rs_audio_render_interleaved(handle, lr.as_mut_ptr(), 480) },
            480
        );
        assert!(lr.iter().all(|&x| x == 0.0));
        assert_eq!(entry.contended(), 2);
        drop(held);
        // SAFETY: No render is in flight and this is the handle's only close.
        unsafe { rs_audio_close(handle) };
    }

    #[test]
    fn poisoned_playout_is_silent_until_non_realtime_recovery() {
        let entry = AudioEntry::new().unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = lock(&entry.playout);
            panic!("test poison");
        }));
        let mut left = [1.0; 512];
        let mut right = [1.0; 512];
        assert_eq!(entry.render_planar(&mut left, &mut right), 512);
        assert!(left.iter().chain(&right).all(|&x| x == 0.0));
        assert_eq!(entry.contended(), 1);
        entry.set_muted(true);
        assert!(entry.stats().muted);
    }
}
