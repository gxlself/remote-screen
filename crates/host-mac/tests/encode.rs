#![cfg(target_os = "macos")]

use std::env;
use std::ffi::OsString;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use host_core::platform::{
    EncodeOpts, EncodedFrame, Encoder, EncoderConfig, Platform, PlatformError,
};
use host_mac::encode::{
    self, churn_block_count, churn_blocks, data_rate_limits, nal_types, psnr_db, rate_limit_pairs,
    refine_bump, select_churn, synthetic_frame, synthetic_frame_pattern, ChurnTexture,
    EncoderStats, MacEncoder, RateLimitScheme, StepMeter,
};
use host_mac::MacPlatform;
use rsp_core::{clock::unix_micros, pb::Codec};
use tokio::sync::mpsc;

// Every test that reads or changes RS_HOST_DISABLE_HEVC holds this lock.
static ENCODE_TEST_LOCK: Mutex<()> = Mutex::new(());

struct DisableHevcGuard(Option<OsString>);
impl DisableHevcGuard {
    fn set(disabled: bool) -> Self {
        let old = env::var_os("RS_HOST_DISABLE_HEVC");
        // SAFETY: all encoder tests serialize this process-local variable;
        // no async runtime or parallel encoder worker reads it outside the lock.
        unsafe {
            if disabled {
                env::set_var("RS_HOST_DISABLE_HEVC", "1");
            } else {
                env::remove_var("RS_HOST_DISABLE_HEVC");
            }
        }
        Self(old)
    }
}
impl Drop for DisableHevcGuard {
    fn drop(&mut self) {
        // SAFETY: the encoder-test lock is still held, including during unwind.
        unsafe {
            match &self.0 {
                Some(value) => env::set_var("RS_HOST_DISABLE_HEVC", value),
                None => env::remove_var("RS_HOST_DISABLE_HEVC"),
            }
        }
    }
}

fn config(codec: Codec, sink: mpsc::Sender<EncodedFrame>) -> EncoderConfig {
    EncoderConfig {
        codec,
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 8_000,
        sink,
    }
}

fn open(codec: Codec) -> Option<(MacEncoder, mpsc::Receiver<EncodedFrame>)> {
    open_rate(codec, 8_000)
}

fn open_rate(codec: Codec, kbps: u32) -> Option<(MacEncoder, mpsc::Receiver<EncodedFrame>)> {
    let (tx, rx) = mpsc::channel(512);
    let mut cfg = config(codec, tx);
    cfg.bitrate_kbps = kbps;
    match encode::encoder(cfg) {
        Ok((encoder, info)) => {
            eprintln!("VT info: {info:?}");
            assert_eq!(info.codec, codec);
            if codec == Codec::Hevc {
                assert!(info.hardware, "HEVC must require hardware");
            }
            Some((encoder, rx))
        }
        Err(error) => {
            assert!(
                env::var_os("RS_REQUIRE_VT").is_none_or(|value| value != "1"),
                "RS_REQUIRE_VT=1: {codec:?} unavailable: {error}"
            );
            eprintln!("SKIP vt-unavailable: {codec:?}: {error}");
            None
        }
    }
}

fn receive(
    encoder: &MacEncoder,
    rx: &mut mpsc::Receiver<EncodedFrame>,
    count: usize,
) -> Vec<EncodedFrame> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frames = Vec::with_capacity(count);
    while frames.len() < count && Instant::now() < deadline {
        match rx.try_recv() {
            Ok(frame) => frames.push(frame),
            Err(mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(1)),
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    assert_eq!(
        frames.len(),
        count,
        "VT created successfully but failed to produce all frames; stats={:?}",
        encoder.stats()
    );
    frames
}

fn smooth_frame_at(index: u32, base: u64, start: Instant) -> host_core::platform::CapturedFrame {
    let offset = Duration::from_micros(u64::from(index) * 1_000_000 / 60);
    let frame = synthetic_frame(1280, 720, index, base + offset.as_micros() as u64);
    // Construct before the deadline; warm-up must not flood a real-time
    // encoder in a burst and inadvertently test overload instead of flags.
    std::thread::sleep((start + offset).saturating_duration_since(Instant::now()));
    frame
}

fn assert_no_encoder_errors(stats: EncoderStats) {
    assert_eq!(stats.submit_error, 0, "{stats:?}");
    assert_eq!(stats.callback_status_error, 0, "{stats:?}");
    assert_eq!(stats.unexpected_null, 0, "{stats:?}");
    assert_eq!(stats.conversion_error, 0, "{stats:?}");
}

fn stream_120(codec: Codec) {
    let Some((mut encoder, mut rx)) = open(codec) else {
        return;
    };
    let started = Instant::now();
    let base = unix_micros();
    for index in 0..120u32 {
        encoder
            .encode(smooth_frame_at(index, base, started), EncodeOpts::default())
            .expect("encode synthetic frame");
    }
    let stats = encoder.finish(); // CompleteFrames drains callbacks before stats.
    let frames = receive(&encoder, &mut rx, 120);
    assert_eq!(stats.submitted, 120, "{stats:?}");
    assert_eq!(stats.output, 120, "{stats:?}");
    assert_eq!(stats.dropped, 0, "{stats:?}");
    assert_eq!(stats.callback_error, 0, "{stats:?}");
    assert_no_encoder_errors(stats);
    assert!(frames[0].keyframe, "first output must be a keyframe");
    let sets: &[u8] = if codec == Codec::Hevc {
        &[32, 33, 34]
    } else {
        &[7, 8]
    };
    for frame in &frames {
        assert!(frame.data.starts_with(&[0, 0, 0, 1]));
        assert!(!frame.still_refine);
        let types = nal_types(&frame.data, codec);
        assert!(!types.is_empty());
        if frame.keyframe {
            for parameter in sets {
                assert!(
                    types.contains(parameter),
                    "missing parameter set {parameter} in {types:?}"
                );
            }
            if codec == Codec::H264 {
                assert!(types.contains(&5), "H.264 keyframe needs IDR NAL");
            }
        } else {
            assert!(
                sets.iter().all(|parameter| !types.contains(parameter)),
                "delta frame must not prepend parameter sets"
            );
        }
    }
    assert!(frames.iter().any(|frame| !frame.keyframe));
    assert!(frames
        .windows(2)
        .all(|pair| pair[0].capture_ts_us < pair[1].capture_ts_us));
}

#[test]
fn hevc_720p60_120_frames_annex_b_and_parameter_sets() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    stream_120(Codec::Hevc);
}

#[test]
fn h264_720p60_120_frames_annex_b_and_parameter_sets() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    stream_120(Codec::H264);
}

#[test]
fn force_keyframe_is_honored_within_two_submissions() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    for codec in [Codec::Hevc, Codec::H264] {
        let Some((mut encoder, mut rx)) = open(codec) else {
            continue;
        };
        let base = unix_micros();
        let started = Instant::now();
        for index in 0..30u32 {
            encoder
                .encode(smooth_frame_at(index, base, started), EncodeOpts::default())
                .expect("warm up encoder");
        }
        receive(&encoder, &mut rx, 30);
        encoder.force_keyframe();
        let requested_us = base + 30 * 1_000_000 / 60;
        for index in 30..32u32 {
            encoder
                .encode(smooth_frame_at(index, base, started), EncodeOpts::default())
                .expect("submit after keyframe request");
        }
        let frames = receive(&encoder, &mut rx, 2);
        assert!(frames
            .iter()
            .any(|frame| frame.keyframe && frame.capture_ts_us >= requested_us));
        assert!(frames.iter().all(|frame| !frame.still_refine));
        let stats = encoder.finish();
        assert_eq!(stats.submitted, 32, "{stats:?}");
        assert_eq!(stats.output, 32, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
        assert_eq!(stats.forced_key_retries, 0, "{stats:?}");
        assert_no_encoder_errors(stats);
    }
}

#[test]
fn refinement_flags_are_per_frame_and_keyframes_take_precedence() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    for codec in [Codec::Hevc, Codec::H264] {
        let Some((mut encoder, mut rx)) = open(codec) else {
            continue;
        };
        let base = unix_micros();
        let started = Instant::now();
        let mut last = None;
        for index in 0..30u32 {
            let frame = smooth_frame_at(index, base, started);
            last = Some(frame.clone());
            encoder
                .encode(frame, EncodeOpts::default())
                .expect("warm up encoder");
        }
        receive(&encoder, &mut rx, 30);
        let mut still = last.expect("thirty warm-up frames were submitted");
        still.capture_ts_us = base + 30 * 16_667;
        encoder
            .encode(
                still.clone(),
                EncodeOpts {
                    quality_boost: true,
                    force_keyframe: false,
                },
            )
            .expect("refine last picture");
        // Submit before waiting for the refine callback: this also exercises
        // the fallback's protection against leaking the temporary rate.
        still.capture_ts_us += 16_667;
        encoder
            .encode(still.clone(), EncodeOpts::default())
            .expect("encode after refine");
        let frames = receive(&encoder, &mut rx, 2);
        assert!(frames[0].still_refine);
        assert!(!frames[0].keyframe);
        assert!(!frames[1].still_refine);

        encoder.set_rate(6_000, 60);
        encoder.set_quality_boost(true);
        for _ in 0..2 {
            still.capture_ts_us += 16_667;
            encoder
                .encode(still.clone(), EncodeOpts::default())
                .expect("persistent quality boost");
        }
        encoder.set_quality_boost(false);
        still.capture_ts_us += 16_667;
        encoder
            .encode(still.clone(), EncodeOpts::default())
            .expect("persistent boost off");
        let frames = receive(&encoder, &mut rx, 3);
        assert!(frames[..2]
            .iter()
            .all(|frame| frame.still_refine && !frame.keyframe));
        assert!(!frames[2].still_refine);

        still.capture_ts_us += 16_667;
        encoder
            .encode(
                still,
                EncodeOpts {
                    force_keyframe: true,
                    quality_boost: true,
                },
            )
            .expect("keyframe takes precedence");
        let frames = receive(&encoder, &mut rx, 1);
        assert!(frames[0].keyframe);
        assert!(!frames[0].still_refine);
        let stats = encoder.finish();
        assert_eq!(stats.submitted, 36, "{stats:?}");
        assert_eq!(stats.output, 36, "{stats:?}");
        assert_eq!(stats.dropped, 0, "{stats:?}");
        assert_no_encoder_errors(stats);
    }
}

#[test]
fn disable_hevc_preserves_h264_and_restores_environment() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(true);
    let platform = MacPlatform::new();
    let (tx, _rx) = mpsc::channel(4);
    assert!(matches!(
        platform.encoder(config(Codec::Hevc, tx)),
        Err(PlatformError::Unsupported)
    ));
    // Only encoder creation can skip; a working H.264 encoder must still
    // produce output even when the process-wide HEVC diagnostic switch is set.
    if let Some((mut encoder, mut rx)) = open(Codec::H264) {
        encoder
            .encode(
                synthetic_frame(1280, 720, 1, unix_micros()),
                EncodeOpts::default(),
            )
            .expect("H.264 remains usable");
        encoder.finish();
        assert!(receive(&encoder, &mut rx, 1)[0].keyframe);
    }
}

#[test]
fn invalid_encoder_parameters_and_foreign_frames_are_errors() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    let platform = MacPlatform::new();
    let (tx, _rx) = mpsc::channel(16);
    let good = config(Codec::Hevc, tx);
    for bad in [
        EncoderConfig {
            width: 1,
            ..good.clone()
        },
        EncoderConfig {
            fps: 0,
            ..good.clone()
        },
        EncoderConfig {
            bitrate_kbps: 0,
            ..good.clone()
        },
    ] {
        assert!(matches!(
            platform.encoder(bad),
            Err(PlatformError::Platform(_))
        ));
    }
    assert!(matches!(
        platform.encoder(EncoderConfig {
            codec: Codec::Unspecified,
            ..good
        }),
        Err(PlatformError::Unsupported)
    ));
    if let Some((mut encoder, _rx)) = open(Codec::H264) {
        let frame = host_core::platform::CapturedFrame {
            display_id: 0,
            width: 1280,
            height: 720,
            capture_ts_us: unix_micros(),
            native: std::sync::Arc::new(()),
        };
        assert!(matches!(
            encoder.encode(frame, EncodeOpts::default()),
            Err(PlatformError::Platform(_))
        ));
        assert!(matches!(
            encoder.encode(
                synthetic_frame(640, 360, 1, unix_micros()),
                EncodeOpts::default()
            ),
            Err(PlatformError::Platform(_))
        ));
        let stats = encoder.finish();
        assert_eq!(stats, encoder.finish(), "finish must be idempotent");
        assert_eq!(stats, encoder.stats());
        assert!(matches!(
            encoder.encode(
                synthetic_frame(1280, 720, 2, unix_micros()),
                EncodeOpts::default()
            ),
            Err(PlatformError::Platform(_))
        ));
    }
}

// DataRateLimits covers the VT compressed sample in decode time. Annex-B
// prepends parameter sets from the format description, so exclude those when
// comparing with the VT byte budget. Each remaining 4-byte start code replaces
// VT's 4-byte NAL length. No reordering means capture PTS is also decode order.
fn vt_sample_bytes(frame: &EncodedFrame, codec: Codec) -> u64 {
    let starts: Vec<_> = frame
        .data
        .windows(4)
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == [0, 0, 0, 1]).then_some(index))
        .collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(frame.data.len());
            let kind = nal_types(&frame.data[*start..end], codec)[0];
            let parameter_set = match codec {
                Codec::Hevc => matches!(kind, 32..=34),
                Codec::H264 => matches!(kind, 7 | 8),
                _ => false,
            };
            (!parameter_set).then_some((end - start) as u64)
        })
        .sum()
}

#[test]
fn noise_is_counted_under_the_hard_rate_limit_and_keyframes_recover() {
    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    const KBPS: u32 = 4_000;
    const COUNT: u32 = 180;
    for codec in [Codec::Hevc, Codec::H264] {
        let Some((mut encoder, mut rx)) = open_rate(codec, KBPS) else {
            continue;
        };
        let start = Instant::now();
        let base = unix_micros();
        let mut frames = Vec::new();
        for index in 0..COUNT {
            let offset = u64::from(index) * 1_000_000 / 60;
            let frame = synthetic_frame_pattern(1280, 720, index, base + offset, 1);
            std::thread::sleep(
                (start + Duration::from_micros(offset)).saturating_duration_since(Instant::now()),
            );
            encoder
                .encode(frame, EncodeOpts::default())
                .expect("noise submission must not fail");
            frames.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        }
        assert_eq!(encoder.stats().forced_key_retries, 0);
        // Request while the byte budget is under pressure. Keep submitting
        // changing noise at 60 fps and retain the hard limit throughout. A
        // dropped forced attempt must be retried without a new API request.
        let key_offset = u64::from(COUNT) * 1_000_000 / 60;
        let mut requested_us = 0;
        let mut deadline = Instant::now();
        for index in COUNT..COUNT + 60 {
            let offset = u64::from(index) * 1_000_000 / 60;
            let frame = synthetic_frame_pattern(1280, 720, index, base + offset, 1);
            std::thread::sleep(
                (start + Duration::from_micros(offset)).saturating_duration_since(Instant::now()),
            );
            if index == COUNT {
                requested_us = unix_micros();
                deadline = Instant::now() + Duration::from_secs(1);
            }
            encoder
                .encode(
                    frame,
                    EncodeOpts {
                        force_keyframe: index == COUNT,
                        quality_boost: false,
                    },
                )
                .expect("continue noise after one forced-keyframe request");
            frames.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        }
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        let stats = encoder.finish();
        frames.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        assert_no_encoder_errors(stats);
        assert_eq!(stats.submitted, u64::from(COUNT) + 60, "{stats:?}");
        assert_eq!(stats.output, frames.len() as u64, "{stats:?}");
        assert!(
            stats.dropped > 0,
            "stress source must exercise drops: {stats:?}"
        );
        assert_eq!(stats.submitted, stats.output + stats.dropped, "{stats:?}");
        let key = frames
            .iter()
            .filter(|frame| frame.keyframe && frame.capture_ts_us >= base + key_offset)
            .min_by_key(|frame| frame.encoded_ts_us)
            .unwrap_or_else(|| panic!("forced noise keyframe missing within 1 second: {stats:?}"));
        let recovery_us = key.encoded_ts_us.saturating_sub(requested_us);
        assert!(
            recovery_us <= 1_000_000,
            "keyframe callback missed the 1 second deadline: {recovery_us} us; {stats:?}"
        );
        assert!(!key.still_refine);
        let failed_before_key = (COUNT..COUNT + 60)
            .map(|index| base + u64::from(index) * 1_000_000 / 60)
            .take_while(|stamp| *stamp < key.capture_ts_us)
            .count() as u64;
        // Every preceding recovery submission failed to produce a keyframe;
        // asynchronous attempts already in flight can add further retries.
        assert!(stats.forced_key_retries >= failed_before_key, "{stats:?}");
        let failed_recovery = 60
            - frames
                .iter()
                .filter(|frame| frame.keyframe && frame.capture_ts_us >= base + key_offset)
                .count() as u64;
        assert!(stats.forced_key_retries <= failed_recovery, "{stats:?}");
        let samples: Vec<_> = frames
            .iter()
            .map(|frame| (frame.capture_ts_us, vt_sample_bytes(frame, codec)))
            .collect();
        let limit_bytes = data_rate_limits(KBPS).0;
        let peak_bytes = samples
            .iter()
            .map(|(end, _)| {
                samples
                    .iter()
                    .filter(|(stamp, _)| *stamp <= *end && end - stamp < 1_000_000)
                    .map(|(_, bytes)| bytes)
                    .sum::<u64>()
            })
            .max()
            .unwrap_or(0);
        let overshoot_pct =
            peak_bytes.saturating_sub(limit_bytes) as f64 * 100.0 / limit_bytes as f64;
        // Dispatcher measurements: 1080p noise + forced keyframes, target 4000 kbps,
        // hard cap 1.5x: HEVC +2.4%, H.264 +18.58%; allow 5% / 25% respectively.
        // H.264 rate-limit compensation is deferred to.
        let tolerance_pct = if codec == Codec::H264 { 25 } else { 5 };
        assert!(
            peak_bytes <= limit_bytes + limit_bytes * tolerance_pct / 100,
            "hard cap exceeded: peak_1s_bytes={peak_bytes}, cap={limit_bytes}, overshoot_pct={overshoot_pct:.2}%, tolerance_pct={tolerance_pct}%, stats={stats:?}"
        );
        eprintln!(
            "noise {codec:?}: stats={stats:?}, key_recovery_us={recovery_us}, failed_before_key={failed_before_key}, peak_1s_kbps={}, cap_kbps={}",
            peak_bytes as f64 * 0.008,
            f64::from(KBPS) * 1.5
        );
    }
}

#[test]
fn pure_rate_limits_refine_bump_and_annex_b_parsing() {
    assert_eq!(data_rate_limits(8_000), (1_500_000, 1.0));
    assert_eq!(data_rate_limits(0), (0, 1.0));
    assert_eq!(data_rate_limits(u32::MAX).0, 805_306_367_812);
    assert_eq!(refine_bump(8_000), 16_000);
    assert_eq!(refine_bump(u32::MAX), u32::MAX);
    let hevc = [
        0, 0, 0, 1, 64, 1, 0, 0, 1, 66, 1, 0, 0, 0, 1, 68, 1, 0, 0, 1, 38, 1,
    ];
    assert_eq!(nal_types(&hevc, Codec::Hevc), [32, 33, 34, 19]);
    let h264 = [0, 0, 1, 0x67, 0, 0, 0, 1, 0x68, 0, 0, 1, 0x65];
    assert_eq!(nal_types(&h264, Codec::H264), [7, 8, 5]);
    for empty in [&[][..], &[0, 0], &[0, 0, 0, 1], &[0, 0, 1, 64]] {
        assert!(nal_types(empty, Codec::Hevc).is_empty());
    }
    assert!(nal_types(&hevc, Codec::Unspecified).is_empty());
}

#[test]
fn churn_grid_is_deterministic_and_bounded() {
    assert_eq!(churn_block_count(128, 128), 4);
    assert_eq!(churn_blocks(128, 128, 7, 0), Vec::<(u32, u32)>::new());
    assert_eq!(churn_blocks(128, 128, 7, 100).len(), 4);
    let first = churn_blocks(1920, 1080, 123, 37);
    assert_eq!(first, churn_blocks(1920, 1080, 123, 37));
    assert_eq!(
        first.len(),
        (churn_block_count(1920, 1080) * 37).div_ceil(100) as usize
    );
    assert!(first.iter().all(|(x, y)| *x < 30 && *y < 17));
    assert_ne!(first, churn_blocks(1920, 1080, 124, 37));
}

#[test]
fn churn_selection_binary_searches_the_lowest_qualifying_level() {
    assert_eq!(select_churn(|value| value >= 37), Some(37));
    assert_eq!(select_churn(|_| false), None);
    assert_eq!(select_churn(|_| true), Some(0));
    assert_eq!(
        rate_limit_pairs(4_000, RateLimitScheme::Current),
        vec![(750_000, 1.0)]
    );
    assert_eq!(
        rate_limit_pairs(4_000, RateLimitScheme::Tight12),
        vec![(600_000, 1.0)]
    );
    assert_eq!(
        rate_limit_pairs(4_000, RateLimitScheme::Short025),
        vec![(750_000, 1.0), (187_500, 0.25)]
    );
}

#[test]
fn psnr_is_finite_for_loss_and_infinite_for_identical_luma() {
    assert!(psnr_db(&[1, 2, 3], &[1, 2, 3]).is_some_and(f64::is_infinite));
    let value = psnr_db(&[0, 255], &[1, 254]).unwrap_or_default();
    assert!(value > 40.0);
    assert_eq!(psnr_db(&[], &[]), None);
    assert_eq!(psnr_db(&[1], &[1, 2]), None);
}

fn step_samples(from: u64, to: u64, transient: bool) -> StepMeter {
    let mut meter = StepMeter::new();
    for index in 0..220u64 {
        let bytes = if index < 100 || (transient && (110..150).contains(&index)) {
            from
        } else {
            to
        };
        meter.push(index * 10_000, bytes);
    }
    meter
}

#[test]
fn step_meter_checks_both_directions_and_requires_a_continuous_hold() {
    let down = step_samples(25_000, 5_000, false);
    assert_eq!(down.window_kbps(900_000), 20_000.0);
    assert_eq!(down.window_kbps(1_200_000), 4_000.0);
    assert_eq!(down.settle_ms(1_000_000, 4_000), Some(90));
    let up = step_samples(5_000, 25_000, false);
    assert_eq!(up.settle_ms(1_000_000, 20_000), Some(60));
    assert_eq!(
        step_samples(25_000, 5_000, true).settle_ms(1_000_000, 4_000),
        Some(590)
    );
    assert_eq!(
        down.settle_ms(2_100_000, 4_000),
        None,
        "insufficient observation after change"
    );
    let mut gaps = StepMeter::new();
    gaps.push(0, 50_000);
    gaps.push(400_000, 50_000);
    assert_eq!(gaps.window_kbps(0), 4_000.0);
    assert_eq!(gaps.window_kbps(100_000), 0.0);
    assert_eq!(
        gaps.settle_ms(0, 4_000),
        None,
        "expiring bytes must break a stable hold even without a callback"
    );
    assert_eq!(gaps.settle_ms(0, 0), None);
    assert_eq!(StepMeter::new().settle_ms(0, 4_000), None);
}

#[test]
fn synthetic_patterns_separate_local_motion_from_temporal_noise() {
    extern "C" {
        fn rs_test_pixel_buffer_luma(
            width: u32,
            height: u32,
            seed: u32,
            pattern: u32,
            out: *mut u8,
            len: usize,
        ) -> i32;
    }

    fn luma(seed: u32, pattern: u32) -> Vec<u8> {
        let mut pixels = vec![0; 640 * 360];
        // SAFETY: the shim validates the dimensions and output size, and
        // receives an exclusively owned, writable buffer of the stated size.
        assert_eq!(
            unsafe {
                rs_test_pixel_buffer_luma(
                    640,
                    360,
                    seed,
                    pattern,
                    pixels.as_mut_ptr(),
                    pixels.len(),
                )
            },
            0
        );
        assert!(pixels.iter().all(|value| (16..=235).contains(value)));
        pixels
    }

    let changed = |a: &[u8], b: &[u8]| {
        a.iter().zip(b).filter(|(a, b)| a != b).count() as f64 / a.len() as f64
    };
    let smooth = luma(123, 0);
    let noise = luma(123, 1);
    assert_eq!(smooth, luma(123, 0));
    assert_eq!(noise, luma(123, 1));
    let smooth_change = changed(&smooth, &luma(124, 0));
    let noise_change = changed(&noise, &luma(124, 1));
    assert!(smooth_change > 0.0 && smooth_change < 0.05);
    assert!(noise_change > 0.9);
    assert_ne!(smooth, noise);
    eprintln!(
        "adjacent-frame changed luma: smooth={:.3}%, noise={:.3}% (640x360, seeds 123/124)",
        smooth_change * 100.0,
        noise_change * 100.0
    );
    let mut short = [255; 2];
    // SAFETY: the output is valid for exactly two bytes, so the shim must
    // reject it before writing a larger image.
    assert_eq!(
        unsafe { rs_test_pixel_buffer_luma(640, 360, 0, 0, short.as_mut_ptr(), short.len()) },
        -1
    );
    assert_eq!(short, [255; 2]);
}

#[test]
fn churn_luma_is_reproducible_and_changes_only_selected_blocks() {
    extern "C" {
        fn rs_test_pixel_buffer_luma_churn(
            width: u32,
            height: u32,
            seed: u32,
            churn_percent: u32,
            out: *mut u8,
            len: usize,
        ) -> i32;
    }
    fn luma(seed: u32, churn: u32) -> Vec<u8> {
        let mut output = vec![0u8; 128 * 128];
        // SAFETY: output has exactly width*height bytes and the shim validates
        // the even dimensions and churn percentage before writing.
        assert_eq!(
            unsafe {
                rs_test_pixel_buffer_luma_churn(
                    128,
                    128,
                    seed,
                    churn,
                    output.as_mut_ptr(),
                    output.len(),
                )
            },
            0
        );
        output
    }
    let fixed = luma(1, 0);
    assert_eq!(fixed, luma(2, 0));
    let first = luma(123, 25);
    assert_eq!(first, luma(123, 25));
    assert_ne!(first, luma(124, 25));
    let changed = first
        .iter()
        .zip(&fixed)
        .filter(|(left, right)| left != right)
        .count();
    assert!(
        changed >= 3 * 64 * 64 / 4,
        "one 64x64 block must be redrawn"
    );
    assert!(
        changed <= 2 * 64 * 64,
        "25% of a four-block grid selects one block"
    );
}

#[test]
fn churn_texture_modes_are_reproducible_and_distinct() {
    let gradient = encode::synthetic_luma_churn_texture(128, 128, 9, 50, ChurnTexture::Gradient)
        .unwrap_or_default();
    let noise = encode::synthetic_luma_churn_texture(128, 128, 9, 50, ChurnTexture::Noise)
        .unwrap_or_default();
    assert_eq!(
        gradient,
        encode::synthetic_luma_churn_texture(128, 128, 9, 50, ChurnTexture::Gradient)
            .unwrap_or_default()
    );
    assert_eq!(
        noise,
        encode::synthetic_luma_churn_texture(128, 128, 9, 50, ChurnTexture::Noise)
            .unwrap_or_default()
    );
    assert_ne!(gradient, noise);
    assert_eq!(ChurnTexture::parse("smooth"), Some(ChurnTexture::Gradient));
    assert_eq!(ChurnTexture::parse("noise"), Some(ChurnTexture::Noise));
}

#[test]
fn synthetic_buffers_are_iosurface_nv12_with_deterministic_moving_content() {
    use std::ffi::c_void;

    let _lock = ENCODE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _env = DisableHevcGuard::set(false);
    // IOSurface allocation is also restricted in the VT sandbox. Use the
    // card's encoder-availability skip rule before testing native buffers.
    let Some((_encoder, _rx)) = open(Codec::H264) else {
        return;
    };

    extern "C" {
        fn rs_test_pixel_buffer_new(width: u32, height: u32, seed: u32) -> *mut c_void;
        fn rs_test_pixel_buffer_new_pattern(
            width: u32,
            height: u32,
            seed: u32,
            pattern: u32,
        ) -> *mut c_void;
        fn rs_pixel_buffer_release(pixel: *mut c_void);
        fn CVPixelBufferGetWidth(pixel: *mut c_void) -> usize;
        fn CVPixelBufferGetHeight(pixel: *mut c_void) -> usize;
        fn CVPixelBufferGetPixelFormatType(pixel: *mut c_void) -> u32;
        fn CVPixelBufferGetIOSurface(pixel: *mut c_void) -> *mut c_void;
        fn CVPixelBufferLockBaseAddress(pixel: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(pixel: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddressOfPlane(pixel: *mut c_void, plane: usize) -> *const u8;
        fn CVPixelBufferGetBytesPerRowOfPlane(pixel: *mut c_void, plane: usize) -> usize;
    }

    struct Buffer(*mut c_void);
    impl Drop for Buffer {
        fn drop(&mut self) {
            // SAFETY: new() returned one retained pixel buffer to this owner.
            unsafe { rs_pixel_buffer_release(self.0) };
        }
    }
    fn picture(seed: u32, pattern: u32) -> Vec<u8> {
        // SAFETY: valid, even dimensions; a successful call returns a retained buffer.
        let buffer = Buffer(unsafe { rs_test_pixel_buffer_new_pattern(64, 32, seed, pattern) });
        assert!(
            !buffer.0.is_null(),
            "allocate synthetic buffer without creating an encoder"
        );
        // SAFETY: the buffer is live and these CoreVideo queries do not mutate it.
        unsafe {
            assert_eq!(CVPixelBufferGetWidth(buffer.0), 64);
            assert_eq!(CVPixelBufferGetHeight(buffer.0), 32);
            assert_eq!(
                CVPixelBufferGetPixelFormatType(buffer.0),
                u32::from_be_bytes(*b"420v")
            );
            assert!(!CVPixelBufferGetIOSurface(buffer.0).is_null());
            assert_eq!(CVPixelBufferLockBaseAddress(buffer.0, 1), 0);
        }
        // SAFETY: the read-only base-address lock protects 32 rows of 64 luma
        // bytes. Only active columns are copied, excluding any stride padding.
        let luma = unsafe {
            let base = CVPixelBufferGetBaseAddressOfPlane(buffer.0, 0);
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer.0, 0);
            (0..32)
                .flat_map(|row| std::slice::from_raw_parts(base.add(row * stride), 64).to_vec())
                .collect::<Vec<_>>()
        };
        // SAFETY: releases the read-only lock just acquired on this live buffer.
        assert_eq!(unsafe { CVPixelBufferUnlockBaseAddress(buffer.0, 1) }, 0);
        assert!(luma.iter().all(|value| (16..=235).contains(value)));
        luma
    }
    for pattern in 0..=1 {
        assert_eq!(picture(123, pattern), picture(123, pattern));
        assert_ne!(picture(123, pattern), picture(124, pattern));
    }
    // SAFETY: invalid dimensions must return NULL before allocating anything.
    assert!(unsafe { rs_test_pixel_buffer_new(1, 32, 0) }.is_null());
}
