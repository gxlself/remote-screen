#![cfg(windows)]

use host_fake::clip::Clip;
use rs_desktop::{
    platform::VideoCodec,
    win_ffmpeg::{can_decode, create_device, device_supports, FfDecLib, FfDecoder},
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;

fn library() -> std::sync::Arc<FfDecLib> {
    FfDecLib::shared().expect("RS_FFMPEG_DIR must point to an LGPL FFmpeg 8.x build")
}

#[test]
fn load_and_versions() {
    let lib = library();
    assert_eq!(lib.versions().avcodec >> 16, 62);
    assert!(lib.versions().license.starts_with("LGPL"));
    eprintln!("FFmpeg directory: {}", lib.versions().dir.display());
    eprintln!(
        "FFmpeg configuration: {}",
        lib.versions()
            .configuration
            .chars()
            .take(200)
            .collect::<String>()
    );
}

#[test]
fn supports_hevc_and_h264() {
    let lib = library();
    assert!(lib.supports(VideoCodec::Hevc));
    assert!(lib.supports(VideoCodec::H264));
}

#[test]
fn device_and_can_decode() {
    let device = create_device().expect("D3D11 video device");
    assert!(device_supports(&device, VideoCodec::Hevc));
    assert!(can_decode(VideoCodec::Hevc));
    assert_eq!(can_decode(VideoCodec::Hevc), can_decode(VideoCodec::Hevc));
}

#[test]
fn decode_bundled_hevc_clip() {
    let lib = library();
    let device = create_device().expect("D3D11 video device");
    let mut decoder = FfDecoder::open(&lib, VideoCodec::Hevc, &device).expect("open HEVC");
    let clip = Clip::bundled();
    let mut frames = Vec::new();
    let mut max_calls = 0usize;
    for index in 0..60usize {
        let mut calls = 1usize;
        let output = decoder
            .decode(index as u32, clip.au(index), clip.is_keyframe(index))
            .expect("decode access unit");
        frames.extend(output);
        calls += frames.len();
        max_calls = max_calls.max(calls);
    }
    frames.extend(decoder.drain());
    assert_eq!(frames.len(), 60);
    let mut ids: Vec<_> = frames.iter().map(|frame| frame.frame_id).collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..60).collect::<Vec<_>>());
    for frame in &frames {
        assert_eq!((frame.width, frame.height), (640, 360));
        let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the decoded texture is a live D3D11 resource owned by the frame.
        unsafe { frame.texture.GetDesc(&mut desc) };
        assert_eq!(desc.Format, DXGI_FORMAT_NV12);
        assert!(frame.array_index < desc.ArraySize);
    }
    eprintln!("maximum decode output call index: {max_calls}");
}

#[test]
fn garbage_then_keyframe_recovers() {
    let lib = library();
    let device = create_device().expect("D3D11 video device");
    let mut decoder = FfDecoder::open(&lib, VideoCodec::Hevc, &device).expect("open HEVC");
    let garbage = vec![0xff; 1000];
    let first = decoder.decode(10, &garbage, false);
    assert!(first.is_err() || first.as_ref().is_ok_and(Vec::is_empty));
    let clip = Clip::bundled();
    let _ = decoder.decode(0, clip.au(0), true);
    let _ = decoder.decode(1, clip.au(1), false);
    assert!(!decoder.drain().is_empty());
}

#[test]
fn open_close_repeat() {
    let lib = library();
    let device = create_device().expect("D3D11 video device");
    let clip = Clip::bundled();
    for _ in 0..20 {
        let mut decoder = FfDecoder::open(&lib, VideoCodec::Hevc, &device).expect("open HEVC");
        for index in 0..3usize {
            let _ = decoder.decode(index as u32, clip.au(index), clip.is_keyframe(index));
        }
    }
}
