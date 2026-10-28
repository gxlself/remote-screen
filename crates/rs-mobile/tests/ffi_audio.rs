mod support;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    mem::{offset_of, size_of},
    ptr,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use rs_audio::{
    codec::{AudioEncoder, EncoderConfig},
    gen::Signal,
    metrics::rms_dbfs,
    FRAME_LEN, FRAME_SAMPLES, FRAME_US, MAX_PACKET,
};
use rs_mobile::rscore::{
    audio::{AudioEntry, AudioTable},
    ffi_audio::*,
    runtime::{install_core, take_core, Core},
};
use rsp_client::audio::AudioSink;

struct CountingAllocator;

thread_local! {
    // Count only this test thread, excluding the Core's runtime workers.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn allocated() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: Every operation delegates to System with the original pointer/layout;
// thread-local counters neither allocate nor change allocator behavior.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: GlobalAlloc forwards its caller's valid layout unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: GlobalAlloc forwards its caller's valid layout unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        allocated();
        // SAFETY: The allocation and both sizes satisfy GlobalAlloc's contract.
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The pointer/layout came from this System-backed allocator.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
static GLOBAL_CORE: Mutex<()> = Mutex::new(());

struct Fixture {
    core: Arc<Core>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = GLOBAL_CORE.lock().unwrap_or_else(|e| e.into_inner());
        let core = support::test_core();
        install_core(core.clone());
        Self {
            core,
            _serial: serial,
        }
    }

    fn entry(&self, id: u64) -> Arc<AudioEntry> {
        self.core.state::<AudioTable>().get_or_create(id).unwrap()
    }

    fn open(&self, id: u64) -> Handle {
        let pointer = rs_audio_open(id);
        assert!(!pointer.is_null());
        Handle(pointer)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        take_core();
        self.core.shutdown();
    }
}

struct Handle(*mut RsAudio);

impl Handle {
    fn planar(&self, left: &mut [f32], right: &mut [f32]) -> u32 {
        assert_eq!(left.len(), right.len());
        // SAFETY: The handle is open; caller slices are disjoint and sized.
        unsafe {
            rs_audio_render(
                self.0,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len() as u32,
            )
        }
    }

    fn interleaved(&self, lr: &mut [f32]) -> u32 {
        assert_eq!(lr.len() % 2, 0);
        // SAFETY: The handle is open and lr contains exactly this many frames.
        unsafe { rs_audio_render_interleaved(self.0, lr.as_mut_ptr(), (lr.len() / 2) as u32) }
    }

    fn mute(&self, muted: bool) {
        // SAFETY: This handle remains open until Drop.
        unsafe { rs_audio_set_muted(self.0, muted) };
    }

    fn stats(&self) -> RsAudioStats {
        let mut stats = RsAudioStats::default();
        // SAFETY: Both the handle and output storage are live and aligned.
        unsafe { rs_audio_stats(self.0, &mut stats) };
        stats
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: Render calls are synchronous; this is the only close.
        unsafe { rs_audio_close(self.0) };
    }
}

const SINE: Signal = Signal::Sine {
    freq_hz: 440,
    amp: 0.5,
};

struct Source {
    encoder: AudioEncoder,
    pcm: [f32; FRAME_LEN],
    packet: [u8; MAX_PACKET],
}

impl Source {
    fn new() -> Self {
        Self {
            encoder: AudioEncoder::new(&EncoderConfig::default()).unwrap(),
            pcm: [0.0; FRAME_LEN],
            packet: [0; MAX_PACKET],
        }
    }

    fn push(&mut self, entry: &AudioEntry, seq: u16) {
        SINE.render_index(u64::from(seq) * FRAME_SAMPLES as u64, 2, &mut self.pcm);
        let len = self.encoder.encode(&self.pcm, &mut self.packet).unwrap();
        let time = u64::from(seq) * FRAME_US;
        entry.push(seq, 1_000_000 + time, &self.packet[..len], 2_000_000 + time);
    }
}

#[test]
fn layout_matches_every_header_field() {
    assert_eq!(size_of::<RsAudioStats>(), 64);
    assert_eq!(
        [
            offset_of!(RsAudioStats, packets),
            offset_of!(RsAudioStats, lost),
            offset_of!(RsAudioStats, concealed),
            offset_of!(RsAudioStats, fec_recovered),
            offset_of!(RsAudioStats, late),
            offset_of!(RsAudioStats, underruns),
            offset_of!(RsAudioStats, target_ms),
            offset_of!(RsAudioStats, level_ms),
            offset_of!(RsAudioStats, contended),
            offset_of!(RsAudioStats, playing),
        ],
        [0, 8, 16, 24, 32, 40, 48, 52, 56, 60]
    );
}

#[test]
fn unknown_null_empty_and_closed_handles() {
    let fixture = Fixture::new();
    assert!(rs_audio_open(999).is_null());
    let entry = fixture.entry(1);
    assert!(Arc::ptr_eq(&entry, &fixture.entry(1)));
    let handle = fixture.open(1);
    let mut left = [1.0; 512];
    let mut right = [1.0; 512];
    // SAFETY: NULL arguments are explicitly accepted. Non-NULL storage is valid.
    unsafe {
        assert_eq!(
            rs_audio_render(ptr::null_mut(), left.as_mut_ptr(), right.as_mut_ptr(), 512),
            0
        );
        assert_eq!(
            rs_audio_render(handle.0, ptr::null_mut(), right.as_mut_ptr(), 512),
            0
        );
        assert_eq!(
            rs_audio_render(handle.0, left.as_mut_ptr(), ptr::null_mut(), 512),
            0
        );
        assert_eq!(
            rs_audio_render_interleaved(ptr::null_mut(), left.as_mut_ptr(), 256),
            0
        );
        assert_eq!(
            rs_audio_render_interleaved(handle.0, ptr::null_mut(), 256),
            0
        );
        rs_audio_close(ptr::null_mut());
        rs_audio_set_muted(ptr::null_mut(), true);
        let mut sentinel = RsAudioStats {
            packets: 123,
            ..Default::default()
        };
        rs_audio_stats(ptr::null_mut(), &mut sentinel);
        assert_eq!(sentinel.packets, 123);
        rs_audio_stats(handle.0, ptr::null_mut());
    }
    assert_eq!(handle.planar(&mut left, &mut right), 512);
    assert!(left.iter().chain(&right).all(|&x| x == 0.0));
    let mut source = Source::new();
    for seq in 0..6 {
        source.push(&entry, seq);
    }
    fixture.core.state::<AudioTable>().remove(1);
    assert!(fixture.core.state::<AudioTable>().get(1).is_none());
    assert!(rs_audio_open(1).is_null());
    left.fill(1.0);
    right.fill(1.0);
    assert_eq!(handle.planar(&mut left, &mut right), 512);
    assert!(left.iter().chain(&right).all(|&x| x == 0.0));
    let mut lr = [1.0; 1024];
    assert_eq!(handle.interleaved(&mut lr), 512);
    assert!(lr.iter().all(|&x| x == 0.0));
    assert!(!handle.stats().playing);
}

#[test]
fn sine_rms_statistics_mute_and_reset() {
    let fixture = Fixture::new();
    let entry = fixture.entry(1);
    let handle = fixture.open(1);
    let mut source = Source::new();
    let mut left = [0.0; 480];
    let mut right = [0.0; 480];
    let mut steady = Vec::new();
    for tick in 0..50 {
        // Pace the first 20 packets like the network; batching 200 ms would
        // correctly trigger the default jitter buffer's overflow trimming.
        if tick < 20 {
            source.push(&entry, tick);
        }
        assert_eq!(handle.planar(&mut left, &mut right), 480);
        if (5..20).contains(&tick) {
            steady.extend_from_slice(&left);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let expected_db = 20.0 * (0.5f32 / 2.0f32.sqrt()).log10();
    assert!((rms_dbfs(&steady) - expected_db).abs() < 1.0);
    let stats = handle.stats();
    assert_eq!(stats.packets, 20);
    assert!((40..=80).contains(&stats.target_ms));

    entry.reset();
    assert_eq!(handle.stats().packets, 0);
    for seq in 0..6 {
        source.push(&entry, seq);
    }
    handle.planar(&mut left, &mut right);
    assert!(rms_dbfs(&left) > -30.0);
    handle.mute(true);
    handle.planar(&mut left, &mut right);
    assert!(left[239..].iter().chain(&right[239..]).all(|&x| x == 0.0));
    handle.mute(false);
    handle.planar(&mut left, &mut right);
    assert!(rms_dbfs(&left) > -30.0);
    handle.mute(true);
    entry.reset();
    assert!(entry.stats().muted);
}

#[test]
fn variable_frame_sizes_match_interleaved_samples() {
    let fixture = Fixture::new();
    let planar_entry = fixture.entry(1);
    let interleaved_entry = fixture.entry(2);
    let planar = fixture.open(1);
    let interleaved = fixture.open(2);
    // Build a shared stream without relying on the player's private lock.
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).unwrap();
    let mut pcm = [0.0; FRAME_LEN];
    let mut packet = [0; MAX_PACKET];
    for seq in 0..20 {
        SINE.render_index(u64::from(seq) * 480, 2, &mut pcm);
        let len = encoder.encode(&pcm, &mut packet).unwrap();
        for entry in [&planar_entry, &interleaved_entry] {
            entry.push(
                seq,
                1_000_000 + u64::from(seq) * FRAME_US,
                &packet[..len],
                2_000_000 + u64::from(seq) * FRAME_US,
            );
        }
    }
    let mut total = 0;
    for frames in [471, 1024].into_iter().cycle().take(50) {
        let mut left = vec![1.0; frames];
        let mut right = vec![1.0; frames];
        let mut lr = vec![1.0; frames * 2];
        assert_eq!(planar.planar(&mut left, &mut right), frames as u32);
        assert_eq!(interleaved.interleaved(&mut lr), frames as u32);
        for ((&l, &r), pair) in left.iter().zip(&right).zip(lr.chunks_exact(2)) {
            assert_eq!([l, r], [pair[0], pair[1]]);
        }
        total += frames;
    }
    assert_eq!(total, 25 * (471 + 1024));
}

fn measured(f: impl FnOnce()) -> usize {
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            COUNTING.set(false);
        }
    }
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    let scope = Scope;
    f();
    drop(scope);
    ALLOCATIONS.get()
}

#[test]
fn native_decode_errors_are_contained_and_reported() {
    let fixture = Fixture::new();
    let entry = fixture.entry(1);
    let handle = fixture.open(1);
    for seq in 0..4 {
        // Opus code 3 requires a second framing byte. This reaches the native
        // error path rather than the allocation-free duration validation.
        entry.push(
            seq,
            1_000_000 + u64::from(seq) * FRAME_US,
            &[0xff],
            2_000_000 + u64::from(seq) * FRAME_US,
        );
    }
    let mut left = [1.0; 480];
    let mut right = [1.0; 480];
    let allocations = measured(|| {
        handle.planar(&mut left, &mut right);
    });
    assert!(left
        .iter()
        .chain(&right)
        .all(|&x| x.is_finite() && x == 0.0));
    assert!(entry.stats().decode_errors > 0);
    // Known AU2C gap: AudioDecoder::decode -> codec(error.to_string()). Keep
    // this observation visible without making the upstream defect a contract.
    eprintln!("upstream native decode error render allocations: {allocations}");
}

#[test]
fn warmed_render_callbacks_allocate_nothing() {
    let fixture = Fixture::new();
    let entry = fixture.entry(1);
    let handle = fixture.open(1);
    let mut source = Source::new();
    let mut left = [0.0; 480];
    let mut right = [0.0; 480];
    let mut lr = [0.0; 960];
    for seq in 0..8 {
        source.push(&entry, seq);
    }
    handle.planar(&mut left, &mut right);
    let mut allocations = 0;
    let mut seq = 8;
    for muted in [false, true] {
        handle.mute(muted);
        for _ in 0..1000 {
            source.push(&entry, seq);
            seq += 1;
            allocations += measured(|| {
                handle.planar(&mut left, &mut right);
            });
            source.push(&entry, seq);
            seq += 1;
            allocations += measured(|| {
                handle.interleaved(&mut lr);
            });
        }
    }
    assert!(rs_audio_open(999).is_null());
    allocations += measured(|| {
        for _ in 0..1000 {
            // SAFETY: NULL handles are valid inputs; output arrays are sized.
            unsafe {
                rs_audio_render(ptr::null_mut(), left.as_mut_ptr(), right.as_mut_ptr(), 480);
                rs_audio_render_interleaved(ptr::null_mut(), lr.as_mut_ptr(), 480);
            }
        }
    });
    fixture.core.state::<AudioTable>().remove(1);
    allocations += measured(|| {
        for _ in 0..1000 {
            handle.planar(&mut left, &mut right);
            handle.interleaved(&mut lr);
        }
    });
    assert_eq!(allocations, 0, "render callback allocations after warmup");
}

#[test]
fn warmed_bad_packet_render_callbacks_allocate_nothing() {
    let fixture = Fixture::new();
    let entry = fixture.entry(1);
    let handle = fixture.open(1);
    let mut source = Source::new();
    let mut left = [0.0; 480];
    let mut right = [0.0; 480];
    for seq in 0..8 {
        source.push(&entry, seq);
    }
    for _ in 0..8 {
        handle.planar(&mut left, &mut right);
    }

    let before = entry.stats();
    let mut allocations = 0;
    for index in 0..50u8 {
        let seq = 8 + u16::from(index);
        let time = u64::from(seq) * FRAME_US;
        // Code 3 needs a second framing byte; this packet reaches the native
        // Opus error path on every render after warmup.
        entry.push(
            seq,
            1_000_000 + time,
            &[0x03 | (index & 0xfc)],
            2_000_000 + time,
        );
        allocations += measured(|| {
            handle.planar(&mut left, &mut right);
        });
        assert!(left.iter().chain(&right).all(|&sample| sample.is_finite()));
    }
    assert_eq!(allocations, 0, "bad-packet render callback allocations");
    assert_eq!(entry.stats().decode_errors - before.decode_errors, 50);
}
