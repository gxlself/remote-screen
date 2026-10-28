//! Kept in its own test binary so allocator instrumentation cannot affect other
//! audio tests. The task explicitly requires a global allocator; the only unsafe
//! code here forwards the allocator contract unchanged to `System`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use rs_audio::codec::{AudioEncoder, EncoderConfig};
use rs_audio::gen::Signal;
use rs_audio::jitter::JitterConfig;
use rs_audio::playout::{AudioPlayout, PlayoutConfig};
use rs_audio::{CHANNELS, FRAME_LEN, FRAME_SAMPLES, FRAME_US, MAX_PACKET};

struct CountingAllocator;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record(counter: &'static std::thread::LocalKey<Cell<usize>>) {
    if ENABLED.try_with(Cell::get).unwrap_or(false) {
        let _ = counter.try_with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: Every allocation operation forwards its original pointer/layout to
// System. Thread-local counters never allocate or modify the allocation itself.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(&ALLOCATIONS);
        // SAFETY: The caller supplies the valid Layout required by GlobalAlloc.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(&ALLOCATIONS);
        // SAFETY: The caller supplies the valid Layout required by GlobalAlloc.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(&DEALLOCATIONS);
        // SAFETY: This pointer came from System with this same Layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(&ALLOCATIONS);
        record(&DEALLOCATIONS);
        // SAFETY: The pointer/Layout and new_size obey GlobalAlloc's contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Measurement;

impl Measurement {
    fn start() -> Self {
        ALLOCATIONS.set(0);
        DEALLOCATIONS.set(0);
        ENABLED.set(true);
        Self
    }

    fn finish(self) -> (usize, usize) {
        ENABLED.set(false);
        (ALLOCATIONS.get(), DEALLOCATIONS.get())
    }
}

impl Drop for Measurement {
    fn drop(&mut self) {
        ENABLED.set(false);
    }
}

fn encode(encoder: &mut AudioEncoder, index: usize, packet: &mut [u8]) -> usize {
    let mut pcm = [0.0; FRAME_LEN];
    Signal::Sine {
        freq_hz: 440,
        amp: 0.5,
    }
    .render_index((index * FRAME_SAMPLES) as u64, CHANNELS, &mut pcm);
    encoder.encode(&pcm, packet).unwrap()
}

fn push(playout: &mut AudioPlayout, seq: usize, time: usize, packet: &[u8]) {
    playout.push(
        seq as u16,
        1_000_000_000 + time as u64 * FRAME_US,
        packet,
        2_000_000_000 + time as u64 * FRAME_US,
    );
}

#[test]
fn warmed_up_pulls_never_allocate_or_free() {
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).unwrap();
    let mut packet = [0; MAX_PACKET];
    let mut left = [0.0; FRAME_SAMPLES];
    let mut right = [0.0; FRAME_SAMPLES];

    // Warm the native decoder and every Rust-side buffer for two seconds.
    for seq in 0..200 {
        let len = encode(&mut encoder, seq, &mut packet);
        push(&mut playout, seq, seq, &packet[..len]);
        playout.pull_planar(&mut left, &mut right);
    }
    let before = playout.stats();
    let measurement = Measurement::start();
    for _ in 0..1_000 {
        playout.pull_planar(&mut left, &mut right);
        std::hint::black_box(&left);
    }
    let empty_counts = measurement.finish();
    assert_eq!(empty_counts, (0, 0));
    assert_eq!(playout.stats().concealed - before.concealed, 3);
    assert!(playout.stats().silent_frames - before.silent_frames > 990);

    let mut push_counts = (0, 0);
    let mut pull_counts = (0, 0);
    for index in 0..1_000 {
        let seq = 200 + index;
        let len = encode(&mut encoder, seq, &mut packet);
        let measurement = Measurement::start();
        // A fresh timestamp after silence starts a new talkspurt.
        push(&mut playout, seq, 1_200 + index, &packet[..len]);
        let counts = measurement.finish();
        push_counts.0 += counts.0;
        push_counts.1 += counts.1;

        let measurement = Measurement::start();
        playout.pull_planar(&mut left, &mut right);
        std::hint::black_box(&left);
        let counts = measurement.finish();
        pull_counts.0 += counts.0;
        pull_counts.1 += counts.1;
    }
    assert_eq!(push_counts, (0, 0));
    assert_eq!(pull_counts, (0, 0));

    // Explicitly cover FEC and its following packet discontinuity before any
    // overflow could discard the missing sequence.
    let missing_before = playout.stats();
    let mut fec_counts = (0, 0);
    for seq in 1_200..1_230 {
        if seq != 1_205 {
            let len = encode(&mut encoder, seq, &mut packet);
            push(&mut playout, seq, seq + 1_000, &packet[..len]);
        }
        let measurement = Measurement::start();
        playout.pull_planar(&mut left, &mut right);
        let counts = measurement.finish();
        fec_counts.0 += counts.0;
        fec_counts.1 += counts.1;
    }
    assert_eq!(fec_counts, (0, 0));
    assert!(playout.stats().fec_recovered > missing_before.fec_recovered);

    // An overflow burst, both mute ramps, and interleaved callbacks as well.
    for seq in 1_230..1_250 {
        let len = encode(&mut encoder, seq, &mut packet);
        push(&mut playout, seq, seq + 1_000, &packet[..len]);
    }
    let mut out = [0.0; 1_024 * CHANNELS];
    let measurement = Measurement::start();
    for index in 0..40 {
        playout.set_muted(index < 20);
        playout.pull_interleaved(&mut out);
        std::hint::black_box(&out);
    }
    let burst_counts = measurement.finish();
    assert_eq!(burst_counts, (0, 0));
    assert!(playout.stats().jitter.overflow_drops > missing_before.jitter.overflow_drops);

    // The malformed-packet path is allocation-free as well.
    playout.reset();
    for seq in 0..4 {
        push(&mut playout, seq, seq, &[0x03, 0xff, 0x91, 0x17]);
    }
    let measurement = Measurement::start();
    playout.pull_planar(&mut left, &mut right);
    let error_counts = measurement.finish();
    assert_eq!(error_counts, (0, 0));
    assert_eq!(playout.stats().decode_errors, 1);
    println!(
        "alloc/free: 1000 no-push pulls={empty_counts:?}, \
         1000 push/pull={push_counts:?}/{pull_counts:?}, FEC={fec_counts:?}, burst={burst_counts:?}; \
         malformed-packet path={error_counts:?}"
    );
}

#[test]
fn warmed_malformed_packet_pushes_and_pulls_never_allocate_or_free() {
    let mut config = PlayoutConfig::default();
    config.jitter = JitterConfig {
        max_target_ms: 1_000,
        ..config.jitter
    };
    let mut playout = AudioPlayout::new(config).unwrap();
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).unwrap();
    let mut packet = [0; MAX_PACKET];
    let mut left = [0.0; FRAME_SAMPLES];
    let mut right = [0.0; FRAME_SAMPLES];

    for seq in 0..200 {
        let len = encode(&mut encoder, seq, &mut packet);
        push(&mut playout, seq, seq, &packet[..len]);
        playout.pull_planar(&mut left, &mut right);
    }
    playout.reset();

    let mut malformed = [0u8; 50];
    let mut random = 0x9e37_79b9u32;
    for byte in &mut malformed {
        random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // Code 3 without its required framing byte is always an invalid packet.
        *byte = (random >> 24) as u8 | 0x03;
    }

    let before = playout.stats();
    let measurement = Measurement::start();
    for (index, byte) in malformed.iter().enumerate() {
        push(&mut playout, index, index, std::slice::from_ref(byte));
    }
    let push_counts = measurement.finish();

    let measurement = Measurement::start();
    for _ in 0..malformed.len() {
        playout.pull_planar(&mut left, &mut right);
        std::hint::black_box(&left);
    }
    let pull_counts = measurement.finish();

    assert_eq!(push_counts, (0, 0));
    assert_eq!(pull_counts, (0, 0));
    assert_eq!(
        playout.stats().decode_errors - before.decode_errors,
        malformed.len() as u64
    );
}
