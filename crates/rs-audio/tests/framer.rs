use rs_audio::framer::{AudioFrame, Framer};
use rs_audio::{AudioError, FRAME_LEN, FRAME_SAMPLES, SAMPLE_RATE};

const T0: u64 = 1_700_000_000_000_000;

fn block_ts(index: u64, frames_per_block: u64) -> u64 {
    T0 + index * frames_per_block * 1_000_000 / u64::from(SAMPLE_RATE)
}

fn stereo_samples(frames: usize, value: f32) -> Vec<f32> {
    vec![value; frames * 2]
}

fn assert_frame_len(frame: &AudioFrame) {
    assert_eq!(frame.pcm.len(), FRAME_LEN);
}

#[test]
fn continuous_1024_frame_blocks_produce_64_frames() {
    let mut framer = Framer::new();
    let mut frames = Vec::new();

    for block in 0..30 {
        frames.extend(
            framer
                .push(block_ts(block, 1024), 2, &stereo_samples(1024, 0.1))
                .expect("valid stereo block"),
        );
    }

    assert_eq!(frames.len(), 64);
    for (n, frame) in frames.iter().enumerate() {
        assert_frame_len(frame);
        let expected = T0 + n as u64 * 10_000;
        assert!(
            frame.capture_ts_us.abs_diff(expected) <= 2,
            "frame {n}: {} != {expected}",
            frame.capture_ts_us
        );
    }

    assert_eq!(frames[2].capture_ts_us, T0 + 20_000);
    assert_eq!(frames[4].capture_ts_us, T0 + 40_000);

    assert_eq!(
        framer.stats(),
        rs_audio::framer::FramerStats {
            frames: 64,
            discontinuities: 0,
            dropped_samples: 0,
        }
    );
}

#[test]
fn mono_input_is_duplicated_to_left_and_right() {
    let mut framer = Framer::new();
    let frames = framer
        .push(T0, 1, &vec![0.25; FRAME_SAMPLES])
        .expect("valid mono block");

    assert_eq!(frames.len(), 1);
    assert!(frames[0].pcm.iter().all(|sample| *sample == 0.25));
    assert_eq!(frames[0].capture_ts_us, T0);
}

#[test]
fn more_than_two_channels_keep_only_the_first_two() {
    let mut framer = Framer::new();
    let mut samples = Vec::with_capacity(FRAME_SAMPLES * 4);
    for _ in 0..FRAME_SAMPLES {
        samples.extend_from_slice(&[0.1, 0.2, 0.3, 0.4]);
    }

    let frames = framer
        .push(T0, 4, &samples)
        .expect("valid four-channel block");

    assert_eq!(frames.len(), 1);
    for pair in frames[0].pcm.chunks_exact(2) {
        assert_eq!(pair, [0.1, 0.2]);
    }
}

#[test]
fn invalid_channel_counts_and_sample_counts_are_rejected() {
    let mut framer = Framer::new();
    assert!(matches!(
        framer.push(T0, 0, &[]),
        Err(AudioError::Invalid("channels"))
    ));
    assert!(matches!(
        framer.push(T0, 2, &[0.0; 7]),
        Err(AudioError::Invalid("sample count"))
    ));
    assert_eq!(framer.stats().frames, 0);
}

#[test]
fn forward_timestamp_jump_drops_partial_samples_and_realigns() {
    let mut framer = Framer::new();
    let first = framer
        .push(T0, 2, &stereo_samples(300, 0.25))
        .expect("valid partial block");
    assert!(first.is_empty());

    let new_ts = T0 + 50_000;
    let frames = framer
        .push(new_ts, 2, &stereo_samples(FRAME_SAMPLES, 0.5))
        .expect("valid aligned block");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].capture_ts_us, new_ts);
    assert_eq!(framer.stats().discontinuities, 1);
    assert_eq!(framer.stats().dropped_samples, 300);
}

#[test]
fn timestamp_jitter_is_not_a_discontinuity_and_frames_follow_the_sample_clock() {
    let mut framer = Framer::new();
    let mut frames = Vec::new();
    let mut jitter = 0x1234_5678_u32;

    for block in 0..30 {
        jitter = jitter.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let offset = i64::from(jitter % 10_001) - 5_000;
        let ts = block_ts(block, 1024).saturating_add_signed(offset);
        frames.extend(
            framer
                .push(ts, 2, &stereo_samples(1024, 0.1))
                .expect("valid jittered block"),
        );
    }

    assert_eq!(frames.len(), 64);
    assert_eq!(framer.stats().discontinuities, 0);
    assert_eq!(framer.stats().dropped_samples, 0);
    for pair in frames.windows(2) {
        let delta = pair[1].capture_ts_us as i64 - pair[0].capture_ts_us as i64;
        assert!(
            (delta - 10_000).abs() <= 1,
            "unexpected frame timestamp delta: {delta}"
        );
    }
}

#[test]
fn jittered_block_boundary_keeps_the_sample_timeline() {
    let mut framer = Framer::new();
    framer
        .push(T0, 2, &stereo_samples(FRAME_SAMPLES, 0.1))
        .expect("valid first block");

    let jittered_ts = T0 + 10_000 + 5_000;
    let frames = framer
        .push(jittered_ts, 2, &stereo_samples(FRAME_SAMPLES, 0.1))
        .expect("valid jittered block");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].capture_ts_us, T0 + 10_000);
    assert_eq!(framer.stats().discontinuities, 0);
}

#[test]
fn exactly_twenty_ms_is_tolerated_but_more_is_not() {
    let mut tolerated = Framer::new();
    tolerated
        .push(T0, 2, &stereo_samples(300, 0.25))
        .expect("valid partial block");
    tolerated
        .push(T0 + 6_250 + 20_000, 2, &stereo_samples(FRAME_SAMPLES, 0.25))
        .expect("valid tolerated block");
    assert_eq!(tolerated.stats().discontinuities, 0);

    let mut discontinuous = Framer::new();
    discontinuous
        .push(T0, 2, &stereo_samples(300, 0.25))
        .expect("valid partial block");
    discontinuous
        .push(T0 + 6_250 + 20_001, 2, &stereo_samples(FRAME_SAMPLES, 0.25))
        .expect("valid discontinuous block");
    assert_eq!(discontinuous.stats().discontinuities, 1);
}

#[test]
fn backward_timestamp_jump_is_a_discontinuity() {
    let mut framer = Framer::new();
    framer
        .push(T0, 2, &stereo_samples(FRAME_SAMPLES, 0.25))
        .expect("valid block");

    let new_ts = T0 + 10_000 - 30_000;
    let frames = framer
        .push(new_ts, 2, &stereo_samples(FRAME_SAMPLES, 0.25))
        .expect("valid regressed block");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].capture_ts_us, new_ts);
    assert_eq!(framer.stats().discontinuities, 1);
}

#[test]
fn non_finite_and_out_of_range_samples_are_cleaned() {
    let mut framer = Framer::new();
    let mut samples = vec![0.0; FRAME_SAMPLES];
    samples[0] = f32::NAN;
    samples[1] = 2.0;
    samples[2] = -3.0;
    samples[3] = f32::INFINITY;

    let frames = framer.push(T0, 1, &samples).expect("valid mono block");

    assert_eq!(frames.len(), 1);
    assert_eq!(
        &frames[0].pcm[..8],
        &[0.0, 0.0, 1.0, 1.0, -1.0, -1.0, 0.0, 0.0]
    );
}

#[test]
fn reset_discards_pending_samples_and_starts_a_new_timeline() {
    let mut framer = Framer::new();
    framer
        .push(T0, 2, &stereo_samples(300, 0.25))
        .expect("valid partial block");
    framer.reset();

    let new_ts = T0 + 2_000_000;
    let frames = framer
        .push(new_ts, 1, &vec![0.75; FRAME_SAMPLES])
        .expect("valid block after reset");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].capture_ts_us, new_ts);
    assert!(frames[0].pcm.iter().all(|sample| *sample == 0.75));
    assert_eq!(framer.stats().discontinuities, 0);
}
