use proptest::prelude::*;
use rs_audio::gen::{sample_index, sample_ts_us, Signal};
use rs_audio::metrics::rms_dbfs;
use rs_audio::{CHANNELS, FRAME_LEN, FRAME_SAMPLES, FRAME_US, SAMPLE_RATE};

const START_TS: u64 = 1_700_000_000_000_000;

fn render_frames(signal: Signal, index: u64, frames: usize) -> Vec<f32> {
    let mut samples = vec![0.0; frames * CHANNELS];
    signal.render_index(index, CHANNELS, &mut samples);
    samples
}

#[test]
fn sine_render_is_independent_of_chunk_boundaries() {
    let signal = Signal::Sine {
        freq_hz: 440,
        amp: 0.5,
    };
    let frames = 9_600;
    let whole = render_frames(signal, sample_index(START_TS), frames);

    let mut chunked = Vec::with_capacity(whole.len());
    for chunk in 0..20 {
        let start_ts = START_TS + chunk as u64 * FRAME_US;
        let mut samples = vec![0.0; FRAME_LEN];
        signal.render(start_ts, CHANNELS, &mut samples);
        chunked.extend_from_slice(&samples);
    }

    assert_eq!(whole, chunked);
}

#[test]
fn sine_has_expected_level_and_matches_across_channels() {
    let signal = Signal::Sine {
        freq_hz: 440,
        amp: 0.5,
    };
    let samples = render_frames(signal, sample_index(START_TS), 9_600);

    let mono: Vec<f32> = samples
        .chunks_exact(CHANNELS)
        .map(|frame| frame[0])
        .collect();
    let peak = rs_audio::metrics::peak(&mono);
    let rms = rms_dbfs(&mono);
    assert!((0.499..=0.5).contains(&peak), "peak was {peak}");
    assert!((rms - -9.03).abs() <= 0.05, "RMS was {rms} dBFS");

    for frame in samples.chunks_exact(CHANNELS) {
        assert!(frame.iter().all(|&sample| sample == frame[0]));
    }
}

#[test]
fn bursts_start_on_period_boundaries_and_are_silent_between_bursts() {
    let signal = Signal::Bursts {
        freq_hz: 1_000,
        amp: 0.5,
        period_us: 500_000,
        on_us: 100_000,
    };
    let period_samples = sample_index(500_000);
    let on_samples = sample_index(100_000);
    let samples = render_frames(signal, 0, sample_index(2_000_000) as usize);

    for burst in 0..4_u64 {
        let first = burst * period_samples;
        assert_eq!(first, sample_index(burst * 500_000));
        let first = first as usize;
        let on_end = first + on_samples as usize;
        let period_end = first + period_samples as usize;

        assert!(samples[(first + 1) * CHANNELS..on_end * CHANNELS]
            .iter()
            .any(|sample| sample.abs() > 0.01));
        assert!(samples[on_end * CHANNELS..period_end * CHANNELS]
            .iter()
            .all(|&sample| sample == 0.0));
    }
}

#[test]
fn bursts_with_non_integral_period_render_as_silence() {
    let signal = Signal::Bursts {
        freq_hz: 440,
        amp: 0.5,
        period_us: 1,
        on_us: 1,
    };
    let mut samples = vec![1.0; FRAME_LEN];
    signal.render_index(0, CHANNELS, &mut samples);
    assert!(samples.iter().all(|&sample| sample == 0.0));
}

#[test]
fn noise_is_deterministic_bounded_and_has_expected_rms() {
    let signal = Signal::Noise { amp: 0.1, seed: 7 };
    let first = render_frames(signal, 123, 48_000);
    let second = render_frames(signal, 123, 48_000);
    assert_eq!(first, second);

    let mono: Vec<f32> = first.chunks_exact(CHANNELS).map(|frame| frame[0]).collect();
    assert!(mono.iter().all(|sample| sample.abs() <= 0.1));
    let expected_rms = 0.1 / 3.0_f32.sqrt();
    let rms = 10.0_f32.powf(rms_dbfs(&mono) / 20.0);
    assert!((rms - expected_rms).abs() / expected_rms <= 0.1);

    let different_seed = render_frames(Signal::Noise { amp: 0.1, seed: 8 }, 123, 480);
    assert_ne!(first[..different_seed.len()], different_seed[..]);
}

#[test]
fn timestamp_round_trip_loses_less_than_one_sample() {
    for ts in START_TS..START_TS + 100_000 {
        let index = sample_index(ts);
        let rounded = sample_ts_us(index);
        assert!(rounded <= ts);
        assert!(ts - rounded < 21, "timestamp {ts} rounded to {rounded}");
        assert_eq!(sample_index(rounded), index);
    }

    for index in [0, 1, 47, 48, 49, SAMPLE_RATE as u64, 2_u64.pow(50) - 1] {
        assert_eq!(sample_index(sample_ts_us(index)), index);
    }
}

#[test]
fn render_index_matches_timestamp_render_at_exact_sample_times() {
    let signal = Signal::Sine {
        freq_hz: 997,
        amp: 0.37,
    };
    for index in [1, 47, 48, 49, 12_345, 2_u64.pow(40)] {
        let by_index = render_frames(signal, index, FRAME_SAMPLES);
        let mut by_timestamp = vec![0.0; FRAME_LEN];
        signal.render(sample_ts_us(index), CHANNELS, &mut by_timestamp);
        assert_eq!(by_index, by_timestamp);
    }
}

#[test]
fn partial_frames_and_zero_channels_are_zero_filled() {
    let signal = Signal::Sine {
        freq_hz: 440,
        amp: 0.5,
    };
    let mut partial = vec![1.0; 7];
    signal.render_index(0, 2, &mut partial);
    assert_eq!(partial[6], 0.0);

    let mut no_channels = vec![1.0; 7];
    signal.render_index(0, 0, &mut no_channels);
    assert!(no_channels.iter().all(|&sample| sample == 0.0));
}

proptest! {
    #[test]
    fn sample_timestamp_round_trips_for_fifty_bit_indices(index in 0_u64..(1_u64 << 50)) {
        prop_assert_eq!(sample_index(sample_ts_us(index)), index);
    }
}
