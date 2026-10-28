//! FFmpeg 8.x (avcodec 62 / avutil 60), x86_64 Windows ABI.
//!
//! Offsets are the measured 8.1.1 public layouts. These structures contain no
//! C `long`, so the relevant LLP64 and LP64 layouts agree. AVOptions validate
//! the context layout before any hardware allocation; frames are checked again
//! at runtime. Keep policy and byte-layout decisions here, independent of FFI.

use std::path::{Path, PathBuf};

pub const AVCODEC_MAJOR: u32 = 62;
pub const AVUTIL_MAJOR: u32 = 60;
pub const AVUTIL_DLL: &str = "avutil-60.dll";
pub const AVCODEC_DLL: &str = "avcodec-62.dll";
pub const FFMPEG_DIR_ENV: &str = "RS_FFMPEG_DIR";
pub const FFMPEG_SUBDIR: &str = "ffmpeg";

pub fn ffmpeg_dir(exe_dir: &Path, env_value: Option<&str>) -> PathBuf {
    match env_value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => exe_dir.join(FFMPEG_SUBDIR),
    }
}

pub fn version_parts(v: u32) -> (u32, u32, u32) {
    (v >> 16, (v >> 8) & 255, v & 255)
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
                text: text.into(),
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

pub const OPTION_OFFSETS: &[(&str, i32)] = &[
    ("b", 56),
    ("flags", 64),
    ("time_base", 84),
    ("video_size", 112),
    ("pixel_format", 136),
    ("bf", 200),
    ("g", 332),
    ("bufsize", 448),
    ("maxrate", 464),
    ("extra_hw_frames", 572),
];

pub fn check_offsets(found: &[(&str, Option<i32>)]) -> Result<(), AbiError> {
    for &(field, want) in OPTION_OFFSETS {
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

/// Public layout offsets, in bytes. Opaque C structures are never mirrored in Rust.
pub mod off {
    pub mod ctx {
        pub const BIT_RATE: usize = 56;
        pub const FLAGS: usize = 64;
        pub const TIME_BASE: usize = 84;
        pub const WIDTH: usize = 112;
        pub const HEIGHT: usize = 116;
        pub const PIX_FMT: usize = 136;
        pub const MAX_B_FRAMES: usize = 200;
        pub const GOP_SIZE: usize = 332;
        pub const RC_BUFFER_SIZE: usize = 448;
        pub const RC_MAX_RATE: usize = 464;
        pub const HW_FRAMES_CTX: usize = 552;
        pub const HW_DEVICE_CTX: usize = 560;
        pub const EXTRA_HW_FRAMES: usize = 572;
    }
    pub mod frame {
        pub const DATA: usize = 0;
        pub const LINESIZE: usize = 64;
        pub const WIDTH: usize = 104;
        pub const HEIGHT: usize = 108;
        pub const FORMAT: usize = 116;
        pub const PICT_TYPE: usize = 120;
        pub const PTS: usize = 136;
        pub const BUF: usize = 184;
        pub const FLAGS: usize = 276;
        pub const HW_FRAMES_CTX: usize = 328;
    }
    pub mod packet {
        pub const BUF: usize = 0;
        pub const PTS: usize = 8;
        pub const DTS: usize = 16;
        pub const DATA: usize = 24;
        pub const SIZE: usize = 32;
        pub const STREAM_INDEX: usize = 36;
        pub const FLAGS: usize = 40;
    }
    pub mod buffer_ref {
        pub const BUFFER: usize = 0;
        pub const DATA: usize = 8;
        pub const SIZE: usize = 16;
    }
    pub mod hwdev {
        pub const TYPE: usize = 8;
        pub const HWCTX: usize = 16;
    }
    pub mod hwframes {
        pub const DEVICE_REF: usize = 8;
        pub const HWCTX: usize = 24;
        pub const INITIAL_POOL_SIZE: usize = 56;
        pub const FORMAT: usize = 60;
        pub const SW_FORMAT: usize = 64;
        pub const WIDTH: usize = 68;
        pub const HEIGHT: usize = 72;
    }
    pub mod d3d11_dev {
        pub const DEVICE: usize = 0;
        pub const DEVICE_CONTEXT: usize = 8;
        pub const VIDEO_DEVICE: usize = 16;
        pub const VIDEO_CONTEXT: usize = 24;
        pub const LOCK: usize = 32;
        pub const UNLOCK: usize = 40;
        pub const LOCK_CTX: usize = 48;
    }
    pub mod d3d11_frames {
        pub const TEXTURE: usize = 0;
        pub const BIND_FLAGS: usize = 8;
        pub const MISC_FLAGS: usize = 12;
    }
    pub mod option {
        pub const OFFSET: usize = 16;
    }
    pub mod side_data {
        pub const DATA: usize = 8;
    }
}

pub const AV_PIX_FMT_NV12: i32 = 23;
pub const AV_PIX_FMT_D3D11: i32 = 171;
pub const AV_PIX_FMT_QSV: i32 = 114;
pub const AV_HWDEVICE_TYPE_QSV: i32 = 5;
pub const AV_HWDEVICE_TYPE_D3D11VA: i32 = 7;
pub const AV_PICTURE_TYPE_NONE: i32 = 0;
pub const AV_PICTURE_TYPE_I: i32 = 1;
pub const AV_PKT_FLAG_KEY: i32 = 1;
pub const AV_OPT_SEARCH_CHILDREN: i32 = 1;
pub const AV_HWFRAME_MAP_WRITE: i32 = 2;
pub const AV_HWFRAME_MAP_OVERWRITE: i32 = 4;
pub const AV_FRAME_DATA_REGIONS_OF_INTEREST: i32 = 18;
pub const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
pub const AV_LOG_QUIET: i32 = -8;
pub const AV_LOG_PANIC: i32 = 0;
pub const AV_LOG_FATAL: i32 = 8;
pub const AV_LOG_ERROR: i32 = 16;
pub const AV_LOG_WARNING: i32 = 24;
pub const AV_LOG_INFO: i32 = 32;
pub const AV_LOG_VERBOSE: i32 = 40;
pub const AV_LOG_DEBUG: i32 = 48;
pub const AV_LOG_TRACE: i32 = 56;
pub const AVERROR_EAGAIN: i32 = -11;
pub const AVERROR_EINVAL: i32 = -22;
pub const AVERROR_ENOMEM: i32 = -12;
pub const AVERROR_ENOSYS: i32 = -40;
pub const AVERROR_EOF: i32 = -541478725;
pub const AVERROR_OPTION_NOT_FOUND: i32 = -1414549496;
pub const AVERROR_ENCODER_NOT_FOUND: i32 = -1129203192;
pub const AVERROR_EXTERNAL: i32 = -542398533;
pub const AVERROR_UNKNOWN: i32 = -1313558101;
pub const RECEIVE_WAIT_US: u64 = 20_000;
pub const FLUSH_WAIT_US: u64 = 100_000;

/// Pointer field to validate in a native hardware frame (Win64 pointer size).
/// FFmpeg 8.0/8.1.1 qsv_get_buffer stores mfxFrameSurface1 in data[3];
/// qsv_map_from exposes the mapped D3D11 texture in data[0]. Checking data[0]
/// on the native QSV frame would incorrectly reject every QSV encoder.
pub fn hw_frame_data_offset(format: i32) -> usize {
    off::frame::DATA + if format == AV_PIX_FMT_QSV { 3 * 8 } else { 0 }
}

pub fn err_name(code: i32) -> String {
    let known = match code {
        AVERROR_EAGAIN => "EAGAIN",
        AVERROR_EINVAL => "EINVAL",
        AVERROR_ENOMEM => "ENOMEM",
        AVERROR_ENOSYS => "ENOSYS",
        AVERROR_EOF => "EOF",
        AVERROR_OPTION_NOT_FOUND => "OPTION_NOT_FOUND",
        AVERROR_ENCODER_NOT_FOUND => "ENCODER_NOT_FOUND",
        AVERROR_EXTERNAL => "EXTERNAL",
        AVERROR_UNKNOWN => "UNKNOWN",
        _ => {
            if code >= 0 {
                return code.to_string();
            }
            if code >= -255 {
                return format!("errno {}", -code);
            }
            let bytes = code.unsigned_abs().to_le_bytes();
            return if bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
                bytes.into_iter().map(char::from).collect()
            } else {
                format!("error {code}")
            };
        }
    };
    known.into()
}

pub fn open_error_is_unsupported(code: i32) -> bool {
    matches!(
        code,
        AVERROR_ENCODER_NOT_FOUND | AVERROR_OPTION_NOT_FOUND | AVERROR_ENOSYS | AVERROR_EINVAL
    )
}

pub const ROI_SIZE: usize = 28;
pub fn roi_bytes(width: u32, height: u32, qoffset: (i32, i32)) -> [u8; ROI_SIZE] {
    let words = [
        ROI_SIZE as u32,
        0,
        height,
        0,
        width,
        qoffset.0 as u32,
        qoffset.1 as u32,
    ];
    let mut bytes = [0; ROI_SIZE];
    for (out, word) in bytes.chunks_exact_mut(4).zip(words) {
        out.copy_from_slice(&word.to_le_bytes());
    }
    bytes
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
        0..=AV_LOG_ERROR => Some(LogTarget::Error),
        17..=AV_LOG_WARNING => Some(LogTarget::Warn),
        25..=AV_LOG_INFO => Some(LogTarget::Debug),
        33..=AV_LOG_DEBUG => Some(LogTarget::Trace),
        _ => None,
    }
}

pub fn clean_log_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .trim_end()
        .chars()
        .take(512)
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
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
