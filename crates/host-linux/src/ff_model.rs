//! Pure Linux video encoder selection and FFmpeg option tables.
//!
//! The platform glue turns the [`GpuNode`] values discovered by the Linux
//! renderer probe into this module's input.  Keeping the policy here free of
//! FFmpeg and device calls makes it testable on every target.

use std::path::{Path, PathBuf};

use rsp_core::pb::Codec;

pub use host_core::encode_model::{
    validate, vbv_bits, EncParams, FfOpt, ParamError, RateCost, ENCODER_ENV,
};
pub use host_core::ffmpeg_abi::{
    AVERROR_EAGAIN, AVERROR_EINVAL, AVERROR_ENCODER_NOT_FOUND, AVERROR_ENOMEM, AVERROR_EOF,
    AVERROR_EXTERNAL, AVERROR_OPTION_NOT_FOUND, AVERROR_UNKNOWN,
};

// ---- backends and preferences ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxBackend {
    Vaapi,
    Nvenc,
    OpenH264,
}

impl LinuxBackend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vaapi => "vaapi",
            Self::Nvenc => "nvenc",
            Self::OpenH264 => "openh264",
        }
    }

    pub fn is_ffmpeg(self) -> bool {
        !matches!(self, Self::OpenH264)
    }

    pub fn rate_cost(self) -> RateCost {
        match self {
            Self::Vaapi | Self::OpenH264 => RateCost::Reopen,
            Self::Nvenc => RateCost::Idr,
        }
    }

    pub fn supports(self, codec: Codec) -> bool {
        match self {
            Self::OpenH264 => codec == Codec::H264,
            Self::Vaapi | Self::Nvenc => matches!(codec, Codec::H264 | Codec::Hevc),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinuxEncoderPref {
    #[default]
    Auto,
    Vaapi,
    Nvenc,
    OpenH264,
}

pub fn parse_pref(s: &str) -> Option<LinuxEncoderPref> {
    match s.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(LinuxEncoderPref::Auto),
        "vaapi" => Some(LinuxEncoderPref::Vaapi),
        "nvenc" => Some(LinuxEncoderPref::Nvenc),
        "openh264" => Some(LinuxEncoderPref::OpenH264),
        _ => None,
    }
}

// ---- candidate ordering ----

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuNode {
    pub dev_path: PathBuf,
    pub vendor: u32,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub backend: LinuxBackend,
    pub render_node: Option<PathBuf>,
}

fn normalized_codec(codec: Codec) -> Codec {
    match codec {
        Codec::Unspecified => Codec::Hevc,
        Codec::H264 | Codec::Hevc => codec,
    }
}

fn forced_backend(pref: LinuxEncoderPref) -> Option<LinuxBackend> {
    match pref {
        LinuxEncoderPref::Auto => None,
        LinuxEncoderPref::Vaapi => Some(LinuxBackend::Vaapi),
        LinuxEncoderPref::Nvenc => Some(LinuxBackend::Nvenc),
        LinuxEncoderPref::OpenH264 => Some(LinuxBackend::OpenH264),
    }
}

pub fn candidates(
    nodes: &[GpuNode],
    pref: LinuxEncoderPref,
    codec: Codec,
    ffmpeg_ok: bool,
) -> Vec<Candidate> {
    let codec = normalized_codec(codec);
    let mut result = Vec::with_capacity(3);

    if ffmpeg_ok
        && nodes
            .iter()
            .any(|node| node.vendor == VENDOR_NVIDIA && node.driver.as_deref() != Some("nouveau"))
    {
        result.push(Candidate {
            backend: LinuxBackend::Nvenc,
            render_node: None,
        });
    }

    if ffmpeg_ok {
        if let Some(node) = nodes
            .iter()
            .find(|node| matches!(node.vendor, VENDOR_INTEL | VENDOR_AMD))
        {
            result.push(Candidate {
                backend: LinuxBackend::Vaapi,
                render_node: Some(node.dev_path.clone()),
            });
        }
    }

    if LinuxBackend::OpenH264.supports(codec) {
        result.push(Candidate {
            backend: LinuxBackend::OpenH264,
            render_node: None,
        });
    }

    if let Some(forced) = forced_backend(pref) {
        if let Some(index) = result
            .iter()
            .position(|candidate| candidate.backend == forced)
        {
            let candidate = result.remove(index);
            result.insert(0, candidate);
        }
    }

    result
}

// ---- FFmpeg names and options ----

pub const VAAPI_SW_FORMAT: &str = "nv12";
pub const SRC_PIX_FMT: &str = "bgr0";
pub const HWDEVICE_VAAPI: &str = "vaapi";

pub fn ffmpeg_codec_name(b: LinuxBackend, codec: Codec) -> Option<&'static str> {
    match (b, codec) {
        (LinuxBackend::Vaapi, Codec::Hevc) => Some("hevc_vaapi"),
        (LinuxBackend::Vaapi, Codec::H264) => Some("h264_vaapi"),
        (LinuxBackend::Nvenc, Codec::Hevc) => Some("hevc_nvenc"),
        (LinuxBackend::Nvenc, Codec::H264) => Some("h264_nvenc"),
        (LinuxBackend::OpenH264, _) | (_, Codec::Unspecified) => None,
    }
}

pub fn input_pix_fmt(b: LinuxBackend) -> Option<&'static str> {
    match b {
        LinuxBackend::Vaapi => Some(HWDEVICE_VAAPI),
        LinuxBackend::Nvenc => Some(SRC_PIX_FMT),
        LinuxBackend::OpenH264 => None,
    }
}

fn opt(name: &'static str, value: impl Into<String>, required: bool) -> FfOpt {
    FfOpt {
        name,
        value: value.into(),
        required,
    }
}

fn common_options(b: LinuxBackend, p: &EncParams) -> Vec<FfOpt> {
    let Some(pixel_format) = input_pix_fmt(b) else {
        return Vec::new();
    };

    vec![
        opt("pixel_format", pixel_format, true),
        opt("video_size", format!("{}x{}", p.width, p.height), true),
        opt("time_base", format!("1/{}", p.fps), true),
        opt("b", p.bitrate_bps.to_string(), true),
        opt("maxrate", p.bitrate_bps.to_string(), true),
        opt("bufsize", vbv_bits(p.bitrate_bps, p.fps).to_string(), true),
        opt("bf", "0", true),
        opt("g", "2147483647", true),
        opt("keyint_min", "2147483647", false),
        opt("flags", "+cgop+low_delay", false),
        opt("color_range", "tv", true),
        opt("colorspace", "bt709", true),
        opt("color_primaries", "bt709", true),
        opt("color_trc", "bt709", true),
    ]
}

pub fn ffmpeg_options(b: LinuxBackend, p: &EncParams) -> Vec<FfOpt> {
    let mut result = common_options(b, p);
    match b {
        LinuxBackend::Vaapi => {
            result.extend([
                opt("rc_mode", "CBR", true),
                opt("async_depth", "1", false),
                opt("aud", "0", false),
            ]);
            if p.codec == Codec::H264 {
                result.extend([opt("profile", "high", false), opt("sei", "0", false)]);
            } else {
                result.push(opt("profile", "main", false));
            }
        }
        LinuxBackend::Nvenc => {
            result.extend([
                opt("preset", "p1", true),
                opt("tune", "ull", true),
                opt("rc", "cbr", true),
                opt("zerolatency", "1", true),
                opt("delay", "0", true),
                opt("forced-idr", "1", true),
                opt(
                    "profile",
                    if p.codec == Codec::H264 {
                        "high"
                    } else {
                        "main"
                    },
                    false,
                ),
            ]);
        }
        LinuxBackend::OpenH264 => {}
    }
    result
}

pub fn ffmpeg_options_retry(b: LinuxBackend, p: &EncParams) -> Option<Vec<FfOpt>> {
    match b {
        LinuxBackend::Vaapi => {
            let mut result = ffmpeg_options(b, p);
            result.push(opt("low_power", "1", false));
            Some(result)
        }
        LinuxBackend::Nvenc | LinuxBackend::OpenH264 => None,
    }
}

pub fn ffmpeg_rate_options(b: LinuxBackend, bitrate_bps: u64, fps: u32) -> Vec<FfOpt> {
    match b {
        LinuxBackend::Nvenc => vec![
            opt("b", bitrate_bps.to_string(), true),
            opt("maxrate", bitrate_bps.to_string(), true),
            opt("bufsize", vbv_bits(bitrate_bps, fps).to_string(), true),
        ],
        LinuxBackend::Vaapi | LinuxBackend::OpenH264 => Vec::new(),
    }
}

// ---- FFmpeg error classification ----

pub const AVERROR_ENOENT: i32 = -2;
pub const AVERROR_EACCES: i32 = -13;
pub const AVERROR_ENOSYS_LINUX: i32 = -38;

pub fn open_error_is_unsupported(code: i32) -> bool {
    matches!(
        code,
        AVERROR_ENCODER_NOT_FOUND
            | AVERROR_OPTION_NOT_FOUND
            | AVERROR_EINVAL
            | AVERROR_ENOSYS_LINUX
            | AVERROR_ENOENT
            | AVERROR_EACCES
            | AVERROR_EXTERNAL
            | AVERROR_UNKNOWN
    )
}

pub fn err_name(code: i32) -> String {
    match code {
        AVERROR_ENOSYS_LINUX => "ENOSYS".to_owned(),
        AVERROR_ENOENT => "ENOENT".to_owned(),
        AVERROR_EACCES => "EACCES".to_owned(),
        // The Windows ABI helper calls -40 ENOSYS.  On Linux it is errno 40,
        // so keep the platform distinction visible to callers.
        host_core::ffmpeg_abi::AVERROR_ENOSYS => "errno 40".to_owned(),
        _ => host_core::ffmpeg_abi::err_name(code),
    }
}

// ---- FFmpeg shared-library lookup ----

pub const AVUTIL_SO: &str = "libavutil.so.60";
pub const SWSCALE_SO: &str = "libswscale.so.9";
pub const AVCODEC_SO: &str = "libavcodec.so.62";
pub const SWSCALE_MAJOR: u32 = 9;
pub const LIB_LOAD_ORDER: [&str; 3] = [AVUTIL_SO, SWSCALE_SO, AVCODEC_SO];

pub fn lib_dirs(exe_dir: &Path, env: Option<&str>) -> Vec<PathBuf> {
    if let Some(value) = env.map(str::trim).filter(|value| !value.is_empty()) {
        return vec![PathBuf::from(value)];
    }
    vec![exe_dir.join("../lib/ffmpeg"), exe_dir.join("ffmpeg")]
}

pub fn pick_lib_dir(dirs: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    dirs.iter()
        .find(|dir| LIB_LOAD_ORDER.iter().all(|name| exists(&dir.join(name))))
        .cloned()
}
