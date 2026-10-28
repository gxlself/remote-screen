//! ：Windows FFmpeg 解码的 ABI 常量与纯逻辑（所有目标编译）。
//!
//! The values in this module intentionally mirror the corresponding subset in
//! `host_core::ffmpeg_abi`. Keep this module independent of the Windows FFI so
//! that the ABI checks run on every development platform.

use std::{
    collections::VecDeque,
    ffi::CStr,
    path::{Path, PathBuf},
};

use crate::platform::VideoCodec;

pub const AVCODEC_MAJOR: u32 = 62;
pub const AVUTIL_MAJOR: u32 = 60;
pub const AVCODEC_DLL: &str = "avcodec-62.dll";
pub const AVUTIL_DLL: &str = "avutil-60.dll";
pub const FFMPEG_DIR_ENV: &str = "RS_FFMPEG_DIR";
pub const FFMPEG_SUBDIR: &str = "ffmpeg";

pub fn ffmpeg_dir(exe_dir: &Path, env_value: Option<&str>) -> PathBuf {
    match env_value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => exe_dir.join(FFMPEG_SUBDIR),
    }
}

fn version_parts(version: u32) -> (u32, u32, u32) {
    (version >> 16, (version >> 8) & 0xff, version & 0xff)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AbiError {
    #[error("{lib} version {found:#x} is not FFmpeg 8.x")]
    Version { lib: &'static str, found: u32 },
    #[error("{lib} is not LGPL: {text}")]
    License { lib: &'static str, text: String },
    #[error("FFmpeg built with {0}")]
    Config(&'static str),
    #[error("ABI mismatch at {field}: {found} != {want}")]
    Offset {
        field: &'static str,
        found: i32,
        want: i32,
    },
}

pub fn check_versions(avcodec: u32, avutil: u32) -> Result<(), AbiError> {
    for (lib, found, want) in [
        ("avcodec", avcodec, AVCODEC_MAJOR),
        ("avutil", avutil, AVUTIL_MAJOR),
    ] {
        if version_parts(found).0 != want {
            return Err(AbiError::Version { lib, found });
        }
    }
    Ok(())
}

pub fn check_license(avcodec: &str, avutil: &str, configuration: &str) -> Result<(), AbiError> {
    for (lib, text) in [("avcodec", avcodec), ("avutil", avutil)] {
        if !text.starts_with("LGPL") {
            return Err(AbiError::License {
                lib,
                text: text.to_owned(),
            });
        }
    }
    for flag in ["--enable-gpl", "--enable-nonfree"] {
        if configuration.contains(flag) {
            return Err(AbiError::Config(flag));
        }
    }
    Ok(())
}

/// AVOption offsets used by the decoder. This is a subset of the complete
/// encoder list in `host_core::ffmpeg_abi`.
pub const DEC_OPTION_OFFSETS: &[(&str, i32)] = &[
    ("flags", 64),
    ("video_size", 112),
    ("pixel_format", 136),
    ("extra_hw_frames", 572),
];

pub fn check_offsets(found: &[(&str, Option<i32>)]) -> Result<(), AbiError> {
    for &(field, want) in DEC_OPTION_OFFSETS {
        let value = found
            .iter()
            .find(|(name, _)| *name == field)
            .and_then(|(_, value)| *value);
        if value != Some(want) {
            return Err(AbiError::Offset {
                field,
                found: value.unwrap_or(-1),
                want,
            });
        }
    }
    Ok(())
}

/// Public layout offsets, in bytes. Opaque C structures are never mirrored as
/// Rust structs; all raw access in the Windows implementation goes through
/// these constants.
pub mod off {
    pub mod ctx {
        pub const FLAGS: usize = 64;
        pub const WIDTH: usize = 112;
        pub const HEIGHT: usize = 116;
        pub const PIX_FMT: usize = 136;
        pub const HW_DEVICE_CTX: usize = 560;
        pub const EXTRA_HW_FRAMES: usize = 572;
    }
    pub mod frame {
        pub const DATA: usize = 0;
        pub const WIDTH: usize = 104;
        pub const HEIGHT: usize = 108;
        pub const FORMAT: usize = 116;
        pub const PTS: usize = 136;
    }
    pub mod packet {
        pub const PTS: usize = 8;
        pub const DATA: usize = 24;
        pub const SIZE: usize = 32;
        pub const FLAGS: usize = 40;
    }
    pub mod buffer_ref {
        pub const DATA: usize = 8;
    }
    pub mod hwdev {
        pub const TYPE: usize = 8;
        pub const HWCTX: usize = 16;
    }
    pub mod d3d11_dev {
        pub const DEVICE: usize = 0;
    }
    pub mod option {
        pub const OFFSET: usize = 16;
    }
    /// `AVCodecHWConfig { pix_fmt: i32, methods: i32, device_type: i32 }`.
    pub mod hw_config {
        pub const PIX_FMT: usize = 0;
        pub const METHODS: usize = 4;
        pub const DEVICE_TYPE: usize = 8;
    }
}

pub const AV_PIX_FMT_D3D11: i32 = 171;
pub const AV_HWDEVICE_TYPE_D3D11VA: i32 = 7;
pub const AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX: i32 = 0x01;
pub const AV_PKT_FLAG_KEY: i32 = 1;
pub const AVERROR_EAGAIN: i32 = -11;
pub const AVERROR_EOF: i32 = -541_478_725;
pub const AVERROR_INVALIDDATA: i32 = -1_094_995_529;

/// Decoder name used with `avcodec_find_decoder_by_name`.
pub fn decoder_name(codec: VideoCodec) -> &'static CStr {
    match codec {
        VideoCodec::Hevc => c"hevc",
        VideoCodec::H264 => c"h264",
    }
}

/// Options are applied in this order before opening the decoder.
pub const DECODER_OPTIONS: &[(&str, &str)] = &[
    ("threads", "1"),
    ("flags", "+low_delay"),
    ("extra_hw_frames", "2"),
];

pub fn hw_config_is_d3d11(pix_fmt: i32, methods: i32, device_type: i32) -> bool {
    pix_fmt == AV_PIX_FMT_D3D11
        && device_type == AV_HWDEVICE_TYPE_D3D11VA
        && methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX != 0
}

pub fn err_name(code: i32) -> String {
    let known = match code {
        AVERROR_INVALIDDATA => "INVALIDDATA",
        AVERROR_EAGAIN => "EAGAIN",
        AVERROR_EOF => "EOF",
        _ => {
            if code >= 0 {
                return code.to_string();
            }
            if code >= -255 {
                return format!("errno {}", -code);
            }
            let bytes = code.unsigned_abs().to_le_bytes();
            return if bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
                bytes.into_iter().map(char::from).collect()
            } else {
                format!("error {code}")
            };
        }
    };
    known.to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    Error,
    Warn,
    Debug,
    Trace,
}

pub fn log_target(level: i32) -> Option<LogTarget> {
    match level {
        0..=16 => Some(LogTarget::Error),
        17..=24 => Some(LogTarget::Warn),
        25..=32 => Some(LogTarget::Debug),
        33..=48 => Some(LogTarget::Trace),
        _ => None,
    }
}

pub fn clean_log_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .trim_end()
        .chars()
        .take(512)
        .map(|ch| {
            if ch.is_control() && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAdmit {
    Pass,
    Drop,
    PassWithSuppressed(u32),
}

pub struct LogLimiter {
    per_sec: u32,
    start: Option<u64>,
    passed: u32,
    suppressed: u32,
}

impl LogLimiter {
    pub fn new(per_sec: u32) -> Self {
        Self {
            per_sec,
            start: None,
            passed: 0,
            suppressed: 0,
        }
    }

    pub fn admit(&mut self, now_us: u64) -> LogAdmit {
        if self
            .start
            .is_some_and(|start| now_us.saturating_sub(start) >= 1_000_000)
        {
            self.start = Some(now_us);
            self.passed = 1;
            let suppressed = std::mem::take(&mut self.suppressed);
            return if suppressed == 0 {
                LogAdmit::Pass
            } else {
                LogAdmit::PassWithSuppressed(suppressed)
            };
        }
        self.start.get_or_insert(now_us);
        if self.passed < self.per_sec {
            self.passed += 1;
            LogAdmit::Pass
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
            LogAdmit::Drop
        }
    }
}

/// Result of associating submitted packet metadata with decoded output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Shown,
    Failed,
}

/// FIFO packet/output pairing with bounded memory. FFmpeg may emit a later
/// presentation timestamp while older packets are still pending, so output
/// settles every queued item through the matching timestamp.
pub struct OutputMatcher<T> {
    entries: VecDeque<(u32, T)>,
    cap: usize,
}

impl<T> OutputMatcher<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            cap,
        }
    }

    /// Record a submitted packet. Entries evicted by the capacity limit are
    /// returned as failed in oldest-first order.
    pub fn push(&mut self, frame_id: u32, value: T) -> Vec<(T, Outcome)> {
        self.entries.push_back((frame_id, value));
        let mut done = Vec::new();
        while self.entries.len() > self.cap {
            if let Some((_, value)) = self.entries.pop_front() {
                done.push((value, Outcome::Failed));
            }
        }
        done
    }

    /// Settle packets through the output timestamp. Queue order is the
    /// monotonic ordering, including the u32 wrap boundary.
    pub fn output(&mut self, frame_id: u32) -> Vec<(T, Outcome)> {
        let Some(index) = self
            .entries
            .iter()
            .position(|(queued_id, _)| *queued_id == frame_id)
        else {
            return Vec::new();
        };
        let mut done = Vec::with_capacity(index + 1);
        for _ in 0..index {
            if let Some((_, value)) = self.entries.pop_front() {
                done.push((value, Outcome::Failed));
            }
        }
        if let Some((_, value)) = self.entries.pop_front() {
            done.push((value, Outcome::Shown));
        }
        done
    }

    /// Settle one packet as failed, preserving all other queued entries.
    pub fn fail(&mut self, frame_id: u32) -> Vec<(T, Outcome)> {
        let Some(index) = self
            .entries
            .iter()
            .position(|(queued_id, _)| *queued_id == frame_id)
        else {
            return Vec::new();
        };
        self.entries
            .remove(index)
            .map(|(_, value)| vec![(value, Outcome::Failed)])
            .unwrap_or_default()
    }

    pub fn fail_all(&mut self) -> Vec<(T, Outcome)> {
        self.entries
            .drain(..)
            .map(|(_, value)| (value, Outcome::Failed))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
