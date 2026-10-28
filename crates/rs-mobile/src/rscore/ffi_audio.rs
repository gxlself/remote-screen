//! Pull-based native audio ABI; handles retain the entry without table lookups.
//! Adapter session ids (>= RS_ADAPTER_ID_BASE) open the adapter's PCM player.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    ptr, slice,
    sync::Arc,
};

use rs_audio::playout::PlayoutStats;

use super::{
    adapters::{
        audio::{AdapterAudio, AdapterAudioTable},
        is_adapter_id,
    },
    audio::{AudioEntry, AudioTable},
    runtime::core,
};

/// What a handle plays: an RSP session's Opus player or an adapter session's
/// PCM player. Both follow the same realtime rules.
pub(super) trait AudioSource: Send + Sync {
    fn render_planar(&self, left: &mut [f32], right: &mut [f32]) -> usize;
    fn render_interleaved(&self, lr: &mut [f32]) -> usize;
    fn set_muted(&self, muted: bool);
    fn stats(&self) -> PlayoutStats;
    fn contended(&self) -> u32;
}

impl AudioSource for AudioEntry {
    fn render_planar(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        AudioEntry::render_planar(self, left, right)
    }

    fn render_interleaved(&self, lr: &mut [f32]) -> usize {
        AudioEntry::render_interleaved(self, lr)
    }

    fn set_muted(&self, muted: bool) {
        AudioEntry::set_muted(self, muted);
    }

    fn stats(&self) -> PlayoutStats {
        AudioEntry::stats(self)
    }

    fn contended(&self) -> u32 {
        AudioEntry::contended(self)
    }
}

impl AudioSource for AdapterAudio {
    fn render_planar(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        AdapterAudio::render_planar(self, left, right)
    }

    fn render_interleaved(&self, lr: &mut [f32]) -> usize {
        AdapterAudio::render_interleaved(self, lr)
    }

    fn set_muted(&self, muted: bool) {
        AdapterAudio::set_muted(self, muted);
    }

    fn stats(&self) -> PlayoutStats {
        AdapterAudio::stats(self)
    }

    fn contended(&self) -> u32 {
        AdapterAudio::contended(self)
    }
}

/// Opaque in C. Only `rs_audio_close` releases this owning reference.
pub struct RsAudio(pub(super) Arc<dyn AudioSource>);

/// Layout and field types match `include/rs_mobile.h` (64 bytes on LP64).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RsAudioStats {
    pub packets: u64,
    pub lost: u64,
    pub concealed: u64,
    pub fec_recovered: u64,
    pub late: u64,
    pub underruns: u64,
    pub target_ms: u32,
    pub level_ms: u32,
    pub contended: u32,
    pub playing: bool,
}

// The video ABI's guard logs on panic. Audio callbacks must never log.
fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_default()
}

fn sample_len(frames: u32, channels: usize) -> Option<usize> {
    (frames as usize)
        .checked_mul(channels)
        .filter(|&len| len <= isize::MAX as usize / size_of::<f32>())
}

/// Look up a session once, on a non-realtime thread. Unknown IDs return NULL.
#[no_mangle]
pub extern "C" fn rs_audio_open(id: u64) -> *mut RsAudio {
    guard(|| {
        let Ok(core) = core() else {
            return ptr::null_mut();
        };
        let source: Arc<dyn AudioSource> = if is_adapter_id(id) {
            let Some(audio) = core.state::<AdapterAudioTable>().get(id) else {
                return ptr::null_mut();
            };
            audio
        } else {
            let Some(entry) = core.state::<AudioTable>().get(id) else {
                return ptr::null_mut();
            };
            entry
        };
        Box::into_raw(Box::new(RsAudio(source)))
    })
}

/// Fill planar stereo, returning frames even when the player is contended.
/// # Safety
/// A non-NULL audio pointer must be an open handle, never concurrently closed.
/// Non-NULL left/right must each hold frames writable floats, be disjoint, and
/// have no other accesses for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn rs_audio_render(
    audio: *mut RsAudio,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) -> u32 {
    guard(|| {
        if audio.is_null() || left.is_null() || right.is_null() {
            return 0;
        }
        let Some(len) = sample_len(frames, 1) else {
            return 0;
        };
        // SAFETY: The caller supplies a live handle and disjoint writable
        // buffers of len floats; sample_len also bounds each slice's byte size.
        let (entry, left, right) = unsafe {
            (
                &(*audio).0,
                slice::from_raw_parts_mut(left, len),
                slice::from_raw_parts_mut(right, len),
            )
        };
        entry.render_planar(left, right);
        frames
    })
}

/// Fill interleaved stereo without waiting, allocating, or looking up a table.
/// # Safety
/// A non-NULL audio pointer must be an open handle, never concurrently closed.
/// Non-NULL lr must hold 2*frames writable floats with no concurrent accesses.
#[no_mangle]
pub unsafe extern "C" fn rs_audio_render_interleaved(
    audio: *mut RsAudio,
    lr: *mut f32,
    frames: u32,
) -> u32 {
    guard(|| {
        if audio.is_null() || lr.is_null() {
            return 0;
        }
        let Some(len) = sample_len(frames, 2) else {
            return 0;
        };
        // SAFETY: The caller supplies a live handle and len writable floats;
        // sample_len checks multiplication and the maximum slice byte size.
        let (entry, lr) = unsafe { (&(*audio).0, slice::from_raw_parts_mut(lr, len)) };
        entry.render_interleaved(lr);
        frames
    })
}

/// Set the local mute ramp on a non-realtime thread. NULL is ignored.
/// # Safety
/// A non-NULL audio pointer must be an open handle, never concurrently closed.
#[no_mangle]
pub unsafe extern "C" fn rs_audio_set_muted(audio: *mut RsAudio, muted: bool) {
    guard(|| {
        // SAFETY: A non-NULL pointer is a live handle by the caller's contract.
        if let Some(audio) = unsafe { audio.as_ref() } {
            audio.0.set_muted(muted);
        }
    });
}

/// Read statistics on a non-realtime thread. Either pointer NULL is ignored.
/// # Safety
/// audio must be NULL or an open handle, never concurrently closed. A non-NULL
/// out must be aligned writable storage for RsAudioStats, exclusively borrowed.
#[no_mangle]
pub unsafe extern "C" fn rs_audio_stats(audio: *mut RsAudio, out: *mut RsAudioStats) {
    guard(|| {
        if audio.is_null() || out.is_null() {
            return;
        }
        // SAFETY: Non-NULL audio is a live handle by the caller's contract.
        let entry = unsafe { &(*audio).0 };
        let stats = entry.stats();
        let mapped = RsAudioStats {
            packets: stats
                .jitter
                .received
                .saturating_sub(stats.jitter.late)
                .saturating_sub(stats.jitter.duplicate),
            lost: stats.jitter.lost,
            concealed: stats.concealed,
            fec_recovered: stats.fec_recovered,
            late: stats.jitter.late,
            underruns: stats.jitter.underruns,
            target_ms: stats.jitter.target_ms,
            level_ms: stats.jitter.level_ms,
            contended: entry.contended(),
            playing: stats.playing && !stats.muted,
        };
        // SAFETY: out is aligned writable storage; write permits uninitialized
        // caller storage and RsAudioStats contains no owning fields to drop.
        unsafe { out.write(mapped) };
    });
}

/// Release a handle; NULL is ignored.
/// # Safety
/// A non-NULL pointer must come from rs_audio_open, be closed exactly once, and
/// have no concurrent or subsequent accesses (including render callbacks).
#[no_mangle]
pub unsafe extern "C" fn rs_audio_close(audio: *mut RsAudio) {
    guard(|| {
        if !audio.is_null() {
            // SAFETY: The caller transfers back the unique Box allocated by
            // rs_audio_open and has stopped every access to this handle.
            drop(unsafe { Box::from_raw(audio) });
        }
    });
}
