#![cfg(feature = "desktop-it")]

use std::path::PathBuf;

use host_core::{encode_model::au_is_key, ffmpeg_abi};
use host_linux::{
    ff_model::{EncParams, LinuxBackend},
    ffmpeg::{FfEncoder, FfLib, FfOpenReq, FfPacket},
    frame::LinuxFrame,
};
use rsp_core::pb::Codec;

fn ffmpeg_dir() -> PathBuf {
    std::env::var_os("RS_FFMPEG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set RS_FFMPEG_DIR to the FFmpeg 8 library directory"))
}

fn gpu() -> String {
    let value = std::env::var("RS_IT_GPU")
        .unwrap_or_else(|_| panic!("set RS_IT_GPU to vaapi, nvenc, or none"));
    assert!(
        matches!(value.as_str(), "vaapi" | "nvenc" | "none"),
        "RS_IT_GPU must be vaapi, nvenc, or none"
    );
    value
}

fn params() -> EncParams {
    EncParams {
        codec: Codec::Hevc,
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_bps: 4_000_000,
    }
}

fn request(backend: LinuxBackend, render_node: Option<&std::path::Path>) -> FfOpenReq<'_> {
    FfOpenReq {
        backend,
        params: params(),
        src_width: 1920,
        src_height: 1080,
        render_node,
    }
}

fn source(width: u32, height: u32) -> LinuxFrame {
    LinuxFrame {
        width,
        height,
        stride: width * 4,
        generation: 0,
        pixels: vec![0; width as usize * height as usize * 4],
    }
}

fn first_key_packet(packets: &[FfPacket]) -> Option<&FfPacket> {
    packets.iter().find(|packet| packet.keyframe)
}

#[test]
fn load_and_abi() {
    let lib = FfLib::load(&ffmpeg_dir()).expect("FFmpeg 8 LGPL libraries must load");
    assert_eq!(ffmpeg_abi::version_parts(lib.versions().avcodec).0, 62);
    assert_eq!(ffmpeg_abi::version_parts(lib.versions().avutil).0, 60);
    assert_eq!(ffmpeg_abi::version_parts(lib.versions().swscale).0, 9);
    assert!(lib.has_encoder("hevc_vaapi"));
    assert!(lib.has_encoder("h264_nvenc"));
    assert!(lib.pix_fmt("bgr0").is_some());
    assert!(lib.pix_fmt("nv12").is_some());
    assert!(lib.pix_fmt("vaapi").is_some());
    assert_eq!(lib.pix_fmt("nv12"), Some(ffmpeg_abi::AV_PIX_FMT_NV12));
}

#[test]
fn bad_dir() {
    let error = match FfLib::load(std::path::Path::new("/nonexistent")) {
        Ok(_) => panic!("nonexistent directory must fail"),
        Err(error) => error,
    };
    assert!(matches!(error, host_linux::ffmpeg::FfError::Load(_)));
}

#[test]
fn open_by_gpu() {
    let mode = gpu();
    let lib = FfLib::load(&ffmpeg_dir()).expect("FFmpeg 8 LGPL libraries must load");
    if mode == "none" {
        let vaapi = FfEncoder::open(&lib, &request(LinuxBackend::Vaapi, None));
        assert!(matches!(
            vaapi,
            Err(host_linux::ffmpeg::FfError::Unsupported(_))
        ));
        let nvenc = FfEncoder::open(&lib, &request(LinuxBackend::Nvenc, None));
        assert!(matches!(
            nvenc,
            Err(host_linux::ffmpeg::FfError::Unsupported(_))
        ));
        return;
    }
    let backend = if mode == "vaapi" {
        LinuxBackend::Vaapi
    } else {
        LinuxBackend::Nvenc
    };
    let node = std::path::Path::new("/dev/dri/renderD128");
    let mut encoder = FfEncoder::open(&lib, &request(backend, (mode == "vaapi").then_some(node)))
        .expect("selected GPU encoder must open");
    let frame = source(1920, 1080);
    let mut packets = Vec::new();
    for index in 0..10 {
        packets.extend(
            encoder
                .encode(&frame, index, index == 0)
                .expect("frame must encode"),
        );
    }
    assert!(
        packets.len() >= 9,
        "expected delayed encoder to produce at least 9 packets"
    );
    let first = first_key_packet(&packets).expect("first IDR packet must be marked keyframe");
    assert!(au_is_key(Codec::Hevc, &first.data));
}

#[test]
fn size_mismatch() {
    let mode = gpu();
    let lib = FfLib::load(&ffmpeg_dir()).expect("FFmpeg 8 LGPL libraries must load");
    if mode == "none" {
        let error = match FfEncoder::open(&lib, &request(LinuxBackend::Vaapi, None)) {
            Ok(_) => panic!("VA-API without render node must be unsupported"),
            Err(error) => error,
        };
        assert!(matches!(error, host_linux::ffmpeg::FfError::Unsupported(_)));
        return;
    }
    let backend = if mode == "vaapi" {
        LinuxBackend::Vaapi
    } else {
        LinuxBackend::Nvenc
    };
    let node = std::path::Path::new("/dev/dri/renderD128");
    let mut encoder = FfEncoder::open(&lib, &request(backend, (mode == "vaapi").then_some(node)))
        .expect("selected GPU encoder must open");
    let error = encoder
        .encode(&source(1280, 720), 0, true)
        .expect_err("wrong source dimensions must fail");
    assert!(matches!(error, host_linux::ffmpeg::FfError::Failed { .. }));
}

#[test]
fn set_rate_nvenc() {
    let mode = gpu();
    let lib = FfLib::load(&ffmpeg_dir()).expect("FFmpeg 8 LGPL libraries must load");
    if mode == "nvenc" {
        let mut encoder = FfEncoder::open(&lib, &request(LinuxBackend::Nvenc, None))
            .expect("NVENC encoder must open");
        encoder
            .set_rate(4_000_000, 60)
            .expect("NVENC rate options must apply");
        encoder
            .encode(&source(1920, 1080), 0, true)
            .expect("NVENC must continue encoding after a rate update");
    } else {
        let node = std::path::Path::new("/dev/dri/renderD128");
        let mut encoder = FfEncoder::open(
            &lib,
            &request(LinuxBackend::Vaapi, (mode == "vaapi").then_some(node)),
        );
        if let Ok(ref mut encoder) = encoder {
            encoder
                .set_rate(4_000_000, 60)
                .expect("VA-API rate update is a no-op");
        } else if mode == "vaapi" {
            panic!("VA-API encoder must open when RS_IT_GPU=vaapi");
        }
    }
}
