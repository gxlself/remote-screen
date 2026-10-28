//! Pure encoder candidate ordering for Windows hosts.

use crate::config::VideoEncoder;
use rsp_core::pb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    Nvenc,
    Amf,
    Qsv,
    MediaFoundation,
    OpenH264,
}

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

pub fn vendor_encoder(vendor_id: u32) -> Option<EncoderKind> {
    match vendor_id {
        VENDOR_NVIDIA => Some(EncoderKind::Nvenc),
        VENDOR_AMD => Some(EncoderKind::Amf),
        VENDOR_INTEL => Some(EncoderKind::Qsv),
        _ => None,
    }
}

pub fn supports(kind: EncoderKind, codec: pb::Codec) -> bool {
    match kind {
        EncoderKind::OpenH264 => codec == pb::Codec::H264,
        EncoderKind::Nvenc | EncoderKind::Amf | EncoderKind::Qsv | EncoderKind::MediaFoundation => {
            matches!(codec, pb::Codec::Hevc | pb::Codec::H264)
        }
    }
}

/// Build the ordered list of encoders to try for the selected codec.
pub fn candidates(
    vendor_id: Option<u32>,
    forced: VideoEncoder,
    codec: pb::Codec,
) -> Vec<EncoderKind> {
    let codec = normalize_codec(codec);
    if let Some(kind) = forced_kind(forced) {
        return if supports(kind, codec) {
            vec![kind]
        } else {
            Vec::new()
        };
    }

    let mut result = Vec::with_capacity(3);
    if let Some(kind) = vendor_id.and_then(vendor_encoder) {
        if supports(kind, codec) {
            result.push(kind);
        }
    }
    if supports(EncoderKind::MediaFoundation, codec) {
        result.push(EncoderKind::MediaFoundation);
    }
    if supports(EncoderKind::OpenH264, codec) {
        result.push(EncoderKind::OpenH264);
    }
    result
}

pub fn ffmpeg_encoder_name(kind: EncoderKind, codec: pb::Codec) -> Option<&'static str> {
    match kind {
        EncoderKind::Nvenc => match codec {
            pb::Codec::Hevc => Some("hevc_nvenc"),
            pb::Codec::H264 => Some("h264_nvenc"),
            pb::Codec::Unspecified => None,
        },
        EncoderKind::Amf => match codec {
            pb::Codec::Hevc => Some("hevc_amf"),
            pb::Codec::H264 => Some("h264_amf"),
            pb::Codec::Unspecified => None,
        },
        EncoderKind::Qsv => match codec {
            pb::Codec::Hevc => Some("hevc_qsv"),
            pb::Codec::H264 => Some("h264_qsv"),
            pb::Codec::Unspecified => None,
        },
        EncoderKind::MediaFoundation | EncoderKind::OpenH264 => None,
    }
}

fn forced_kind(forced: VideoEncoder) -> Option<EncoderKind> {
    match forced {
        VideoEncoder::Auto => None,
        VideoEncoder::Nvenc => Some(EncoderKind::Nvenc),
        VideoEncoder::Amf => Some(EncoderKind::Amf),
        VideoEncoder::Qsv => Some(EncoderKind::Qsv),
        VideoEncoder::Mf => Some(EncoderKind::MediaFoundation),
        VideoEncoder::OpenH264 => Some(EncoderKind::OpenH264),
    }
}

fn normalize_codec(codec: pb::Codec) -> pb::Codec {
    match codec {
        pb::Codec::Unspecified => pb::Codec::Hevc,
        pb::Codec::Hevc | pb::Codec::H264 => codec,
    }
}
