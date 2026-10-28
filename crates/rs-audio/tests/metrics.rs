use rs_audio::gen::{sample_index, Signal};
use rs_audio::metrics::{
    burst_latencies_us, peak, percentile, rms_dbfs, ClickMeter, OnsetDetector, WindowMeter,
};

const SINE_START_INDEX: u64 = 900;

fn mono_sine(samples: usize) -> Vec<f32> {
    let mut output = vec![0.0; samples];
    Signal::Sine {
        freq_hz: 440,
        amp: 0.5,
    }
    .render_index(SINE_START_INDEX, 1, &mut output);
    output
}

#[test]
fn clean_sine_has_no_clicks() {
    let mut meter = ClickMeter::for_sine(440, 0.5);
    meter.push(&mono_sine(48_000));
    assert_eq!(meter.clicks(), 0);
    assert!(meter.positions().is_empty());
}

#[test]
fn a_zeroed_frame_is_reported_at_its_start() {
    let mut samples = mono_sine(48_000);
    samples[24_000..24_480].fill(0.0);

    let mut meter = ClickMeter::for_sine(440, 0.5);
    meter.push(&samples);
    assert!(meter.clicks() >= 1);
    assert!(
        meter
            .positions()
            .iter()
            .any(|&position| position.abs_diff(24_000) <= 48),
        "positions: {:?}",
        meter.positions()
    );
}

#[test]
fn a_step_creates_exactly_one_click() {
    let mut samples = mono_sine(48_000);
    for sample in &mut samples[24_000..] {
        *sample += 0.3;
    }

    let mut meter = ClickMeter::for_sine(440, 0.5);
    meter.push(&samples);
    assert_eq!(meter.clicks(), 1);
    assert_eq!(meter.positions(), &[24_000]);
}

#[test]
fn window_meter_finds_a_ten_millisecond_gap() {
    let mut samples = mono_sine(48_000);
    samples[24_000..24_480].fill(0.0);

    let mut meter = WindowMeter::new();
    meter.push(&samples);
    assert_eq!(meter.gaps(-40.0, 10), 1);
    assert!(meter.energy_jumps(6.0) >= 2);
}

#[test]
fn window_meter_ignores_a_five_millisecond_gap_and_clean_sine() {
    let mut five_ms = mono_sine(48_000);
    five_ms[24_000..24_240].fill(0.0);
    let mut five_ms_meter = WindowMeter::new();
    five_ms_meter.push(&five_ms);
    assert_eq!(five_ms_meter.gaps(-40.0, 10), 0);

    let mut clean_meter = WindowMeter::new();
    clean_meter.push(&mono_sine(48_000));
    assert_eq!(clean_meter.gaps(-40.0, 10), 0);
    assert_eq!(clean_meter.energy_jumps(6.0), 0);
}

#[test]
fn window_meter_state_crosses_push_boundaries() {
    let mut samples = mono_sine(48_000);
    samples[24_000..24_480].fill(0.0);
    let mut whole = WindowMeter::new();
    whole.push(&samples);

    let mut chunked = WindowMeter::new();
    for chunk in samples.chunks(137) {
        chunked.push(chunk);
    }

    assert_eq!(whole.windows(), chunked.windows());
    assert_eq!(whole.gaps(-40.0, 10), chunked.gaps(-40.0, 10));
    assert_eq!(whole.energy_jumps(6.0), chunked.energy_jumps(6.0));
}

#[test]
fn onset_detector_measures_burst_latency() {
    let capture_start_us = 300_000;
    let output_offset_us = 70_000;
    let total_samples = 2 * 48_000;
    let mut captured = vec![0.0; total_samples];
    Signal::Bursts {
        freq_hz: 440,
        amp: 0.5,
        period_us: 500_000,
        on_us: 100_000,
    }
    .render(capture_start_us, 1, &mut captured);

    let mut detector = OnsetDetector::new(-50.0, -20.0);
    for (chunk_index, chunk) in captured.chunks(480).enumerate() {
        let first_time_us = capture_start_us + output_offset_us + chunk_index as u64 * 10_000;
        detector.push(chunk, first_time_us);
    }

    assert_eq!(
        detector.onsets().len(),
        4,
        "onsets: {:?}",
        detector.onsets()
    );
    let latencies = burst_latencies_us(detector.onsets(), 500_000);
    for latency in latencies {
        assert!(
            latency.abs_diff(output_offset_us) <= 100,
            "latency was {latency} us"
        );
    }
}

#[test]
fn percentile_uses_nearest_rank_and_rejects_empty_input() {
    assert_eq!(percentile(&[5, 1, 3, 2, 4], 0.5), Some(3));
    assert_eq!(percentile(&[], 0.5), None);
    assert_eq!(percentile(&[9, 1, 5], 0.0), Some(1));
    assert_eq!(percentile(&[9, 1, 5], 1.0), Some(9));
}

#[test]
fn rms_and_peak_handle_silence() {
    assert_eq!(rms_dbfs(&[]), f32::NEG_INFINITY);
    assert_eq!(rms_dbfs(&[0.0, 0.0]), f32::NEG_INFINITY);
    assert_eq!(peak(&[]), 0.0);
    assert_eq!(peak(&[-0.25, 0.1]), 0.25);
    assert_eq!(sample_index(0), 0);
}
