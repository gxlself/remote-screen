//! PcmPlayout . The counting allocator records only the thread that
//! enables it, so the other tests in this binary do not affect it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use rs_audio::pcm::{PcmPlayout, PCM_MAX_MS, PCM_TARGET_MS};
use rs_audio::{FRAME_SAMPLES, SAMPLE_RATE};

const TARGET: usize = 3_840; // 80 ms at 48 kHz

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

fn measured(f: impl FnOnce()) -> (usize, usize) {
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            ENABLED.set(false);
        }
    }
    ALLOCATIONS.set(0);
    DEALLOCATIONS.set(0);
    ENABLED.set(true);
    let scope = Scope;
    f();
    drop(scope);
    (ALLOCATIONS.get(), DEALLOCATIONS.get())
}

fn add(total: &mut (usize, usize), counts: (usize, usize)) {
    total.0 += counts.0;
    total.1 += counts.1;
}

/// Pseudo-random interleaved stereo over the whole i16 range.
fn noise(frames: usize, seed: u32) -> Vec<i16> {
    let mut state = seed | 1;
    (0..frames * 2)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 16) as u16 as i16
        })
        .collect()
}

fn scaled(pcm: &[i16]) -> Vec<f32> {
    pcm.iter()
        .map(|&sample| f32::from(sample) / 32768.0)
        .collect()
}

fn pull(playout: &mut PcmPlayout, frames: usize) -> Vec<f32> {
    let mut out = vec![1.0; frames * 2];
    assert_eq!(playout.pull_interleaved(&mut out), frames);
    out
}

fn sine(rate: u32, freq: f64, frames: usize, start: usize) -> Vec<i16> {
    (start..start + frames)
        .flat_map(|index| {
            let phase = std::f64::consts::TAU * freq * index as f64 / f64::from(rate);
            let sample = (phase.sin() * 16_384.0).round() as i16;
            [sample, sample]
        })
        .collect()
}

/// Rising zero crossings, interpolated between samples, over the left channel.
fn frequency(interleaved: &[f32]) -> f64 {
    let left: Vec<f32> = interleaved.iter().step_by(2).copied().collect();
    let crossings: Vec<f64> = left
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
        .map(|(index, pair)| index as f64 + f64::from(pair[0] / (pair[0] - pair[1])))
        .collect();
    assert!(crossings.len() > 100, "{} crossings", crossings.len());
    let span = crossings[crossings.len() - 1] - crossings[0];
    (crossings.len() - 1) as f64 * f64::from(SAMPLE_RATE) / span
}

#[test]
fn stereo_48k_passes_through_after_80ms() {
    let mut playout = PcmPlayout::new();
    playout.set_format(48_000, 2);
    let input = noise(4_800, 7);
    playout.push(&input[..(TARGET - 1) * 2]);
    // Short of 80 ms: silence that consumes nothing.
    assert!(pull(&mut playout, 480).iter().all(|&x| x == 0.0));
    let stats = playout.stats();
    assert!(!stats.playing);
    assert_eq!(stats.jitter.level_ms, 79);
    assert_eq!(stats.jitter.target_ms, PCM_TARGET_MS);
    playout.push(&input[(TARGET - 1) * 2..]);
    assert_eq!(playout.stats().jitter.level_ms, 100);

    let mut output = Vec::new();
    for frames in [1, 479, 1_024, 3_296] {
        let mut left = vec![1.0; frames];
        let mut right = vec![1.0; frames];
        assert_eq!(playout.pull_planar(&mut left, &mut right), frames);
        for (l, r) in left.into_iter().zip(right) {
            output.extend([l, r]);
        }
    }
    assert_eq!(output, scaled(&input));
    let stats = playout.stats();
    assert!(stats.playing);
    assert_eq!(stats.jitter.received, 2);
    assert_eq!(stats.jitter.level_ms, 0);
    assert_eq!(stats.decoded, 10);
    assert_eq!(stats.silent_frames, 1);
    assert_eq!(stats.jitter.underruns, 0);
}

#[test]
fn pull_shapes_match_audio_playout() {
    let mut playout = PcmPlayout::new();
    let input = noise(TARGET, 11);
    playout.push(&input);
    let expected = scaled(&input);
    // Mismatched planar slices: the common prefix consumes, the rest is zeroed.
    let mut left = [1.0; 4];
    let mut right = [1.0; 2];
    assert_eq!(playout.pull_planar(&mut left, &mut right), 2);
    assert_eq!(left, [expected[0], expected[2], 0.0, 0.0]);
    assert_eq!(right, [expected[1], expected[3]]);
    // An odd trailing interleaved sample is zeroed without consuming.
    let mut out = [1.0; 5];
    assert_eq!(playout.pull_interleaved(&mut out), 2);
    assert_eq!(
        out,
        [expected[4], expected[5], expected[6], expected[7], 0.0]
    );
    assert_eq!(playout.pull_planar(&mut [], &mut []), 0);
    assert_eq!(pull(&mut playout, 1), expected[8..10]);
}

#[test]
fn resamples_44k1_sine_to_48k() {
    let mut playout = PcmPlayout::new();
    playout.set_format(44_100, 2);
    let mut output = Vec::new();
    // Real-time pacing: 10 ms in, 10 ms out, for two seconds.
    for tick in 0..200 {
        playout.push(&sine(44_100, 1_000.0, 441, tick * 441));
        output.extend(pull(&mut playout, FRAME_SAMPLES));
    }
    // 2 s of 44.1 kHz input is 95 999 frames at 48 kHz: 192 pulls of 480
    // after the 80 ms prebuffer, and 3 839 frames (79 ms) still buffered.
    let stats = playout.stats();
    assert_eq!(stats.jitter.underruns, 0);
    assert_eq!(stats.jitter.overflow_drops, 0);
    assert_eq!(stats.decoded, 192);
    assert_eq!(stats.jitter.level_ms, 79);
    let start = output.iter().position(|&x| x != 0.0).unwrap() & !1;
    let measured = frequency(&output[start..]);
    assert!((measured - 1_000.0).abs() / 1_000.0 < 0.01, "{measured} Hz");
    assert!(output.chunks_exact(2).all(|pair| pair[0] == pair[1]));
    let peak = output.iter().fold(0.0f32, |peak, &x| peak.max(x.abs()));
    assert!((0.49..=0.5).contains(&peak), "{peak}");
}

#[test]
fn mono_is_copied_and_extra_channels_dropped() {
    let mut playout = PcmPlayout::new();
    playout.set_format(48_000, 1);
    let mono = noise(TARGET / 2, 3);
    playout.push(&mono);
    let mut left = vec![1.0; TARGET];
    let mut right = vec![1.0; TARGET];
    playout.pull_planar(&mut left, &mut right);
    assert_eq!(left, scaled(&mono));
    assert_eq!(right, scaled(&mono));

    playout.set_format(48_000, 4);
    let quad = noise(TARGET * 2 + 2, 5);
    // A trailing partial frame is ignored.
    playout.push(&quad[..TARGET * 4 + 3]);
    let first_two: Vec<i16> = quad[..TARGET * 4]
        .chunks_exact(4)
        .flat_map(|frame| [frame[0], frame[1]])
        .collect();
    assert_eq!(pull(&mut playout, TARGET), scaled(&first_two));
    assert_eq!(playout.stats().jitter.level_ms, 0);
}

#[test]
fn underrun_is_silent_and_prebuffers_again() {
    let mut playout = PcmPlayout::new();
    let first = noise(TARGET, 13);
    playout.push(&first);
    assert_eq!(pull(&mut playout, TARGET), scaled(&first));
    assert!(pull(&mut playout, 480).iter().all(|&x| x == 0.0));
    let stats = playout.stats();
    assert_eq!(stats.jitter.underruns, 1);
    assert!(!stats.playing);

    // 40 ms is not enough to resume, and waiting does not consume it.
    let second = noise(TARGET, 17);
    playout.push(&second[..TARGET]);
    assert!(pull(&mut playout, 960).iter().all(|&x| x == 0.0));
    assert_eq!(playout.stats().jitter.level_ms, 40);
    playout.push(&second[TARGET..]);
    assert_eq!(pull(&mut playout, TARGET), scaled(&second));
    assert_eq!(playout.stats().jitter.underruns, 1);
    assert!(pull(&mut playout, 1).iter().all(|&x| x == 0.0));
    assert_eq!(playout.stats().jitter.underruns, 2);
}

#[test]
fn backlog_over_300ms_drops_the_oldest_down_to_80ms() {
    let mut playout = PcmPlayout::new();
    let input = noise(48_000, 19);
    let chunk = 480 * 2;
    let limit = PCM_MAX_MS as usize / 10;
    // Exactly 300 ms stays.
    for index in 0..limit {
        playout.push(&input[index * chunk..(index + 1) * chunk]);
    }
    assert_eq!(playout.stats().jitter.level_ms, PCM_MAX_MS);
    assert_eq!(playout.stats().jitter.overflow_drops, 0);
    // The next 10 ms makes 310 ms: keep only the newest 80 ms.
    playout.push(&input[limit * chunk..(limit + 1) * chunk]);
    let stats = playout.stats();
    assert_eq!(stats.jitter.level_ms, PCM_TARGET_MS);
    assert_eq!(stats.jitter.overflow_drops, 1);
    let end = (limit + 1) * chunk;
    assert_eq!(
        pull(&mut playout, TARGET),
        scaled(&input[end - TARGET * 2..end])
    );

    // One push far larger than the ring keeps its newest 80 ms.
    playout.push(&input);
    assert_eq!(playout.stats().jitter.overflow_drops, 2);
    assert_eq!(
        pull(&mut playout, TARGET),
        scaled(&input[input.len() - TARGET * 2..])
    );
}

#[test]
fn mute_ramps_to_zero_over_5ms() {
    let mut playout = PcmPlayout::new();
    playout.push(&[16_384; 48_000 / 4 * 2]);
    assert!(pull(&mut playout, 480).iter().all(|&x| x == 0.5));

    playout.set_muted(true);
    assert!(playout.stats().muted);
    let level = playout.stats().jitter.level_ms;
    let faded = pull(&mut playout, 480);
    let left: Vec<f32> = faded.iter().step_by(2).copied().collect();
    assert!(left[0] < 0.5 && left[0] > 0.49, "{}", left[0]);
    assert!(left[..240].windows(2).all(|pair| pair[1] < pair[0]));
    assert_eq!(left[239], 0.0);
    assert!(faded[480..].iter().all(|&x| x == 0.0));
    // Muted output still consumes audio.
    assert_eq!(playout.stats().jitter.level_ms, level - 10);

    playout.set_muted(false);
    let restored = pull(&mut playout, 480);
    assert!(restored[..478].iter().all(|&x| x < 0.5));
    assert!(restored[478..].iter().all(|&x| x == 0.5));
    assert!(!playout.stats().muted);
}

#[test]
fn stop_and_invalid_formats() {
    let mut playout = PcmPlayout::new();
    playout.push(&noise(4_800, 23));
    playout.stop();
    let stats = playout.stats();
    assert_eq!(stats.jitter.level_ms, 0);
    assert!(!stats.playing);
    assert!(pull(&mut playout, 480).iter().all(|&x| x == 0.0));
    assert_eq!(playout.stats().jitter.underruns, 0);

    for (rate, channels) in [(0, 2), (48_000, 0)] {
        playout.set_format(rate, channels);
        playout.push(&noise(4_800, 29));
        assert_eq!(playout.stats().jitter.level_ms, 0);
    }
    assert_eq!(playout.stats().jitter.received, 1);
    playout.set_format(48_000, 2);
    let input = noise(TARGET, 31);
    playout.push(&input);
    assert_eq!(pull(&mut playout, TARGET), scaled(&input));
}

#[test]
fn warmed_pulls_and_pushes_never_allocate_or_free() {
    let mut playout = PcmPlayout::new();
    playout.set_format(44_100, 2);
    let chunk = sine(44_100, 440.0, 441, 0);
    let burst = sine(44_100, 440.0, 44_100, 0);
    let mut left = [0.0; FRAME_SAMPLES];
    let mut right = [0.0; FRAME_SAMPLES];
    let mut out = [0.0; 1_024 * 2 + 1];
    for _ in 0..20 {
        playout.push(&chunk);
        playout.pull_planar(&mut left, &mut right);
    }
    let before = playout.stats();

    let mut pushes = (0, 0);
    let mut pulls = (0, 0);
    for tick in 0..400 {
        // Steady play, a gap long enough to underrun, a backlog burst, mute
        // ramps, a stop, and a format change, with both pull shapes.
        if !(100..120).contains(&tick) {
            add(&mut pushes, measured(|| playout.push(&chunk)));
        }
        if tick == 200 {
            add(&mut pushes, measured(|| playout.push(&burst)));
        }
        if tick == 300 {
            add(&mut pushes, measured(|| playout.stop()));
            add(&mut pushes, measured(|| playout.set_format(48_000, 1)));
        }
        playout.set_muted((150..250).contains(&tick));
        add(
            &mut pulls,
            measured(|| {
                if tick % 2 == 0 {
                    playout.pull_planar(&mut left, &mut right);
                } else {
                    playout.pull_interleaved(&mut out);
                }
            }),
        );
        std::hint::black_box((&left, &out));
    }
    assert_eq!(pushes, (0, 0));
    assert_eq!(pulls, (0, 0));
    let stats = playout.stats();
    assert!(stats.jitter.underruns > before.jitter.underruns);
    assert!(stats.jitter.overflow_drops > before.jitter.overflow_drops);
    assert!(stats.decoded > before.decoded);
}
