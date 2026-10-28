use rs_audio::silence::{Gate, SilenceGate, SILENCE_DBFS, SILENCE_HANG_FRAMES};
use rs_audio::{FRAME_LEN, SAMPLE_RATE};

fn sine_frame(amplitude: f32) -> Vec<f32> {
    (0..FRAME_LEN)
        .map(|sample| {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * sample as f32 / SAMPLE_RATE as f32;
            amplitude * phase.sin()
        })
        .collect()
}

fn silent_frame() -> Vec<f32> {
    vec![0.0; FRAME_LEN]
}

#[test]
fn silence_is_sent_for_the_hang_window_then_suppressed() {
    assert_eq!(SILENCE_DBFS, -70.0);
    assert_eq!(SILENCE_HANG_FRAMES, 20);

    let mut gate = SilenceGate::new();
    let mut results = Vec::new();
    for _ in 0..10 {
        results.push(gate.push(&sine_frame(0.1)));
    }
    for _ in 0..25 {
        results.push(gate.push(&silent_frame()));
    }

    assert_eq!(&results[..10], &[Gate::Send; 10]);
    assert_eq!(&results[10..30], &[Gate::Send; 20]);
    assert_eq!(&results[30..], &[Gate::Skip; 5]);
    assert!(gate.suppressed());
}

#[test]
fn audio_resumes_and_the_next_frame_is_sent() {
    let mut gate = SilenceGate::new();
    for _ in 0..10 {
        assert_eq!(gate.push(&sine_frame(0.1)), Gate::Send);
    }
    for _ in 0..21 {
        let _ = gate.push(&silent_frame());
    }
    assert!(gate.suppressed());

    assert_eq!(gate.push(&sine_frame(0.1)), Gate::Resume);
    assert!(!gate.suppressed());
    assert_eq!(gate.push(&sine_frame(0.1)), Gate::Send);
}

#[test]
fn peak_threshold_distinguishes_quiet_from_audible_frames() {
    let mut gate = SilenceGate::with(-70.0, 0);
    assert_eq!(gate.push(&vec![2e-4; FRAME_LEN]), Gate::Skip);

    let mut gate = SilenceGate::with(-70.0, 0);
    assert_eq!(gate.push(&vec![4e-4; FRAME_LEN]), Gate::Send);
}

#[test]
fn non_finite_samples_are_not_treated_as_silence() {
    let mut gate = SilenceGate::with(-70.0, 0);
    let mut frame = silent_frame();
    frame[0] = f32::NAN;
    assert_eq!(gate.push(&frame), Gate::Send);
}

#[test]
fn zero_hang_frames_suppresses_the_first_quiet_frame() {
    let mut gate = SilenceGate::with(-60.0, 0);
    assert_eq!(gate.push(&silent_frame()), Gate::Skip);
    assert!(gate.suppressed());
}

#[test]
fn an_audible_frame_resets_the_quiet_run() {
    let mut gate = SilenceGate::new();
    for _ in 0..5 {
        assert_eq!(gate.push(&silent_frame()), Gate::Send);
    }
    assert_eq!(gate.push(&sine_frame(0.1)), Gate::Send);

    for _ in 0..20 {
        assert_eq!(gate.push(&silent_frame()), Gate::Send);
    }
    assert_eq!(gate.push(&silent_frame()), Gate::Skip);
    assert!(gate.suppressed());
}

#[test]
fn reset_restores_normal_gating() {
    let mut gate = SilenceGate::new();
    for _ in 0..21 {
        let _ = gate.push(&silent_frame());
    }
    assert!(gate.suppressed());
    gate.reset();
    assert!(!gate.suppressed());
    assert_eq!(gate.push(&sine_frame(0.1)), Gate::Send);
}
