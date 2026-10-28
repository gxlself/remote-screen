//! Synthetic VideoToolbox probes. stdout is exactly one JSON object per run.

use std::collections::HashSet;
use std::env;
use std::time::Duration;

use host_core::platform::{EncodeOpts, EncodedFrame, Encoder, EncoderConfig, PlatformError};
use host_mac::encode::{self, ChurnTexture, EncoderInfo, EncoderStats, RateLimitScheme, StepMeter};
use rsp_core::{clock::unix_micros, pb::Codec};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub fn requested() -> bool {
    env::args().any(|arg| {
        matches!(
            arg.as_str(),
            "--encode-bench"
                | "--encode-step"
                | "--refine"
                | "--calibrate"
                | "--h264-experiment"
                | "--rate-limit-diagnosis"
        )
    })
}

#[derive(Clone, Copy)]
enum Mode {
    Bench,
    Step(u32, u32),
    Refine,
    H264Experiment,
    RateLimitDiagnosis,
}

#[derive(Clone)]
struct Args {
    mode: Mode,
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
    kbps: u32,
    seconds: u64,
    at: u64,
    pattern: Pattern,
    calibrate: bool,
    h264_limit: RateLimitScheme,
    texture: ChurnTexture,
    disable_data_rate_limits: bool,
    refine_relax_data_rate_limits: bool,
}

#[derive(Clone, Copy)]
enum Pattern {
    Smooth,
    Noise,
    Churn(u8),
}

impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::Noise => "noise",
            Self::Churn(_) => "churn",
        }
    }

    fn churn(self) -> Option<u8> {
        match self {
            Self::Churn(value) => Some(value),
            Self::Smooth | Self::Noise => None,
        }
    }
}

struct ProbeError {
    code: &'static str,
    message: String,
    stats: Option<Box<EncoderStats>>,
    info: Option<EncoderInfo>,
    pattern: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalibrationClass {
    Qualified,
    TooLowOutput,
    TooManyDrops,
    Uncalibratable,
}

impl CalibrationClass {
    fn fields(self) -> (&'static str, &'static str) {
        match self {
            Self::Qualified => ("qualified", "lower_churn"),
            Self::TooLowOutput => ("too_low_output", "increase_churn"),
            Self::TooManyDrops => ("too_many_drops", "decrease_churn"),
            Self::Uncalibratable => ("uncalibratable", "stop"),
        }
    }
}

fn classify_calibration(output_ok: bool, drop_ok: bool) -> CalibrationClass {
    match (output_ok, drop_ok) {
        (true, true) => CalibrationClass::Qualified,
        (false, true) => CalibrationClass::TooLowOutput,
        (true, false) => CalibrationClass::TooManyDrops,
        (false, false) => CalibrationClass::Uncalibratable,
    }
}

struct H264LimitGuard(Option<std::ffi::OsString>);

impl H264LimitGuard {
    fn set(scheme: RateLimitScheme) -> Self {
        let old = env::var_os("RS_HOST_H264_LIMIT");
        // SAFETY: the probe is single-threaded at the process level and the
        // guard spans encoder creation plus all property updates for a run.
        unsafe {
            if scheme == RateLimitScheme::Current {
                env::remove_var("RS_HOST_H264_LIMIT");
            } else {
                env::set_var("RS_HOST_H264_LIMIT", scheme.name());
            }
        }
        Self(old)
    }
}

impl Drop for H264LimitGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is dropped after the run's encoder has finished.
        unsafe {
            match &self.0 {
                Some(value) => env::set_var("RS_HOST_H264_LIMIT", value),
                None => env::remove_var("RS_HOST_H264_LIMIT"),
            }
        }
    }
}

struct DataRateLimitsGuard(Option<std::ffi::OsString>);

impl DataRateLimitsGuard {
    fn set(disabled: bool) -> Self {
        let old = env::var_os("RS_HOST_DISABLE_DATA_RATE_LIMITS");
        // SAFETY: the probe uses a current-thread runtime and holds this guard
        // for all native property updates in the associated encoder run.
        unsafe {
            if disabled {
                env::set_var("RS_HOST_DISABLE_DATA_RATE_LIMITS", "1");
            } else {
                env::remove_var("RS_HOST_DISABLE_DATA_RATE_LIMITS");
            }
        }
        Self(old)
    }
}

impl Drop for DataRateLimitsGuard {
    fn drop(&mut self) {
        // SAFETY: the encoder run has completed before this guard is dropped.
        unsafe {
            match &self.0 {
                Some(value) => env::set_var("RS_HOST_DISABLE_DATA_RATE_LIMITS", value),
                None => env::remove_var("RS_HOST_DISABLE_DATA_RATE_LIMITS"),
            }
        }
    }
}

impl ProbeError {
    fn arguments(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_arguments",
            message: message.into(),
            stats: None,
            info: None,
            pattern: None,
        }
    }
}

impl From<PlatformError> for ProbeError {
    fn from(error: PlatformError) -> Self {
        Self {
            code: if matches!(error, PlatformError::Unsupported) {
                "unsupported"
            } else {
                "platform"
            },
            message: error.to_string(),
            stats: None,
            info: None,
            pattern: None,
        }
    }
}

fn parse_args() -> Result<Args, ProbeError> {
    let mut args = Args {
        mode: Mode::Bench,
        codec: Codec::Hevc,
        width: 1920,
        height: 1080,
        fps: 60,
        kbps: 8_000,
        seconds: 5,
        at: 2,
        pattern: Pattern::Smooth,
        calibrate: false,
        h264_limit: RateLimitScheme::Current,
        texture: ChurnTexture::Noise,
        disable_data_rate_limits: false,
        refine_relax_data_rate_limits: false,
    };
    let mut selected = false;
    let mut explicit_pattern = false;
    let mut explicit_churn = false;
    let mut explicit_texture = false;
    let mut churn_value = 0u8;
    let mut values = env::args().skip(1);
    while let Some(flag) = values.next() {
        if matches!(
            flag.as_str(),
            "--encode-bench"
                | "--encode-step"
                | "--refine"
                | "--h264-experiment"
                | "--rate-limit-diagnosis"
        ) {
            if selected {
                return Err(ProbeError::arguments("select one encoder probe mode"));
            }
            selected = true;
            args.mode = match flag.as_str() {
                "--encode-step" => {
                    let value = required_value(&mut values, &flag)?;
                    let (from, to) = value.split_once(':').ok_or_else(|| {
                        ProbeError::arguments("--encode-step requires OLD:NEW kbps")
                    })?;
                    Mode::Step(parse_number(from, &flag)?, parse_number(to, &flag)?)
                }
                "--refine" => Mode::Refine,
                "--h264-experiment" => Mode::H264Experiment,
                "--rate-limit-diagnosis" => Mode::RateLimitDiagnosis,
                _ => Mode::Bench,
            };
            continue;
        }
        if flag == "--calibrate" {
            args.calibrate = true;
            continue;
        }
        if flag == "--no-data-rate-limits" {
            args.disable_data_rate_limits = true;
            continue;
        }
        if flag == "--refine-no-data-rate-limits" {
            args.refine_relax_data_rate_limits = true;
            continue;
        }
        let value = required_value(&mut values, &flag)?;
        match flag.as_str() {
            "--codec" => {
                args.codec = match value.as_str() {
                    "hevc" => Codec::Hevc,
                    "h264" => Codec::H264,
                    _ => return Err(ProbeError::arguments("--codec must be hevc or h264")),
                }
            }
            "--size" => {
                let (width, height) = value
                    .split_once('x')
                    .ok_or_else(|| ProbeError::arguments("--size requires WIDTHxHEIGHT"))?;
                args.width = parse_number(width, &flag)?;
                args.height = parse_number(height, &flag)?;
            }
            "--fps" => args.fps = parse_number(&value, &flag)?,
            "--kbps" => args.kbps = parse_number(&value, &flag)?,
            "--seconds" => args.seconds = parse_number(&value, &flag)?,
            "--at" => args.at = parse_number(&value, &flag)?,
            "--pattern" => {
                explicit_pattern = true;
                args.pattern = match value.as_str() {
                    "smooth" => Pattern::Smooth,
                    "noise" => Pattern::Noise,
                    "churn" => Pattern::Churn(churn_value),
                    _ => {
                        return Err(ProbeError::arguments(
                            "--pattern must be smooth, noise, or churn",
                        ))
                    }
                }
            }
            "--churn" => {
                let churn: u8 = parse_number(&value, &flag)?;
                if churn > 100 {
                    return Err(ProbeError::arguments("--churn must be 0..100"));
                }
                explicit_churn = true;
                churn_value = churn;
                args.pattern = Pattern::Churn(churn);
            }
            "--h264-limit" => {
                args.h264_limit = RateLimitScheme::parse(&value).ok_or_else(|| {
                    ProbeError::arguments("--h264-limit must be current, tight12, or short025")
                })?;
            }
            "--texture" => {
                explicit_texture = true;
                args.texture = ChurnTexture::parse(&value)
                    .ok_or_else(|| ProbeError::arguments("--texture must be gradient or noise"))?;
            }
            _ => {
                return Err(ProbeError::arguments(format!(
                    "unknown encoder probe argument: {flag}"
                )))
            }
        }
    }
    if !explicit_pattern && !explicit_churn && matches!(args.mode, Mode::Step(..) | Mode::Refine) {
        args.pattern = Pattern::Churn(0);
        args.calibrate = true;
    } else if matches!(args.pattern, Pattern::Churn(0)) && !explicit_churn && !args.calibrate {
        // An explicit --pattern churn without --churn requests calibration;
        // an explicit nonzero --churn remains a fixed experimental setting.
        args.calibrate = true;
    }
    if !explicit_texture
        && matches!(
            args.mode,
            Mode::Step(..) | Mode::Refine | Mode::H264Experiment
        )
    {
        // The default for rate-response, refinement and H.264 limit work is a
        // compressible block texture. Stress/noise remains opt-in.
        args.texture = ChurnTexture::Gradient;
    }
    if !(1..=120).contains(&args.fps) || args.seconds == 0 || args.seconds > 300 || args.at > 298 {
        return Err(ProbeError::arguments(
            "fps must be 1..120; seconds 1..300; at 0..298",
        ));
    }
    if args.width < 2
        || args.height < 2
        || args.width > 16384
        || args.height > 16384
        || !args.width.is_multiple_of(2)
        || !args.height.is_multiple_of(2)
    {
        return Err(ProbeError::arguments(
            "size must be even, within 2..16384 pixels",
        ));
    }
    let valid_rate = |kbps| (1..=150_000).contains(&kbps);
    if !valid_rate(args.kbps)
        || matches!(args.mode, Mode::Step(from, to) if !valid_rate(from) || !valid_rate(to))
    {
        return Err(ProbeError::arguments("bitrate must be 1..150000 kbps"));
    }
    Ok(args)
}

fn required_value(
    values: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, ProbeError> {
    values
        .next()
        .ok_or_else(|| ProbeError::arguments(format!("missing value for {flag}")))
}

fn parse_number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, ProbeError> {
    value
        .parse()
        .map_err(|_| ProbeError::arguments(format!("invalid number for {flag}: {value}")))
}

fn info_json(info: EncoderInfo) -> Value {
    json!({
        "codec": if info.codec == Codec::H264 { "h264" } else { "hevc" },
        "low_latency": info.low_latency,
        "hardware": info.hardware,
        "quality_supported": info.quality_supported,
    })
}

fn stats_json(stats: EncoderStats) -> Value {
    json!({
        "submitted": stats.submitted,
        "output": stats.output,
        "dropped": stats.dropped,
        "callback_error": stats.callback_error,
        "submit_error": stats.submit_error,
        "callback_status_error": stats.callback_status_error,
        "unexpected_null": stats.unexpected_null,
        "conversion_error": stats.conversion_error,
        "forced_key_retries": stats.forced_key_retries,
    })
}

struct FrameSample {
    capture_us: u64,
    output_us: u64,
    bytes: usize,
    data: Vec<u8>,
    keyframe: bool,
    still_refine: bool,
    nal_types: Vec<u8>,
}

struct Submission {
    index: u64,
    capture_us: u64,
    refine: bool,
}

struct Run {
    info: EncoderInfo,
    frames: Vec<FrameSample>,
    submissions: Vec<Submission>,
    started_us: u64,
    elapsed_us: u64,
    changed_us: Option<u64>,
    change_completed_us: Option<u64>,
    change_index: Option<u64>,
    stats: EncoderStats,
}

#[derive(Clone, Copy)]
struct Step {
    target_kbps: u32,
    window_seconds: f64,
    force_keyframe: bool,
}

fn collect(rx: &mut mpsc::Receiver<EncodedFrame>, codec: Codec, samples: &mut Vec<FrameSample>) {
    while let Ok(frame) = rx.try_recv() {
        let types = if frame.keyframe || frame.still_refine {
            encode::nal_types(&frame.data, codec)
        } else {
            Vec::new()
        };
        samples.push(FrameSample {
            capture_us: frame.capture_ts_us,
            output_us: frame.encoded_ts_us,
            bytes: frame.data.len(),
            data: frame.data,
            keyframe: frame.keyframe,
            still_refine: frame.still_refine,
            nal_types: types,
        });
    }
}

async fn run_stream(
    args: &Args,
    kbps: u32,
    step: Option<Step>,
    refine: bool,
    trigger_refine: bool,
) -> Result<Run, ProbeError> {
    let _h264_limit = H264LimitGuard::set(args.h264_limit);
    let _data_rate_limits = DataRateLimitsGuard::set(args.disable_data_rate_limits);
    let (tx, mut rx) = mpsc::channel(1024);
    let (mut encoder, info) = encode::encoder(EncoderConfig {
        codec: args.codec,
        width: args.width,
        height: args.height,
        fps: args.fps,
        bitrate_kbps: kbps,
        sink: tx,
    })
    .map_err(|error| {
        let mut error = ProbeError::from(error);
        error.pattern = Some(args.pattern.name());
        error
    })?;
    let seconds = if step.is_some() {
        args.seconds.max(args.at + 2)
    } else {
        args.seconds
    };
    let count = if refine {
        61
    } else {
        seconds * u64::from(args.fps)
    };
    let mut run = Run {
        info,
        frames: Vec::with_capacity(count as usize),
        submissions: Vec::with_capacity(count as usize),
        started_us: unix_micros(),
        elapsed_us: 0,
        changed_us: None,
        change_completed_us: None,
        change_index: None,
        stats: EncoderStats::default(),
    };
    let start = Instant::now();
    let mut last_picture = None;
    let mut last_pts = 0;
    for index in 0..count {
        collect(&mut rx, args.codec, &mut run.frames);
        // Generate outside the measured encode interval. Only the pixel owner
        // is reused for still frames; every submission gets a fresh Unix PTS.
        let mut frame = if refine && index >= 30 {
            last_picture
                .clone()
                .ok_or_else(|| ProbeError::arguments("missing synthetic still picture"))?
        } else {
            match args.pattern {
                Pattern::Smooth => {
                    encode::synthetic_frame_pattern(args.width, args.height, index as u32, 0, 0)
                }
                Pattern::Noise => {
                    encode::synthetic_frame_pattern(args.width, args.height, index as u32, 0, 1)
                }
                Pattern::Churn(churn) => encode::synthetic_frame_churn_texture(
                    args.width,
                    args.height,
                    index as u32,
                    0,
                    churn,
                    args.texture,
                ),
            }
        };
        if refine && index == 29 {
            last_picture = Some(frame.clone());
        }
        let due = start + Duration::from_secs_f64(index as f64 / f64::from(args.fps));
        tokio::time::sleep_until(due).await;
        let mut force = false;
        if let Some(step) = step {
            if run.changed_us.is_none() && start.elapsed() >= Duration::from_secs(args.at) {
                run.changed_us = Some(unix_micros());
                encoder.set_rate_window(step.target_kbps, args.fps, step.window_seconds);
                run.change_completed_us = Some(unix_micros());
                run.change_index = Some(index);
                force = step.force_keyframe;
            }
        }
        frame.capture_ts_us = unix_micros().max(last_pts + 1);
        last_pts = frame.capture_ts_us;
        run.submissions.push(Submission {
            index,
            capture_us: last_pts,
            refine: refine && trigger_refine && index == 30,
        });
        let _refine_limit =
            if refine && trigger_refine && index == 30 && args.refine_relax_data_rate_limits {
                Some(DataRateLimitsGuard::set(true))
            } else {
                None
            };
        let encode_result = encoder.encode(
            frame,
            EncodeOpts {
                force_keyframe: force,
                quality_boost: refine && trigger_refine && index == 30,
            },
        );
        drop(_refine_limit);
        if let Err(error) = encode_result {
            let stats = encoder.finish();
            return Err(ProbeError {
                code: "platform",
                message: format!("{error}; stats={stats:?}"),
                stats: Some(Box::new(stats)),
                info: Some(info),
                pattern: Some(args.pattern.name()),
            });
        }
        collect(&mut rx, args.codec, &mut run.frames);
    }
    // Include the final frame interval in throughput, then drain VT once. A
    // per-frame flush would distort both latency and the rate-control response.
    tokio::time::sleep_until(start + Duration::from_secs_f64(count as f64 / f64::from(args.fps)))
        .await;
    run.stats = encoder.finish();
    collect(&mut rx, args.codec, &mut run.frames);
    run.elapsed_us = start.elapsed().as_micros() as u64;
    // Stress probes measure output throughput even when VT enforces its cap
    // by dropping frames. Step probes also retain smooth drop diagnostics.
    // Exact frame counts remain mandatory for smooth bench/refine and tests.
    let output_pts: HashSet<_> = run.frames.iter().map(|frame| frame.capture_us).collect();
    let consistent = run.stats.submitted == count
        && run.stats.output == run.frames.len() as u64
        && run.stats.output + run.stats.dropped == count
        && output_pts.len() == run.frames.len()
        && run
            .submissions
            .iter()
            .filter(|frame| output_pts.contains(&frame.capture_us))
            .count()
            == run.frames.len()
        && run.stats.submit_error == 0
        && run.stats.callback_status_error == 0
        && run.stats.unexpected_null == 0
        && run.stats.conversion_error == 0;
    let allow_counted_drops =
        step.is_some() || matches!(args.pattern, Pattern::Noise | Pattern::Churn(_));
    if !consistent || (run.frames.len() != count as usize && !allow_counted_drops) {
        return Err(ProbeError {
            code: "missing_frames",
            message: format!(
                "received {} of {count} submitted frames; stats={:?}",
                run.frames.len(),
                run.stats
            ),
            stats: Some(Box::new(run.stats)),
            info: Some(info),
            pattern: Some(args.pattern.name()),
        });
    }
    Ok(run)
}

fn drop_diagnostics(run: &Run) -> Value {
    let output_pts: HashSet<_> = run.frames.iter().map(|frame| frame.capture_us).collect();
    let missing: Vec<_> = run
        .submissions
        .iter()
        .filter(|frame| !output_pts.contains(&frame.capture_us))
        .collect();
    let missing_near_change: Vec<_> = missing
        .iter()
        .filter_map(|frame| {
            let changed = run.changed_us?;
            let offset_us = i128::from(frame.capture_us) - i128::from(changed);
            (offset_us.abs() <= 200_000).then(|| {
                json!({
                    "index": frame.index,
                    "capture_ts_us": frame.capture_us,
                    "offset_from_rate_change_us": offset_us as i64,
                    "rate_change_submission": Some(frame.index) == run.change_index,
                })
            })
        })
        .collect();
    let change_frame = run
        .submissions
        .iter()
        .find(|frame| Some(frame.index) == run.change_index)
        .map(|frame| {
            json!({
                "index": frame.index,
                "capture_ts_us": frame.capture_us,
                "output": output_pts.contains(&frame.capture_us),
                "dropped": !output_pts.contains(&frame.capture_us),
            })
        });
    json!({
        "missing_frames": missing.len(),
        "all_missing_counted_as_dropped": missing.len() as u64 == run.stats.dropped,
        "missing_submission_indices": missing.iter().map(|frame| frame.index).collect::<Vec<_>>(),
        "rate_change_frame": change_frame,
        "missing_within_200ms_of_rate_change": missing_near_change,
    })
}

fn percentile(samples: &mut [u64], percent: usize) -> u64 {
    samples.sort_unstable();
    samples
        .get((samples.len() * percent).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or(0)
}

fn output_kbps(run: &Run) -> f64 {
    let bytes: usize = run.frames.iter().map(|frame| frame.bytes).sum();
    bytes as f64 * 8_000.0 / run.elapsed_us.max(1) as f64
}

fn calibration_target(args: &Args) -> u32 {
    match args.mode {
        Mode::Step(from, to) => from.max(to),
        Mode::Bench | Mode::Refine | Mode::H264Experiment | Mode::RateLimitDiagnosis => args.kbps,
    }
}

fn calibration_args(args: &Args, churn: u8) -> Args {
    let mut trial = args.clone();
    trial.pattern = Pattern::Churn(churn);
    trial.calibrate = false;
    // "Unconstrained" means keep AverageBitRate but remove the byte-window
    // limiter, so a trial's drop count is not caused by the calibration cap.
    trial.disable_data_rate_limits = true;
    trial
}

async fn calibrate_churn(args: &Args) -> Result<(Option<u8>, Value), ProbeError> {
    let target_kbps = calibration_target(args);
    let unconstrained_kbps = target_kbps.saturating_mul(3).min(150_000);
    let mut trials = Vec::new();
    let mut evaluated = Vec::new();

    // Binary search over real encoder runs. A low output asks for more
    // entropy; a high drop rate asks for less. If both fail at once there is
    // no churn-only solution at this texture/size/rate.
    let mut low = 0u8;
    let mut high = 100u8;
    let mut chosen = None;
    let mut terminal_reason = None;
    while low <= high {
        let churn = low + (high - low) / 2;
        if !evaluated.contains(&churn) {
            let trial_args = calibration_args(args, churn);
            let result = run_stream(&trial_args, unconstrained_kbps, None, false, false).await?;
            let output = output_kbps(&result);
            let drop_rate = if result.stats.submitted == 0 {
                1.0
            } else {
                result.stats.dropped as f64 / result.stats.submitted as f64
            };
            let output_ok = output >= f64::from(target_kbps) * 0.9;
            let drop_ok = drop_rate <= 0.05;
            let (classification, direction) = classify_calibration(output_ok, drop_ok).fields();
            let qualifies = output_ok && drop_ok;
            trials.push(json!({
                "churn": churn,
                "texture": args.texture.name(),
                "out_kbps": output,
                "dropped": result.stats.dropped,
                "submitted": result.stats.submitted,
                "drop_rate": drop_rate,
                "output_ok": output_ok,
                "drop_ok": drop_ok,
                "classification": classification,
                "direction": direction,
                "qualifies": qualifies,
                "stats": stats_json(result.stats),
            }));
            evaluated.push(churn);
            if qualifies {
                chosen = Some(churn);
                if churn == 0 {
                    break;
                }
                high = churn - 1;
            } else if churn == 100 {
                break;
            } else if classify_calibration(output_ok, drop_ok) == CalibrationClass::Uncalibratable {
                terminal_reason = Some("both_output_and_drop_constraints_failed");
                break;
            } else if output_ok {
                high = churn - 1;
            } else {
                low = churn + 1;
            }
        } else {
            break;
        }
    }
    // If a duplicate candidate was reached due to an unusual boundary, report
    // it without claiming a successful calibration.
    let report = json!({
        "mode": "calibrate",
        "pattern": "churn",
        "texture": args.texture.name(),
        "target_kbps": target_kbps,
        "calibration_target_kbps": target_kbps,
        "step_low_end_kbps": match args.mode {
            Mode::Step(from, to) => Some(from.min(to)),
            _ => None,
        },
        "step_content_role": "calibrated_high_target_with_low_endpoint_checked_separately",
        "unconstrained_kbps": unconstrained_kbps,
        "selected_churn": chosen,
        "qualified": chosen.is_some(),
        "uncalibratable": terminal_reason.is_some(),
        "terminal_reason": terminal_reason,
        "data_rate_limits_enabled": false,
        "trials": trials,
    });
    Ok((chosen, report))
}

async fn bench(args: &Args) -> Result<Value, ProbeError> {
    let run = run_stream(args, args.kbps, None, false, false).await?;
    let mut latencies: Vec<_> = run
        .frames
        .iter()
        .map(|frame| frame.output_us.saturating_sub(frame.capture_us))
        .collect();
    let bytes: usize = run.frames.iter().map(|frame| frame.bytes).sum();
    Ok(json!({
        "info": info_json(run.info),
        "frames": run.frames.len(),
        "keyframes": run.frames.iter().filter(|frame| frame.keyframe).count(),
        "encode_p50_us": percentile(&mut latencies, 50),
        "encode_p95_us": percentile(&mut latencies, 95),
        "out_kbps": bytes as f64 * 8_000.0 / run.elapsed_us.max(1) as f64,
        "pattern": args.pattern.name(),
        "churn": args.pattern.churn(),
        "texture": matches!(args.pattern, Pattern::Churn(_)).then_some(args.texture.name()),
        "data_rate_limits_enabled": !args.disable_data_rate_limits,
        "refine_data_rate_limits_relaxed": args.refine_relax_data_rate_limits,
        "h264_limit": args.h264_limit.name(),
        "stats": stats_json(run.stats),
        "all_frames_output": run.frames.len() as u64 == run.stats.submitted,
        "drop_diagnostics": drop_diagnostics(&run),
    }))
}

async fn step_once(
    args: &Args,
    from: u32,
    to: u32,
    window_seconds: f64,
    force_keyframe: bool,
) -> Result<Value, ProbeError> {
    let run = run_stream(
        args,
        from,
        Some(Step {
            target_kbps: to,
            window_seconds,
            force_keyframe,
        }),
        false,
        false,
    )
    .await?;
    let changed = run
        .changed_us
        .ok_or_else(|| ProbeError::arguments("probe ended before the bitrate change"))?;
    let mut meter = StepMeter::new();
    for frame in &run.frames {
        meter.push(frame.output_us, frame.bytes as u64);
    }
    let last_us = meter
        .samples()
        .last()
        .map(|(stamp, _)| *stamp)
        .unwrap_or(run.started_us);
    let mut buckets = Vec::new();
    for offset in (0..=last_us.saturating_sub(run.started_us)).step_by(50_000) {
        buckets.push(
            json!({"t_ms": offset / 1_000, "kbps": meter.window_kbps(run.started_us + offset)}),
        );
    }
    let before_us = changed.saturating_sub(run.started_us).min(1_000_000);
    let before_bytes: usize = run
        .frames
        .iter()
        .filter(|frame| frame.output_us < changed && changed - frame.output_us <= before_us)
        .map(|frame| frame.bytes)
        .sum();
    let before_kbps = before_bytes as f64 * 8_000.0 / before_us.max(1) as f64;
    let output_pts: HashSet<_> = run.frames.iter().map(|frame| frame.capture_us).collect();
    let change_index = run.change_index.unwrap_or(u64::MAX);
    let post_submitted = run
        .submissions
        .iter()
        .filter(|frame| frame.index >= change_index)
        .count() as u64;
    let post_dropped = run
        .submissions
        .iter()
        .filter(|frame| frame.index >= change_index && !output_pts.contains(&frame.capture_us))
        .count() as u64;
    Ok(json!({
        "info": info_json(run.info), "from_kbps": from, "to_kbps": to,
        "pattern": args.pattern.name(), "churn": args.pattern.churn(),
        "texture": args.texture.name(),
        "data_rate_limits_enabled": !args.disable_data_rate_limits,
        "h264_limit": args.h264_limit.name(), "stats": stats_json(run.stats),
        "changed_at_ms": changed.saturating_sub(run.started_us) / 1_000,
        "rate_change_started_us": changed,
        "rate_change_completed_us": run.change_completed_us,
        "drop_diagnostics": drop_diagnostics(&run),
        "before_out_kbps": before_kbps,
        "source_reaches_initial_target": before_kbps >= f64::from(from) * 0.7,
        "step_low_end_kbps": from.min(to),
        "content_role": "high_target_calibrated_and_low_endpoint_measured",
        "post_change_submitted": post_submitted,
        "post_change_dropped": post_dropped,
        "post_change_drop_rate": post_dropped as f64 / post_submitted.max(1) as f64,
        "window_seconds": window_seconds, "forced_keyframe": force_keyframe,
        "settle_ms": meter.settle_ms(changed, to), "samples": buckets,
    }))
}

async fn step_direction(args: &Args, from: u32, to: u32) -> Result<Value, ProbeError> {
    let mut attempts = vec![step_once(args, from, to, 1.0, false).await?];
    let settled = |attempt: &Value| {
        attempt["settle_ms"]
            .as_u64()
            .is_some_and(|value| value <= 200)
    };
    if !settled(&attempts[0]) {
        attempts.push(step_once(args, from, to, 0.5, false).await?);
        if !settled(&attempts[1]) && to < from {
            attempts.push(step_once(args, from, to, 0.5, true).await?);
        }
    }
    let last = attempts[attempts.len() - 1].clone();
    let upward = to > from;
    let meets = settled(&last);
    let mut distribution = Vec::new();
    if upward && !meets {
        // ABR can take longer than the 200 ms acceptance threshold. Keep the
        // criterion unchanged and report five independent observations.
        distribution.push(last["settle_ms"].clone());
        while distribution.len() < 5 {
            let attempt = step_once(args, from, to, 1.0, false).await?;
            distribution.push(attempt["settle_ms"].clone());
            attempts.push(attempt);
        }
    }
    Ok(json!({
        "from_kbps": from, "to_kbps": to, "settle_ms": last["settle_ms"],
        "meets_200ms": meets, "settle_distribution_ms": (!distribution.is_empty()).then_some(distribution),
        "attempts": attempts,
    }))
}

async fn refine(args: &Args) -> Result<Value, ProbeError> {
    let a = run_stream(args, args.kbps, None, true, true).await?;
    let b = run_stream(args, args.kbps, None, true, false).await?;
    refine_ab_report(args, a, b)
}

fn freeze_submission(run: &Run) -> Option<&Submission> {
    run.submissions
        .iter()
        .find(|frame| frame.index == 30 || frame.refine)
}

fn freeze_metrics(run: &Run) -> (Option<usize>, Option<usize>, Option<u64>) {
    let Some(submission) = freeze_submission(run) else {
        return (None, None, None);
    };
    let first = run
        .frames
        .iter()
        .find(|frame| frame.capture_us == submission.capture_us)
        .map(|frame| frame.bytes);
    let freeze_submissions: Vec<_> = run
        .submissions
        .iter()
        .filter(|frame| (30..35).contains(&frame.index))
        .collect();
    let first_five = (!freeze_submissions.is_empty()).then(|| {
        freeze_submissions
            .iter()
            .filter_map(|submission| {
                run.frames
                    .iter()
                    .find(|frame| frame.capture_us == submission.capture_us)
                    .map(|frame| frame.bytes)
            })
            .sum::<usize>()
    });
    (first, first_five, first.map(|_| submission.capture_us))
}

fn psnr_for_run(args: &Args, run: &Run) -> Option<f64> {
    let churn = args.pattern.churn()?;
    let source =
        encode::synthetic_luma_churn_texture(args.width, args.height, 29, churn, args.texture)?;
    let freeze = freeze_submission(run)?;
    let target = run
        .frames
        .iter()
        .find(|frame| frame.capture_us == freeze.capture_us)?;
    let mut ordered: Vec<_> = run.frames.iter().collect();
    ordered.sort_unstable_by_key(|frame| frame.capture_us);
    let keyframe_index = ordered.iter().position(|frame| frame.keyframe)?;
    let target_index = ordered
        .iter()
        .position(|frame| frame.capture_us == target.capture_us)?;
    if target_index < keyframe_index {
        return None;
    }
    let keyframe = ordered[keyframe_index];
    let samples: Vec<_> = ordered
        .iter()
        .skip(keyframe_index + 1)
        .take(target_index.saturating_sub(keyframe_index))
        .map(|frame| frame.data.as_slice())
        .collect();
    let decoded = encode::decode_annexb_sequence_luma(
        &keyframe.data,
        &samples,
        run.info.codec,
        args.width,
        args.height,
    )?;
    encode::psnr_db(&source, &decoded)
}

fn psnr_report_value(value: f64) -> f64 {
    if value.is_infinite() {
        100.0
    } else {
        value
    }
}

fn refine_group_json(args: &Args, run: &Run, trigger_refine: bool) -> Value {
    let (first_bytes, first_five_bytes, freeze_capture_ts_us) = freeze_metrics(run);
    let psnr = psnr_for_run(args, run);
    let refine_frame = run.frames.iter().find(|frame| frame.still_refine);
    let freeze_submission_refine = freeze_submission(run).is_some_and(|frame| frame.refine);
    let refine_submission = run.submissions.iter().find(|frame| frame.refine);
    let refine_output = run.frames.iter().find(|frame| frame.still_refine);
    let refine_submitted_but_missing = refine_submission.is_some_and(|submission| {
        !run.frames
            .iter()
            .any(|frame| frame.capture_us == submission.capture_us)
    });
    let refine_drop_reason = if !trigger_refine {
        None
    } else {
        match (refine_submission, refine_output) {
            (None, _) => Some("missing_refine_submission"),
            (Some(_), Some(_)) => None,
            (Some(_), None)
                if refine_submitted_but_missing && args.refine_relax_data_rate_limits =>
            {
                Some("vt_frame_dropped_even_with_data_rate_limit_relaxed")
            }
            (Some(_), None) if refine_submitted_but_missing => {
                Some("vt_frame_dropped_or_data_rate_limit")
            }
            (Some(_), None) => Some("no_output_callback"),
        }
    };
    json!({
        "trigger_refine": trigger_refine,
        "info": info_json(run.info),
        "frames": run.frames.len(),
        "pattern": args.pattern.name(),
        "churn": args.pattern.churn(),
        "texture": args.texture.name(),
        "data_rate_limits_enabled": !args.disable_data_rate_limits,
        "refine_data_rate_limits_relaxed": args.refine_relax_data_rate_limits,
        "stats": stats_json(run.stats),
        "freeze_capture_ts_us": freeze_capture_ts_us,
        "freeze_first_frame_bytes": first_bytes,
        "freeze_first_5_bytes": first_five_bytes,
        "freeze_submission_refine": freeze_submission_refine,
        "psnr_y_db": psnr.map(psnr_report_value),
        "psnr_source": "NV12 Y plane",
        "psnr_available": psnr.is_some(),
        "refine_output": refine_output.is_some(),
        "refine_drop_reason": refine_drop_reason,
        "refine_boost_kbps": (!run.info.quality_supported).then(|| args.kbps.saturating_mul(2)),
        "refine_limit_explanation": if refine_drop_reason.is_some() && !run.info.quality_supported {
            Some("bitrate_2x_and_DataRateLimits_were_used_for_refine")
        } else {
            None
        },
        "refine_flags": refine_output
            .map(|frame| u8::from(frame.keyframe) | (u8::from(frame.still_refine) << 1)),
        "refine_nal_types": refine_frame.map(|frame| &frame.nal_types),
    })
}

fn refine_ab_report(args: &Args, a: Run, b: Run) -> Result<Value, ProbeError> {
    let (a_first, a_five, _) = freeze_metrics(&a);
    let (b_first, b_five, _) = freeze_metrics(&b);
    let a_psnr = psnr_for_run(args, &a);
    let b_psnr = psnr_for_run(args, &b);
    let first_frame_better = a_first.zip(b_first).is_some_and(|(a, b)| a > b);
    let first_five_better = a_five.zip(b_five).is_some_and(|(a, b)| a > b);
    let psnr_better = a_psnr.zip(b_psnr).is_some_and(|(a, b)| a > b);
    Ok(json!({
        "mode": "refine_ab",
        "pattern": args.pattern.name(),
        "churn": args.pattern.churn(),
        "texture": args.texture.name(),
        "data_rate_limits_enabled": !args.disable_data_rate_limits,
        "refine_data_rate_limits_relaxed": args.refine_relax_data_rate_limits,
        "h264_limit": args.h264_limit.name(),
        "same_sequence": true,
        "groups": {
            "A": refine_group_json(args, &a, true),
            "B": refine_group_json(args, &b, false),
        },
        "compare": {
            "freeze_first_frame_a_gt_b": first_frame_better,
            "freeze_first_5_sum_a_gt_b": first_five_better,
            "psnr_a_db": a_psnr.map(psnr_report_value),
            "psnr_b_db": b_psnr.map(psnr_report_value),
            "psnr_a_gt_b": psnr_better,
            "quality_proven": first_frame_better && first_five_better && psnr_better,
        },
    }))
}

fn vt_sample_bytes(frame: &FrameSample, codec: Codec) -> u64 {
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
            let types = encode::nal_types(&frame.data[*start..end], codec);
            let kind = types.first().copied()?;
            let parameter_set = match codec {
                Codec::Hevc => matches!(kind, 32..=34),
                Codec::H264 => matches!(kind, 7 | 8),
                Codec::Unspecified => false,
            };
            (!parameter_set).then_some((end - start) as u64)
        })
        .sum()
}

fn peak_window_bytes(run: &Run, codec: Codec, window_us: u64) -> u64 {
    run.frames
        .iter()
        .map(|end| {
            run.frames
                .iter()
                .filter(|frame| {
                    frame.capture_us <= end.capture_us
                        && end.capture_us - frame.capture_us < window_us
                })
                .map(|frame| vt_sample_bytes(frame, codec))
                .sum()
        })
        .max()
        .unwrap_or(0)
}

async fn h264_scheme_runs(args: &Args, scheme: RateLimitScheme) -> Result<Value, ProbeError> {
    let mut trial_args = args.clone();
    trial_args.codec = Codec::H264;
    trial_args.h264_limit = scheme;
    trial_args.pattern = match trial_args.pattern {
        Pattern::Smooth | Pattern::Noise => Pattern::Churn(5),
        pattern => pattern,
    };
    let mut runs = Vec::new();
    for _ in 0..5 {
        let run = run_stream(&trial_args, trial_args.kbps, None, false, false).await?;
        let cap = host_mac::encode::rate_limit_pairs(trial_args.kbps, scheme)
            .first()
            .map(|(bytes, _)| *bytes)
            .unwrap_or(0);
        let peak = peak_window_bytes(&run, Codec::H264, 1_000_000);
        let short_cap = host_mac::encode::rate_limit_pairs(trial_args.kbps, scheme)
            .get(1)
            .map(|(bytes, _)| *bytes);
        let peak_short = short_cap.map(|_| peak_window_bytes(&run, Codec::H264, 250_000));
        let overshoot = peak.saturating_sub(cap) as f64 * 100.0 / cap.max(1) as f64;
        let short_overshoot = peak_short
            .zip(short_cap)
            .map(|(peak, cap)| peak.saturating_sub(cap) as f64 * 100.0 / cap.max(1) as f64);
        let drop_rate = if run.stats.submitted == 0 {
            1.0
        } else {
            run.stats.dropped as f64 / run.stats.submitted as f64
        };
        runs.push(json!({
            "data_rate_limits_enabled": !trial_args.disable_data_rate_limits,
            "peak_1s_bytes": peak,
            "cap_1s_bytes": cap,
            "peak_overshoot_pct": overshoot,
            "peak_0_25s_bytes": peak_short,
            "cap_0_25s_bytes": short_cap,
            "peak_0_25s_overshoot_pct": short_overshoot,
            "dropped": run.stats.dropped,
            "submitted": run.stats.submitted,
            "drop_rate": drop_rate,
            "stats": stats_json(run.stats),
        }));
    }
    let peak = runs
        .iter()
        .filter_map(|run| run["peak_overshoot_pct"].as_f64())
        .fold(0.0, f64::max);
    let drop_rate = runs
        .iter()
        .filter_map(|run| run["drop_rate"].as_f64())
        .fold(0.0, f64::max);
    Ok(json!({
        "scheme": scheme.name(),
        "pattern": trial_args.pattern.name(),
        "churn": trial_args.pattern.churn(),
        "texture": trial_args.texture.name(),
        "runs": runs,
        "max_peak_overshoot_pct": peak,
        "max_drop_rate": drop_rate,
        "meets_peak_5pct": peak <= 5.0,
    }))
}

async fn h264_experiment(args: &Args) -> Result<Value, ProbeError> {
    let mut schemes = Vec::new();
    for scheme in [
        RateLimitScheme::Current,
        RateLimitScheme::Tight12,
        RateLimitScheme::Short025,
    ] {
        schemes.push(h264_scheme_runs(args, scheme).await?);
    }
    Ok(json!({
        "mode": "h264_limit_experiment",
        "codec": "h264",
        "runs_per_scheme": 5,
        "schemes": schemes,
        "selected_scheme": Value::Null,
        "selection_rule": "peak overshoot <= 5% and drop rate <= current * 1.5; choose after real VT data",
        "production_default": RateLimitScheme::Current.name(),
    }))
}

async fn rate_limit_diagnosis(args: &Args) -> Result<Value, ProbeError> {
    let mut trial = args.clone();
    trial.kbps = 4_000;
    trial.seconds = trial.seconds.max(5);
    trial.pattern = match trial.pattern {
        Pattern::Smooth | Pattern::Noise => Pattern::Churn(5),
        pattern => pattern,
    };
    let mut cases = Vec::new();
    for disabled in [false, true] {
        let mut run_args = trial.clone();
        run_args.disable_data_rate_limits = disabled;
        let run = run_stream(&run_args, run_args.kbps, None, false, false).await?;
        let bytes: usize = run.frames.iter().map(|frame| frame.bytes).sum();
        let peak = peak_window_bytes(&run, run.info.codec, 1_000_000);
        let cap = host_mac::encode::data_rate_limits(run_args.kbps).0;
        cases.push(json!({
            "data_rate_limits_enabled": !disabled,
            "pattern": run_args.pattern.name(),
            "churn": run_args.pattern.churn(),
            "texture": run_args.texture.name(),
            "kbps": run_args.kbps,
            "out_kbps": bytes as f64 * 8_000.0 / run.elapsed_us.max(1) as f64,
            "peak_1s_bytes": peak,
            "cap_1s_bytes": cap,
            "peak_overshoot_pct": peak.saturating_sub(cap) as f64 * 100.0 / cap.max(1) as f64,
            "drop_rate": run.stats.dropped as f64 / run.stats.submitted.max(1) as f64,
            "stats": stats_json(run.stats),
        }));
    }
    Ok(json!({
        "mode": "data_rate_limit_diagnosis",
        "codec": if trial.codec == Codec::H264 { "h264" } else { "hevc" },
        "same_content": true,
        "cases": cases,
        "interpretation": "If disabling DataRateLimits materially reduces drops, the hard byte budget is causal; otherwise drops are VT realtime behavior.",
    }))
}

async fn run_mode(mut args: Args) -> Result<Value, ProbeError> {
    let calibration = if args.calibrate {
        let (_, report) = calibrate_churn(&args).await?;
        let selected = report["selected_churn"].as_u64().map(|value| value as u8);
        args.pattern = Pattern::Churn(selected.unwrap_or(5));
        args.calibrate = false;
        Some(if selected.is_none() {
            let mut report = report;
            report["fallback_used"] = Value::Bool(true);
            report["fallback_churn"] = json!(5);
            report
        } else {
            report
        })
    } else {
        None
    };
    let result = match args.mode {
        Mode::Bench => bench(&args).await,
        Mode::Refine => refine(&args).await,
        Mode::H264Experiment => h264_experiment(&args).await,
        Mode::RateLimitDiagnosis => rate_limit_diagnosis(&args).await,
        Mode::Step(from, to) => match step_direction(&args, from, to).await {
            Ok(forward) => step_direction(&args, to, from)
                .await
                .map(|reverse| json!({"directions": [forward, reverse]})),
            Err(error) => Err(error),
        },
    }?;
    Ok(match calibration {
        Some(calibration) => json!({"calibration": calibration, "result": result}),
        None => result,
    })
}

#[cfg(test)]
fn refine_report(pattern: Pattern, run: Run) -> Result<Value, ProbeError> {
    let submission = run
        .submissions
        .iter()
        .find(|frame| frame.refine)
        .ok_or_else(|| ProbeError::arguments("missing refine submission"))?;
    // Noise can drop warm-up frames. Locate the actual boost by its PTS,
    // never by its position in the shorter output list.
    let boosted = run
        .frames
        .iter()
        .find(|frame| frame.capture_us == submission.capture_us);
    let preceding = run
        .frames
        .iter()
        .rev()
        .find(|frame| frame.capture_us < submission.capture_us && !frame.keyframe);
    let mut before: Vec<_> = run
        .frames
        .iter()
        .filter(|frame| frame.capture_us < submission.capture_us && !frame.keyframe)
        .map(|frame| frame.bytes as u64)
        .collect();
    let mut after: Vec<_> = run
        .frames
        .iter()
        .filter(|frame| frame.capture_us > submission.capture_us && !frame.keyframe)
        .map(|frame| frame.bytes as u64)
        .collect();
    let before_median = percentile(&mut before, 50);
    let after_median = percentile(&mut after, 50);
    let ratio = (before_median > 0).then(|| after_median as f64 / before_median as f64);
    let larger = boosted
        .zip(preceding)
        .map(|(boost, prev)| boost.bytes > prev.bytes);
    let warmup_output = run
        .frames
        .iter()
        .filter(|frame| frame.capture_us < submission.capture_us)
        .count();
    let warmup_dropped = submission.index - warmup_output as u64;
    let warmup_bytes: usize = run
        .frames
        .iter()
        .filter(|frame| frame.capture_us < submission.capture_us)
        .map(|frame| frame.bytes)
        .sum();
    let warmup_us = submission.capture_us.saturating_sub(run.started_us);
    let after_output = run
        .frames
        .iter()
        .filter(|frame| frame.capture_us > submission.capture_us)
        .count();
    let flags_ok = boosted.is_some_and(|frame| frame.still_refine && !frame.keyframe);
    Ok(json!({
        "info": info_json(run.info), "frames": run.frames.len(),
        "pattern": pattern.name(), "stats": stats_json(run.stats),
        "drop_diagnostics": drop_diagnostics(&run),
        "refine_method": if run.info.quality_supported { "quality" } else { "bitrate_2x" },
        "warmup_output": warmup_output,
        "warmup_dropped": warmup_dropped,
        "warmup_out_kbps": warmup_bytes as f64 * 8_000.0 / warmup_us.max(1) as f64,
        "rate_limited_warmup": warmup_dropped > 0,
        "refine_capture_ts_us": submission.capture_us,
        "refine_output": boosted.is_some(),
        "refine_size": boosted.map(|frame| frame.bytes),
        "previous_p_size": preceding.map(|frame| frame.bytes),
        "previous_p_capture_ts_us": preceding.map(|frame| frame.capture_us),
        "refine_larger_than_previous_p": larger,
        "meets_refine_expectation": warmup_dropped > 0 && flags_ok && larger == Some(true),
        "refine_flags": boosted.map(|frame| u8::from(frame.keyframe) | (u8::from(frame.still_refine) << 1)),
        "refine_nal_types": boosted.map(|frame| &frame.nal_types),
        "baseline_p_size_median": before_median,
        "after_p_size_median": after_median,
        "after_baseline_ratio": ratio,
        "after_returns_to_baseline": ratio.is_some_and(|ratio| (0.7..=1.3).contains(&ratio)),
        "after_output": after_output,
        "after_flags_clear": after_output > 0 && run.frames.iter()
            .filter(|frame| frame.capture_us > submission.capture_us)
            .all(|frame| !frame.still_refine),
    }))
}

#[tokio::main(flavor = "current_thread")]
pub async fn main() {
    let result = match parse_args() {
        Ok(args) => run_mode(args).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            println!(
                "{}",
                json!({
                    "error": error.code, "message": error.message,
                    "hevc_disabled": env::var_os("RS_HOST_DISABLE_HEVC").is_some_and(|value| value == "1"),
                    "stats": error.stats.map(|stats| stats_json(*stats)),
                    "info": error.info.map(info_json),
                    "pattern": error.pattern,
                })
            );
            std::process::exit(if error.code == "invalid_arguments" {
                2
            } else {
                1
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_failure_reasons_choose_opposite_search_directions() {
        assert_eq!(
            classify_calibration(true, true).fields(),
            ("qualified", "lower_churn")
        );
        assert_eq!(
            classify_calibration(false, true).fields(),
            ("too_low_output", "increase_churn")
        );
        assert_eq!(
            classify_calibration(true, false).fields(),
            ("too_many_drops", "decrease_churn")
        );
        assert_eq!(
            classify_calibration(false, false).fields(),
            ("uncalibratable", "stop")
        );
    }

    fn dropped_warmup(boost_bytes: Option<usize>) -> Run {
        let started_us = 1_000_000;
        let submissions: Vec<_> = (0..32)
            .map(|index| Submission {
                index,
                capture_us: started_us + index * 16_667,
                refine: index == 30,
            })
            .collect();
        let frames: Vec<_> = submissions
            .iter()
            .filter(|frame| frame.index != 29 && (frame.index != 30 || boost_bytes.is_some()))
            .map(|frame| FrameSample {
                capture_us: frame.capture_us,
                output_us: frame.capture_us + 10_000,
                bytes: if frame.refine {
                    boost_bytes.unwrap_or_default()
                } else {
                    100
                },
                data: Vec::new(),
                keyframe: frame.index == 0,
                still_refine: frame.refine,
                nal_types: Vec::new(),
            })
            .collect();
        Run {
            info: EncoderInfo {
                codec: Codec::Hevc,
                low_latency: true,
                hardware: true,
                quality_supported: false,
            },
            stats: EncoderStats {
                submitted: 32,
                output: frames.len() as u64,
                dropped: 32 - frames.len() as u64,
                ..EncoderStats::default()
            },
            frames,
            started_us,
            elapsed_us: 32 * 16_667,
            changed_us: Some(submissions[30].capture_us - 500),
            change_completed_us: Some(submissions[30].capture_us - 100),
            change_index: Some(30),
            submissions,
        }
    }

    #[test]
    fn rate_change_drop_is_identified_by_submission_pts() {
        let run = dropped_warmup(None);
        let report = drop_diagnostics(&run);
        assert_eq!(report["missing_frames"], 2);
        assert_eq!(report["missing_submission_indices"], json!([29, 30]));
        assert_eq!(report["all_missing_counted_as_dropped"], true);
        assert_eq!(report["rate_change_frame"]["index"], 30);
        assert_eq!(report["rate_change_frame"]["dropped"], true);
        assert_eq!(
            report["missing_within_200ms_of_rate_change"][1]["offset_from_rate_change_us"],
            500
        );
    }

    #[test]
    fn refinement_report_survives_drops_and_requires_a_strict_size_increase() {
        let report = refine_report(Pattern::Noise, dropped_warmup(Some(200)))
            .unwrap_or_else(|error| panic!("{}", error.message));
        assert_eq!(report["warmup_dropped"], 1);
        assert_eq!(report["refine_size"], 200);
        assert_eq!(report["previous_p_size"], 100);
        assert_eq!(report["refine_flags"], 2);
        assert_eq!(report["meets_refine_expectation"], true);
        let equal = refine_report(Pattern::Noise, dropped_warmup(Some(100)))
            .unwrap_or_else(|error| panic!("{}", error.message));
        assert_eq!(equal["refine_larger_than_previous_p"], false);
        assert_eq!(equal["meets_refine_expectation"], false);
        let dropped = refine_report(Pattern::Noise, dropped_warmup(None))
            .unwrap_or_else(|error| panic!("{}", error.message));
        assert!(dropped["refine_size"].is_null());
        assert_eq!(dropped["refine_output"], false);
        assert_eq!(dropped["meets_refine_expectation"], false);
    }
}
