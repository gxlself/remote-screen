use rs_audio::codec::{AudioDecoder, AudioEncoder, EncoderConfig};
use rs_audio::gen::Signal;
use rs_audio::jitter::{JitterConfig, PushResult};
use rs_audio::metrics::{rms_dbfs, ClickMeter, WindowMeter};
use rs_audio::playout::{AudioPlayout, PlayoutConfig, PlayoutStats};
use rs_audio::{CHANNELS, FRAME_LEN, FRAME_SAMPLES, FRAME_US, MAX_PACKET};

const HOST_US: u64 = 1_000_000_000;
const RECV_US: u64 = 2_000_000_000;
const SINE: Signal = Signal::Sine {
    freq_hz: 440,
    amp: 0.5,
};

fn packets(count: usize) -> Vec<Vec<u8>> {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).unwrap();
    let mut pcm = [0.0; FRAME_LEN];
    let mut payload = [0; MAX_PACKET];
    (0..count)
        .map(|index| {
            SINE.render_index((index * FRAME_SAMPLES) as u64, CHANNELS, &mut pcm);
            let len = encoder.encode(&pcm, &mut payload).unwrap();
            payload[..len].to_vec()
        })
        .collect()
}

fn push(playout: &mut AudioPlayout, seq: usize, time: usize, packet: &[u8]) {
    assert!(matches!(
        playout.push(
            seq as u16,
            HOST_US + time as u64 * FRAME_US,
            packet,
            RECV_US + time as u64 * FRAME_US,
        ),
        PushResult::Accepted | PushResult::Resync
    ));
}

fn pull(playout: &mut AudioPlayout) -> [f32; FRAME_SAMPLES] {
    let mut left = [0.0; FRAME_SAMPLES];
    let mut right = [0.0; FRAME_SAMPLES];
    assert_eq!(playout.pull_planar(&mut left, &mut right), FRAME_SAMPLES);
    // Opus may code identical source channels jointly; both still remain finite
    // and bounded, and their decoding must not depend on the callback layout.
    assert!(left
        .iter()
        .chain(&right)
        .all(|x| x.is_finite() && x.abs() <= 1.0));
    left
}

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn simulate(drop: impl Fn(usize) -> bool) -> (Vec<f32>, PlayoutStats) {
    let packets = packets(1_000);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut output = Vec::with_capacity(1_000 * FRAME_SAMPLES);
    for (tick, packet) in packets.iter().enumerate() {
        if !drop(tick) {
            push(&mut playout, tick, tick, packet);
        }
        output.extend_from_slice(&pull(&mut playout));
    }
    (output, playout.stats())
}

fn metrics(samples: &[f32]) -> (u64, u64, u64) {
    let mut clicks = ClickMeter::for_sine(440, 0.5);
    clicks.push(samples);
    let mut windows = WindowMeter::new();
    windows.push(samples);
    (
        clicks.clicks(),
        windows.gaps(-40.0, 10),
        windows.energy_jumps(6.0),
    )
}

#[test]
fn defaults_and_ten_seconds_without_loss() {
    let cfg = PlayoutConfig::default();
    assert_eq!(cfg.jitter, JitterConfig::default());
    assert_eq!(cfg.crossfade_samples, 120);
    assert_eq!(cfg.fade_samples, 240);
    let (output, stats) = simulate(|_| false);
    let (clicks, gaps, _) = metrics(&output[5 * FRAME_SAMPLES..]);
    assert_eq!(clicks, 0);
    assert_eq!(gaps, 0);
    assert_eq!(stats.concealed, 0);
    assert_eq!(stats.fec_recovered, 0);
    assert_eq!(stats.decoded, 997);
    assert_eq!(stats.silent_frames, 3);
    assert!(stats.playing);
    assert_eq!(stats.current_capture_ts_us, Some(HOST_US + 996 * FRAME_US));
}

// A callback can span several receive ticks. Deliver the packets available by
// its end before pulling; keep the same 30 ms lead, packet receive timestamps,
// and initial prebuffer as the 480-sample schedule. Neither schedule underflows
// or overflows, so both must decode exactly the same packets and samples.
fn render_callbacks(packets: &[Vec<u8>], variable: bool) -> (Vec<f32>, PlayoutStats, usize) {
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let total = packets.len() * FRAME_LEN;
    let mut output = Vec::with_capacity(total);
    for (tick, packet) in packets.iter().enumerate().take(3) {
        push(&mut playout, tick, tick, packet);
        let mut frame = [0.0; FRAME_LEN];
        playout.pull_interleaved(&mut frame);
        output.extend_from_slice(&frame);
    }
    let mut next_packet = 3;
    let mut seed = 0x52_53_41_55_32_43;
    let mut largest = 0;
    while output.len() < total {
        let requested = if variable {
            random(&mut seed) as usize % 1_024 + 1
        } else {
            FRAME_SAMPLES
        };
        let len = requested.min((total - output.len()) / CHANNELS);
        largest = largest.max(len);
        let decoded_end = output.len() / CHANNELS + len - 3 * FRAME_SAMPLES;
        let available = (decoded_end.div_ceil(FRAME_SAMPLES) + 3).min(packets.len());
        while next_packet < available {
            push(
                &mut playout,
                next_packet,
                next_packet,
                &packets[next_packet],
            );
            next_packet += 1;
        }
        // Alternate output layouts, retaining both channels for comparison.
        let mut stereo = vec![0.0; len * CHANNELS];
        if variable && random(&mut seed) & 1 != 0 {
            let mut left = vec![0.0; len];
            let mut right = vec![0.0; len];
            assert_eq!(playout.pull_planar(&mut left, &mut right), len);
            for (pair, (left, right)) in stereo
                .chunks_exact_mut(CHANNELS)
                .zip(left.into_iter().zip(right))
            {
                pair.copy_from_slice(&[left, right]);
            }
        } else {
            assert_eq!(playout.pull_interleaved(&mut stereo), len);
        }
        output.extend_from_slice(&stereo);
    }
    (output, playout.stats(), largest)
}

#[test]
fn random_callback_lengths_match_480_sample_pulls_exactly() {
    let packets = packets(1_000);
    let (reference, ref_stats, _) = render_callbacks(&packets, false);
    let (variable, stats, largest) = render_callbacks(&packets, true);
    assert_eq!(largest, 1_024);
    assert_eq!(variable, reference);
    assert_eq!(stats, ref_stats);
    let (normal, normal_stats) = simulate(|_| false);
    assert!(reference.iter().step_by(CHANNELS).copied().eq(normal));
    assert_eq!(stats, normal_stats);
}

#[test]
fn two_percent_random_loss_meets_quality_limits() {
    let mut seed = 0x52_53_41_55_32_43;
    // Uniformly choose exactly 20 of 1000 packet positions without replacement
    // (a partial Fisher-Yates shuffle), so the measured loss rate is 2% rather
    // than a potentially much smaller Bernoulli realization in this short run.
    let mut positions: Vec<usize> = (0..1_000).collect();
    let mut dropped = vec![false; 1_000];
    for index in 0..20 {
        let chosen = index + random(&mut seed) as usize % (1_000 - index);
        positions.swap(index, chosen);
        dropped[positions[index]] = true;
    }
    let (output, stats) = simulate(|tick| dropped[tick]);
    let (clicks, gaps, jumps) = metrics(&output[5 * FRAME_SAMPLES..]);
    let recovered = stats.concealed + stats.fec_recovered;
    println!(
        "2% loss: clicks={clicks}, gaps={gaps}, energy_jumps={jumps}, \
         concealed={}, fec_recovered={}, total={recovered}",
        stats.concealed, stats.fec_recovered
    );
    assert_eq!(gaps, 0);
    assert!(clicks <= 3, "clicks={clicks}");
    assert!(jumps <= 3, "energy_jumps={jumps}");
    assert!((10..=40).contains(&recovered), "recovered={recovered}");
}

#[test]
fn three_consecutive_losses_retain_energy_and_continuity() {
    let (output, stats) = simulate(|tick| (200..203).contains(&tick));
    assert_eq!(stats.concealed, 2);
    assert_eq!(stats.fec_recovered, 1);
    let lost = &output[203 * FRAME_SAMPLES..206 * FRAME_SAMPLES];
    let source_db = 20.0 * (0.5_f32 / 2.0_f32.sqrt()).log10();
    let mut windows = WindowMeter::new();
    windows.push(lost);
    let audible = windows
        .windows()
        .iter()
        .filter(|&&db| db >= source_db - 10.0)
        .count();
    let (clicks, _, _) = metrics(&output[202 * FRAME_SAMPLES..207 * FRAME_SAMPLES]);
    println!("30 ms loss: audible_windows={audible}/6, clicks={clicks}");
    assert!(audible >= 3, "windows={:?}", windows.windows());
    assert!(clicks <= 1, "clicks={clicks}");
}

#[test]
fn pause_fades_to_exact_silence_then_resumes_with_fade_in() {
    let packets = packets(180);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut previous = [0.0; FRAME_SAMPLES];
    for (tick, packet) in packets.iter().enumerate().take(100) {
        push(&mut playout, tick, tick, packet);
        previous = pull(&mut playout);
    }
    let mut meter = ClickMeter::for_sine(440, 0.5);
    meter.push(&previous);
    let mut stopped = false;
    for tick in 0..30 {
        let frame = pull(&mut playout);
        meter.push(&frame);
        if !playout.stats().playing && !stopped {
            stopped = true;
            assert_eq!(playout.stats().current_capture_ts_us, None);
            assert!(frame[..239].iter().any(|&x| x != 0.0));
            assert!(frame[239..].iter().all(|&x| x == 0.0));
        } else if stopped {
            assert!(frame.iter().all(|&x| x == 0.0));
        }
        if tick >= 10 {
            assert!(frame.iter().all(|&x| x == 0.0));
        }
    }
    assert!(stopped);
    assert_eq!(meter.clicks(), 0);
    let mut resumed = false;
    for (seq, packet) in packets.iter().enumerate().take(140).skip(100) {
        push(&mut playout, seq, seq + 30, packet);
        let frame = pull(&mut playout);
        meter.push(&frame);
        if playout.stats().playing && !resumed {
            resumed = true;
            assert!(frame[0].abs() <= 1.0 / 240.0);
            assert!(rms_dbfs(&frame[240..]) > -20.0);
        }
    }
    assert!(resumed);
    assert_eq!(meter.clicks(), 0);
}

#[test]
fn mute_ramps_mid_frame_while_decoding_and_draining_the_queue() {
    let packets = packets(100);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut reference = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut meter = ClickMeter::for_sine(440, 0.5);
    for (tick, packet) in packets.iter().enumerate() {
        push(&mut playout, tick, tick, packet);
        push(&mut reference, tick, tick, packet);
        if tick == 40 {
            let mut before = [0.0; 137 * CHANNELS];
            let mut ref_before = before;
            playout.pull_interleaved(&mut before);
            reference.pull_interleaved(&mut ref_before);
            assert_eq!(before, ref_before);
            meter.push(&before.iter().step_by(CHANNELS).copied().collect::<Vec<_>>());
            playout.set_muted(true);
            // Repeating the same request must not restart its ramp.
            let mut fading = [0.0; 343 * CHANNELS];
            let mut ref_fading = fading;
            playout.pull_interleaved(&mut fading[..120 * CHANNELS]);
            playout.set_muted(true);
            playout.pull_interleaved(&mut fading[120 * CHANNELS..]);
            reference.pull_interleaved(&mut ref_fading);
            for (index, (&actual, &expected)) in fading.iter().zip(&ref_fading).enumerate() {
                let gain = (1.0 - (index / CHANNELS + 1) as f32 / 240.0).max(0.0);
                assert!((actual - expected * gain).abs() < 1e-5);
            }
            assert!(fading[239 * CHANNELS..].iter().all(|&x| x == 0.0));
            meter.push(&fading.iter().step_by(CHANNELS).copied().collect::<Vec<_>>());
        } else {
            if tick == 70 {
                playout.set_muted(false);
            }
            let frame = pull(&mut playout);
            let ref_frame = pull(&mut reference);
            if (41..70).contains(&tick) {
                assert!(frame.iter().all(|&x| x == 0.0));
                assert!(playout.stats().muted);
            } else if tick == 70 {
                for (index, (&actual, &expected)) in frame.iter().zip(&ref_frame).enumerate() {
                    let gain = ((index + 1) as f32 / 240.0).min(1.0);
                    assert!((actual - expected * gain).abs() < 1e-5);
                }
                assert_eq!(&frame[239..], &ref_frame[239..]);
            } else {
                assert_eq!(frame, ref_frame);
            }
            meter.push(&frame);
        }
        let stats = playout.stats();
        assert_eq!(stats.jitter.received, tick as u64 + 1);
        assert!(stats.jitter.level_ms <= 30);
        assert_eq!(stats.decoded, reference.stats().decoded);
        assert_eq!(
            stats.current_capture_ts_us,
            reference.stats().current_capture_ts_us
        );
    }
    assert_eq!(meter.clicks(), 0);
}

#[test]
fn overflow_burst_crossfades_the_discontinuity() {
    let packets = packets(125);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut before = [0.0; FRAME_SAMPLES];
    for (tick, packet) in packets.iter().enumerate().take(100) {
        push(&mut playout, tick, tick, packet);
        before = pull(&mut playout);
    }
    for (seq, packet) in packets.iter().enumerate().take(120).skip(100) {
        assert_eq!(
            playout.push(
                seq as u16,
                HOST_US + seq as u64 * FRAME_US,
                packet,
                RECV_US + 120 * FRAME_US
            ),
            PushResult::Accepted
        );
    }
    assert!(playout.stats().jitter.overflow_drops > 0);
    let mut meter = ClickMeter::for_sine(440, 0.5);
    meter.push(&before);
    for _ in 0..3 {
        meter.push(&pull(&mut playout));
    }
    println!("20 packet overflow: clicks={}", meter.clicks());
    assert!(meter.clicks() <= 1);
}

fn immediate() -> PlayoutConfig {
    PlayoutConfig {
        jitter: JitterConfig {
            min_target_ms: 10,
            max_target_ms: 10,
            ..JitterConfig::default()
        },
        ..PlayoutConfig::default()
    }
}

#[test]
fn invalid_opus_packet_uses_plc_without_panicking() {
    let packets = packets(20);
    let mut playout = AudioPlayout::new(immediate()).unwrap();
    let mut decoder = AudioDecoder::new().unwrap();
    let mut expected = [0.0; FRAME_LEN];
    for (seq, packet) in packets.iter().enumerate() {
        push(&mut playout, seq, seq, packet);
        pull(&mut playout);
        decoder.decode(packet, &mut expected).unwrap();
    }
    // Fixed garbage with an invalid Opus frame count, not a valid silent packet.
    let garbage = [0x03, 0xff, 0x91, 0x17, 0x6d, 0xb4, 0x39];
    push(&mut playout, 20, 20, &garbage);
    let output = pull(&mut playout);
    decoder.conceal(&mut expected).unwrap();
    assert_eq!(output, std::array::from_fn(|i| expected[i * CHANNELS]));
    let stats = playout.stats();
    assert_eq!(stats.decode_errors, 1);
    assert_eq!(stats.concealed, 1);
    assert_eq!(stats.decoded, 20);
    assert_eq!(stats.current_capture_ts_us, None);
}

#[test]
fn invalid_fec_falls_back_to_plc() {
    let mut playout = AudioPlayout::new(immediate()).unwrap();
    let packets = packets(20);
    for (seq, packet) in packets.iter().enumerate() {
        push(&mut playout, seq, seq, packet);
        pull(&mut playout);
    }
    push(&mut playout, 21, 21, &[0x03, 0xff]);
    pull(&mut playout);
    let stats = playout.stats();
    assert_eq!(stats.decode_errors, 1);
    assert_eq!(stats.concealed, 1);
    assert_eq!(stats.fec_recovered, 0);
    assert_eq!(stats.current_capture_ts_us, None);
}

#[test]
fn reset_discards_partial_frame_and_restarts_prebuffer_and_fade() {
    let packets = packets(20);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    for (seq, packet) in packets.iter().enumerate().take(10) {
        push(&mut playout, seq, seq, packet);
        pull(&mut playout);
    }
    let mut partial = [0.0; 17];
    let mut right = partial;
    playout.pull_planar(&mut partial, &mut right);
    playout.reset();
    let mut fresh = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    assert_eq!(playout.stats(), fresh.stats());
    for (seq, packet) in packets.iter().enumerate().take(10) {
        push(&mut playout, seq, seq, packet);
        push(&mut fresh, seq, seq, packet);
        let frame = pull(&mut playout);
        assert_eq!(frame, pull(&mut fresh));
        if seq < 3 {
            assert!(frame.iter().all(|&x| x == 0.0));
            assert!(!playout.stats().playing);
        }
    }
    playout.set_muted(true);
    playout.reset();
    assert!(playout.stats().muted);
    assert!(!playout.stats().playing);
}

#[test]
fn empty_and_malformed_output_lengths_do_not_advance_extra_samples() {
    let packets = packets(4);
    let mut playout = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    let mut reference = AudioPlayout::new(PlayoutConfig::default()).unwrap();
    for (seq, packet) in packets.iter().enumerate() {
        push(&mut playout, seq, seq, packet);
        push(&mut reference, seq, seq, packet);
    }
    let before = playout.stats();
    assert_eq!(playout.pull_planar(&mut [], &mut []), 0);
    assert_eq!(playout.pull_interleaved(&mut []), 0);
    let mut odd = [f32::NAN];
    assert_eq!(playout.pull_interleaved(&mut odd), 0);
    assert_eq!(odd, [0.0]);
    let mut extra = [f32::NAN; 3];
    assert_eq!(playout.pull_planar(&mut extra, &mut []), 0);
    assert_eq!(extra, [0.0; 3]);
    assert_eq!(playout.stats(), before);

    let mut left = [f32::NAN; 471];
    let mut right = [f32::NAN; 473];
    let mut expected = [0.0; 471 * CHANNELS];
    assert_eq!(playout.pull_planar(&mut left, &mut right), 471);
    reference.pull_interleaved(&mut expected);
    for (i, pair) in expected.chunks_exact(CHANNELS).enumerate() {
        assert_eq!(left[i], pair[0]);
        assert_eq!(right[i], pair[1]);
    }
    assert_eq!(&right[471..], &[0.0; 2]);
    let mut left = [f32::NAN; 15];
    let mut right = [f32::NAN; 9];
    assert_eq!(playout.pull_planar(&mut left, &mut right), 9);
    reference.pull_interleaved(&mut [0.0; 18]);
    assert_eq!(&left[9..], &[0.0; 6]);
    let mut interleaved = [f32::NAN; 943];
    let mut expected = [0.0; 942];
    assert_eq!(playout.pull_interleaved(&mut interleaved), 471);
    reference.pull_interleaved(&mut expected);
    assert_eq!(&interleaved[..942], &expected);
    assert_eq!(interleaved[942], 0.0);
    assert_eq!(playout.stats(), reference.stats());
}

#[test]
fn fades_can_span_frames_and_zero_length_fades_are_supported() {
    let packets = packets(20);
    for fade_samples in [0, 1, 240, 1_024] {
        let mut playout = AudioPlayout::new(PlayoutConfig {
            fade_samples,
            crossfade_samples: usize::MAX,
            ..immediate()
        })
        .unwrap();
        for (seq, packet) in packets.iter().enumerate() {
            push(&mut playout, seq, seq, packet);
            pull(&mut playout);
        }
        playout.set_muted(true);
        let mut muted = vec![1.0; (fade_samples + FRAME_SAMPLES) * CHANNELS];
        playout.pull_interleaved(&mut muted);
        assert!(muted[fade_samples * CHANNELS..].iter().all(|&x| x == 0.0));
        for _ in 0..10 {
            pull(&mut playout);
        }
        assert!(pull(&mut playout).iter().all(|&x| x == 0.0));
    }
}
