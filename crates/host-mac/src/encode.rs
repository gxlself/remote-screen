//! VideoToolbox diagnostics and pure helpers for the encoder probe.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use host_core::platform::{CapturedFrame, EncoderConfig, Result};
use rsp_core::pb::Codec;

pub use crate::platform::MacEncoder;

/// Capabilities of the session actually selected by VideoToolbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderInfo {
    pub codec: Codec,
    pub low_latency: bool,
    pub hardware: bool,
    pub quality_supported: bool,
}

/// Counters collected by the native VideoToolbox callback and submit path.
/// `callback_error` includes a nonzero callback status or a NULL sample
/// buffer; the more specific counters make normal dropped callbacks distinct
/// from actual encoder/conversion failures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncoderStats {
    pub submitted: u64,
    pub output: u64,
    pub dropped: u64,
    pub callback_error: u64,
    pub submit_error: u64,
    pub callback_status_error: u64,
    pub unexpected_null: u64,
    pub conversion_error: u64,
    /// Forced attempts that failed to emit a keyframe, including attempts
    /// already in flight when another keyframe confirms the request.
    /// Submit/callback reports for the same attempt count once.
    pub forced_key_retries: u64,
}

/// Create the same encoder as `Platform::encoder`, with diagnostic information.
#[doc(hidden)]
pub fn encoder(config: EncoderConfig) -> Result<(MacEncoder, EncoderInfo)> {
    crate::platform::create_encoder(config)
}

/// VideoToolbox's byte budget: 1.5 times the target bitrate over one second.
pub fn data_rate_limits(kbps: u32) -> (u64, f64) {
    (u64::from(kbps) * 1_500 / 8, 1.0)
}

/// Data-rate limit choices used by the H.264 compensation probe.
///
/// `Current` is the production setting (1.5x over one second).  The other
/// choices are deliberately explicit so a real VideoToolbox run can compare
/// them before changing the production default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitScheme {
    Current,
    Tight12,
    Short025,
}

impl RateLimitScheme {
    pub fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Tight12 => "tight_1.2x",
            Self::Short025 => "short_0.25s",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "current" | "1.5" | "1.5x" => Some(Self::Current),
            "tight12" | "tight_1.2x" | "1.2" | "1.2x" => Some(Self::Tight12),
            "short025" | "short_0.25s" | "0.25" => Some(Self::Short025),
            _ => None,
        }
    }
}

/// Return the DataRateLimits pairs that a scheme requests from VT.
///
/// The values are kept in the same bytes/seconds units as the native API.
/// `Short025` appends a 250 ms budget while retaining the normal one-second
/// budget, which lets the probe measure both constraints independently.
pub fn rate_limit_pairs(kbps: u32, scheme: RateLimitScheme) -> Vec<(u64, f64)> {
    let one_second = match scheme {
        RateLimitScheme::Current | RateLimitScheme::Short025 => data_rate_limits(kbps).0,
        RateLimitScheme::Tight12 => u64::from(kbps) * 1_200 / 8,
    };
    let mut pairs = vec![(one_second, 1.0)];
    if scheme == RateLimitScheme::Short025 {
        pairs.push((u64::from(kbps) * 1_500 / 8 / 4, 0.25));
    }
    pairs
}

/// Temporary bitrate for refinement when `Quality` is unavailable.
pub fn refine_bump(kbps: u32) -> u32 {
    kbps.saturating_mul(2)
}

/// Churn uses a fixed 64x64-pixel grid.  The grid rounds up at the right and
/// bottom edges so every pixel belongs to exactly one block.
pub const CHURN_BLOCK_SIZE: u32 = 64;

/// Content drawn inside selected churn blocks.  `Gradient` keeps spatial
/// structure so the same block coverage can be used at a low target without
/// turning every frame into incompressible pixel noise; `Noise` is the stress
/// pattern used to exercise the hard VT budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChurnTexture {
    Gradient,
    Noise,
}

impl ChurnTexture {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Noise => "noise",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gradient" | "smooth" => Some(Self::Gradient),
            "noise" | "random" => Some(Self::Noise),
            _ => None,
        }
    }

    pub(crate) fn native(self) -> u32 {
        match self {
            Self::Gradient => 0,
            Self::Noise => 1,
        }
    }
}

pub fn churn_grid(width: u32, height: u32) -> (u32, u32) {
    (
        width.div_ceil(CHURN_BLOCK_SIZE),
        height.div_ceil(CHURN_BLOCK_SIZE),
    )
}

pub fn churn_block_count(width: u32, height: u32) -> u32 {
    let (columns, rows) = churn_grid(width, height);
    columns.saturating_mul(rows)
}

fn churn_hash(seed: u32, block: u32) -> u32 {
    let mut value = seed
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add(block.wrapping_mul(0x85eb_ca6b))
        .wrapping_add(0xc2b2_ae35);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value
}

/// Return the deterministic set of blocks redrawn for one frame.
///
/// Sorting pseudo-random keys rather than sampling with a mutable RNG makes
/// the selection reproducible across repeated runs and independent of the
/// order in which callers inspect the blocks.
pub fn churn_blocks(width: u32, height: u32, seed: u32, churn_percent: u8) -> Vec<(u32, u32)> {
    let (columns, rows) = churn_grid(width, height);
    let total = columns.saturating_mul(rows);
    let percent = u32::from(churn_percent.min(100));
    let selected = total.saturating_mul(percent).div_ceil(100);
    let mut keys: Vec<_> = (0..total)
        .map(|index| (churn_hash(seed, index), index))
        .collect();
    keys.sort_unstable();
    keys.truncate(selected as usize);
    keys.into_iter()
        .map(|(_, index)| (index % columns, index / columns))
        .collect()
}

/// Compute PSNR for two equally sized luma planes. A lossless comparison is
/// represented by positive infinity; JSON-facing callers should map that
/// non-finite value to a documented finite sentinel or null.
pub fn psnr_db(source: &[u8], reconstructed: &[u8]) -> Option<f64> {
    if source.is_empty() || source.len() != reconstructed.len() {
        return None;
    }
    let mse = source
        .iter()
        .zip(reconstructed)
        .map(|(left, right)| {
            let delta = f64::from(*left) - f64::from(*right);
            delta * delta
        })
        .sum::<f64>()
        / source.len() as f64;
    if mse == 0.0 {
        Some(f64::INFINITY)
    } else {
        Some(10.0 * (255.0 * 255.0 / mse).log10())
    }
}

/// Find the lowest churn level satisfying a monotone calibration predicate.
/// The probe supplies the predicate from real VT measurements; this helper is
/// pure so its boundary behaviour can be tested in the sandbox.
pub fn select_churn<F>(mut qualifies: F) -> Option<u8>
where
    F: FnMut(u8) -> bool,
{
    let mut low = 0u8;
    let mut high = 100u8;
    let mut selected = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        if qualifies(middle) {
            selected = Some(middle);
            if middle == 0 {
                break;
            }
            high = middle - 1;
        } else if middle == 100 {
            break;
        } else {
            low = middle + 1;
        }
    }
    selected
}

/// NAL types in a HEVC or H.264 Annex-B access unit (3- or 4-byte start codes).
/// Empty NALs, incomplete HEVC headers and unknown codecs produce no entries.
pub fn nal_types(annexb: &[u8], codec: Codec) -> Vec<u8> {
    if !matches!(codec, Codec::Hevc | Codec::H264) {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = 0;
    while let Some((start, code_len)) = next_start_code(annexb, offset) {
        let nal_start = start + code_len;
        let next = next_start_code(annexb, nal_start)
            .map(|(position, _)| position)
            .unwrap_or(annexb.len());
        let minimum_header = if codec == Codec::Hevc { 2 } else { 1 };
        if next - nal_start >= minimum_header {
            let header = annexb[nal_start];
            result.push(if codec == Codec::Hevc {
                (header >> 1) & 0x3f
            } else {
                header & 0x1f
            });
        }
        offset = next;
    }
    result
}

fn next_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    (from..data.len().saturating_sub(2)).find_map(|index| {
        if data[index..].starts_with(&[0, 0, 0, 1]) {
            Some((index, 4))
        } else if data[index..].starts_with(&[0, 0, 1]) {
            Some((index, 3))
        } else {
            None
        }
    })
}

/// Byte counts at output callbacks, with timestamps in Unix microseconds.
#[derive(Debug, Default, Clone)]
pub struct StepMeter {
    samples: Vec<(u64, u64)>,
}

impl StepMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, t_us: u64, bytes: u64) {
        // Sorting also makes the meter usable with batched or out-of-order input.
        let index = self.samples.partition_point(|(stamp, _)| *stamp <= t_us);
        self.samples.insert(index, (t_us, bytes));
    }

    pub fn samples(&self) -> &[(u64, u64)] {
        &self.samples
    }

    /// Kilobits per second in the preceding 100 ms, including this instant.
    pub fn window_kbps(&self, t_us: u64) -> f64 {
        self.samples
            .iter()
            .filter(|(stamp, _)| *stamp <= t_us && t_us - *stamp < 100_000)
            .map(|(_, bytes)| *bytes as f64)
            .sum::<f64>()
            * 0.08
    }

    /// First entry into target ±30% that lasts 300 ms. The result is backdated
    /// to entry, after observing the full hold interval, so a <=200 ms response
    /// can be measured. Both arrivals and window expirations are checked; a
    /// gap in output cannot count as stable throughput. Returns None if the
    /// observation ends before a full 300 ms hold can be confirmed.
    pub fn settle_time_us(&self, changed_at_us: u64, target_kbps: u32) -> Option<u64> {
        let last_us = self.samples.last()?.0;
        if target_kbps == 0 || changed_at_us > last_us {
            return None;
        }
        let mut events = BTreeMap::<u64, f64>::new();
        for (stamp, bytes) in &self.samples {
            *events.entry(*stamp).or_default() += *bytes as f64;
            if let Some(expires) = stamp.checked_add(100_000) {
                *events.entry(expires).or_default() -= *bytes as f64;
            }
        }
        let target = f64::from(target_kbps);
        let in_band = |rate: f64| rate >= target * 0.7 && rate <= target * 1.3;
        let mut bytes = self.window_kbps(changed_at_us) / 0.08;
        let mut stable_since = in_band(bytes * 0.08).then_some(changed_at_us);
        for (stamp, delta) in events.range((
            std::ops::Bound::Excluded(changed_at_us),
            std::ops::Bound::Included(last_us),
        )) {
            if let Some(start) = stable_since {
                if *stamp - start >= 300_000 {
                    return Some(start - changed_at_us);
                }
            }
            bytes += delta;
            if in_band(bytes * 0.08) {
                stable_since.get_or_insert(*stamp);
            } else {
                stable_since = None;
            }
        }
        stable_since
            .filter(|start| last_us - start >= 300_000)
            .map(|start| start - changed_at_us)
    }

    pub fn settle_ms(&self, changed_at_us: u64, target_kbps: u32) -> Option<u64> {
        self.settle_time_us(changed_at_us, target_kbps)
            .map(|duration| duration.div_ceil(1_000))
    }
}

extern "C" {
    fn rs_test_pixel_buffer_new_pattern(
        width: u32,
        height: u32,
        seed: u32,
        pattern: u32,
    ) -> *mut c_void;
    fn rs_test_pixel_buffer_new_churn_texture(
        width: u32,
        height: u32,
        seed: u32,
        churn_percent: u32,
        texture: u32,
    ) -> *mut c_void;
    fn rs_test_pixel_buffer_luma_churn_texture(
        width: u32,
        height: u32,
        seed: u32,
        churn_percent: u32,
        texture: u32,
        out: *mut u8,
        len: usize,
    ) -> i32;
    fn rs_decode_annexb_luma(
        parameter_data: *const u8,
        parameter_len: usize,
        sample_data: *const u8,
        sample_len: usize,
        codec: u32,
        width: u32,
        height: u32,
        out: *mut u8,
        out_len: usize,
    ) -> i32;
    fn rs_decode_annexb_sequence_luma(
        parameter_data: *const u8,
        parameter_len: usize,
        sample_data: *const *const u8,
        sample_lens: *const usize,
        sample_count: usize,
        codec: u32,
        width: u32,
        height: u32,
        out: *mut u8,
        out_len: usize,
    ) -> i32;
    fn rs_pixel_buffer_release(pixel: *mut c_void);
}

pub(crate) struct SyntheticFrame(pub(crate) NonNull<c_void>);
// SAFETY: a synthetic buffer is immutable after initialization. Its retained
// reference is released only when the last CapturedFrame owner disappears.
unsafe impl Send for SyntheticFrame {}
// SAFETY: shared references only lend the immutable buffer to VideoToolbox.
unsafe impl Sync for SyntheticFrame {}
impl Drop for SyntheticFrame {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the reference returned by pixel_buffer_new.
        unsafe { rs_pixel_buffer_release(self.0.as_ptr()) };
    }
}

/// An IOSurface-backed NV12 screen-like frame with a fixed background and
/// small moving blocks/text bars. Use `synthetic_frame_pattern` for noise.
/// Invalid dimensions or allocation failure yield an unusable native frame;
/// `Encoder::encode` then returns a platform error, preserving this infallible
/// helper's signature without panicking on native allocation failure.
#[doc(hidden)]
pub fn synthetic_frame(w: u32, h: u32, seed: u32, ts_us: u64) -> CapturedFrame {
    synthetic_frame_pattern(w, h, seed, ts_us, 0)
}

/// Synthetic frame with pattern 0 (smooth screen-like) or 1 (high entropy
/// noise for exercising VideoToolbox's hard data-rate limit).
#[doc(hidden)]
pub fn synthetic_frame_pattern(
    w: u32,
    h: u32,
    seed: u32,
    ts_us: u64,
    pattern: u32,
) -> CapturedFrame {
    // SAFETY: primitive arguments only; the shim validates dimensions and
    // returns one retained CVPixelBuffer, or NULL on failure.
    let pixel = unsafe { rs_test_pixel_buffer_new_pattern(w, h, seed, pattern) };
    let native: Arc<dyn std::any::Any + Send + Sync> = match NonNull::new(pixel) {
        Some(pixel) => Arc::new(SyntheticFrame(pixel)),
        None => {
            tracing::error!(
                width = w,
                height = h,
                "could not allocate synthetic NV12 frame"
            );
            Arc::new(())
        }
    };
    CapturedFrame {
        display_id: 0,
        width: w,
        height: h,
        capture_ts_us: ts_us,
        native,
    }
}

/// Synthetic frame with a fixed screen-like background and deterministic
/// entropy in the requested percentage of 64x64 blocks.
#[doc(hidden)]
pub fn synthetic_frame_churn(
    w: u32,
    h: u32,
    seed: u32,
    ts_us: u64,
    churn_percent: u8,
) -> CapturedFrame {
    synthetic_frame_churn_texture(w, h, seed, ts_us, churn_percent, ChurnTexture::Noise)
}

/// Synthetic churn frame with an explicit block texture.
#[doc(hidden)]
pub fn synthetic_frame_churn_texture(
    w: u32,
    h: u32,
    seed: u32,
    ts_us: u64,
    churn_percent: u8,
    texture: ChurnTexture,
) -> CapturedFrame {
    // SAFETY: primitive arguments only; the shim validates dimensions and
    // returns one retained CVPixelBuffer, or NULL on failure.
    let pixel = unsafe {
        rs_test_pixel_buffer_new_churn_texture(
            w,
            h,
            seed,
            u32::from(churn_percent.min(100)),
            texture.native(),
        )
    };
    let native: Arc<dyn std::any::Any + Send + Sync> = match NonNull::new(pixel) {
        Some(pixel) => Arc::new(SyntheticFrame(pixel)),
        None => {
            tracing::error!(
                width = w,
                height = h,
                churn = churn_percent,
                texture = texture.name(),
                "could not allocate synthetic churn NV12 frame"
            );
            Arc::new(())
        }
    };
    CapturedFrame {
        display_id: 0,
        width: w,
        height: h,
        capture_ts_us: ts_us,
        native,
    }
}

/// Return the source NV12 Y plane used by the churn generator.  This is used
/// by the optional VideoToolbox decoder in the A/B quality probe.
#[doc(hidden)]
pub fn synthetic_luma_churn(w: u32, h: u32, seed: u32, churn_percent: u8) -> Option<Vec<u8>> {
    synthetic_luma_churn_texture(w, h, seed, churn_percent, ChurnTexture::Noise)
}

/// Return the source luma plane for an explicit churn texture.
#[doc(hidden)]
pub fn synthetic_luma_churn_texture(
    w: u32,
    h: u32,
    seed: u32,
    churn_percent: u8,
    texture: ChurnTexture,
) -> Option<Vec<u8>> {
    let length = (u64::from(w) * u64::from(h)).try_into().ok()?;
    let mut output = vec![0u8; length];
    // SAFETY: the vector is allocated for exactly width*height bytes and the
    // shim validates all dimensions before writing.
    let status = unsafe {
        rs_test_pixel_buffer_luma_churn_texture(
            w,
            h,
            seed,
            u32::from(churn_percent.min(100)),
            texture.native(),
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status == 0).then_some(output)
}

/// Decode an Annex-B access unit using VideoToolbox and return its NV12 Y
/// plane. `parameter_data` must contain a keyframe with codec parameter sets;
/// it is fed first when `sample_data` is a delta frame.
#[doc(hidden)]
pub fn decode_annexb_luma(
    parameter_data: &[u8],
    sample_data: &[u8],
    codec: Codec,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    if parameter_data.is_empty()
        || sample_data.is_empty()
        || !matches!(codec, Codec::Hevc | Codec::H264)
    {
        return None;
    }
    let length = (u64::from(width) * u64::from(height)).try_into().ok()?;
    let mut output = vec![0u8; length];
    // SAFETY: all pointers refer to live immutable slices or the output vector;
    // the shim copies the decoded plane before returning.
    let status = unsafe {
        rs_decode_annexb_luma(
            parameter_data.as_ptr(),
            parameter_data.len(),
            sample_data.as_ptr(),
            sample_data.len(),
            codec as u32,
            width,
            height,
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status == 0).then_some(output)
}

/// Decode a keyframe followed by an ordered sequence of access units and
/// return the final frame's NV12 Y plane. This preserves inter-frame
/// references for P frames used by the refine A/B comparison.
#[doc(hidden)]
pub fn decode_annexb_sequence_luma(
    parameter_data: &[u8],
    samples: &[&[u8]],
    codec: Codec,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    if parameter_data.is_empty() || samples.is_empty() {
        return None;
    }
    let length = (u64::from(width) * u64::from(height)).try_into().ok()?;
    let mut output = vec![0u8; length];
    let pointers: Vec<_> = samples.iter().map(|sample| sample.as_ptr()).collect();
    let lengths: Vec<_> = samples.iter().map(|sample| sample.len()).collect();
    // SAFETY: pointers and lengths borrow the immutable sample slices for the
    // duration of the synchronous native decode call.
    let status = unsafe {
        rs_decode_annexb_sequence_luma(
            parameter_data.as_ptr(),
            parameter_data.len(),
            pointers.as_ptr(),
            lengths.as_ptr(),
            samples.len(),
            codec as u32,
            width,
            height,
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status == 0).then_some(output)
}
