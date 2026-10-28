//! Pure WASAPI wave-format parsing and PCM conversion.
//!
//! The Win32 layer owns COM and audio-client lifetimes.  Keeping these
//! operations here makes all format and timestamp edge cases testable on any
//! host.

pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

pub const SUBTYPE_PCM: u128 = 0x00000001_0000_0010_8000_00aa00389b71;
pub const SUBTYPE_IEEE_FLOAT: u128 = 0x00000003_0000_0010_8000_00aa00389b71;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    I24,
    I24In32,
    I32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub block_align: u16,
    pub format: SampleFormat,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("unsupported wave format")]
    Unsupported,
    #[error("bad wave format")]
    Bad,
}

/// Parse the fields of a `WAVEFORMATEX` (and, where present, its extensible
/// suffix) into the small set of formats consumed by the audio pipeline.
pub fn parse_wave_format(
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    block_align: u16,
    ext: Option<(u16, u128)>,
) -> Result<WaveFormat, FormatError> {
    if channels == 0 || sample_rate == 0 {
        return Err(FormatError::Bad);
    }
    let Some(expected_align) = (u32::from(channels))
        .checked_mul(u32::from(bits))
        .and_then(|bits| bits.checked_div(8))
    else {
        return Err(FormatError::Bad);
    };
    if expected_align == 0
        || expected_align > u32::from(u16::MAX)
        || block_align != expected_align as u16
    {
        return Err(FormatError::Bad);
    }

    let format = match tag {
        WAVE_FORMAT_PCM => pcm_format(bits),
        WAVE_FORMAT_IEEE_FLOAT if bits == 32 => Some(SampleFormat::F32),
        WAVE_FORMAT_EXTENSIBLE => {
            let Some((valid_bits, subtype)) = ext else {
                return Err(FormatError::Unsupported);
            };
            match subtype {
                SUBTYPE_PCM if bits == 32 && valid_bits == 24 => Some(SampleFormat::I24In32),
                SUBTYPE_PCM => pcm_format(bits),
                SUBTYPE_IEEE_FLOAT if bits == 32 => Some(SampleFormat::F32),
                _ => None,
            }
        }
        _ => None,
    };
    format
        .map(|format| WaveFormat {
            sample_rate,
            channels,
            block_align,
            format,
        })
        .ok_or(FormatError::Unsupported)
}

fn pcm_format(bits: u16) -> Option<SampleFormat> {
    match bits {
        16 => Some(SampleFormat::I16),
        24 => Some(SampleFormat::I24),
        32 => Some(SampleFormat::I32),
        _ => None,
    }
}

fn bytes_per_sample(format: SampleFormat) -> usize {
    match format {
        SampleFormat::I24 => 3,
        SampleFormat::F32 | SampleFormat::I24In32 | SampleFormat::I32 => 4,
        SampleFormat::I16 => 2,
    }
}

fn valid_format(fmt: &WaveFormat) -> bool {
    if fmt.sample_rate == 0 || fmt.channels == 0 {
        return false;
    }
    let Some(expected) = usize::from(fmt.channels).checked_mul(bytes_per_sample(fmt.format)) else {
        return false;
    };
    expected == usize::from(fmt.block_align)
}

/// Convert interleaved samples to the pipeline's stereo f32 representation.
/// The destination is deliberately append-only so callers can reuse a buffer
/// and retain any samples already written before this call.
pub fn to_stereo_f32(
    data: &[u8],
    fmt: &WaveFormat,
    frames: u32,
    out: &mut Vec<f32>,
) -> Result<(), FormatError> {
    if !valid_format(fmt) {
        return Err(FormatError::Bad);
    }
    let frames = usize::try_from(frames).map_err(|_| FormatError::Bad)?;
    let needed = frames
        .checked_mul(usize::from(fmt.block_align))
        .ok_or(FormatError::Bad)?;
    if data.len() < needed {
        return Err(FormatError::Bad);
    }
    let channels = usize::from(fmt.channels);
    let sample_bytes = bytes_per_sample(fmt.format);
    let output_len = frames.checked_mul(2).ok_or(FormatError::Bad)?;
    out.try_reserve(output_len).map_err(|_| FormatError::Bad)?;
    for frame in 0..frames {
        let base = frame * usize::from(fmt.block_align);
        let left = decode_sample(&data[base..base + sample_bytes], fmt.format);
        let right = if channels == 1 {
            left
        } else {
            decode_sample(
                &data[base + sample_bytes..base + sample_bytes * 2],
                fmt.format,
            )
        };
        out.push(left);
        out.push(right);
    }
    Ok(())
}

fn decode_sample(bytes: &[u8], format: SampleFormat) -> f32 {
    match format {
        SampleFormat::F32 => {
            let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if value.is_finite() {
                value
            } else {
                0.0
            }
        }
        SampleFormat::I16 => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0,
        SampleFormat::I24 => {
            let raw =
                u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16);
            let signed = if raw & 0x0080_0000 != 0 {
                (raw | 0xff00_0000) as i32
            } else {
                raw as i32
            };
            signed as f32 / 8_388_608.0
        }
        SampleFormat::I24In32 | SampleFormat::I32 => {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0
        }
    }
}

/// Convert a WASAPI QPC timestamp expressed in 100 ns units to unix µs using
/// a contemporaneous pair of clocks.
pub fn qpc_to_unix_us(sample_qpc_100ns: u64, now_qpc_100ns: u64, now_unix_us: u64) -> u64 {
    if sample_qpc_100ns > now_qpc_100ns {
        return now_unix_us;
    }
    let elapsed_us = now_qpc_100ns.saturating_sub(sample_qpc_100ns) / 10;
    now_unix_us.saturating_sub(elapsed_us)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPlan {
    AutoConvert,
    UseMix(WaveFormat),
    Unsupported,
}

pub fn plan_open(requested_rate: u32, autoconvert_ok: bool, mix: Option<WaveFormat>) -> OpenPlan {
    if autoconvert_ok {
        return OpenPlan::AutoConvert;
    }
    let Some(mix) = mix else {
        return OpenPlan::Unsupported;
    };
    if mix.sample_rate == requested_rate && valid_format(&mix) {
        OpenPlan::UseMix(mix)
    } else {
        OpenPlan::Unsupported
    }
}
