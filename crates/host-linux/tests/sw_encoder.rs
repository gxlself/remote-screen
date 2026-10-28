#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use host_core::encode_model::EncParams;
use host_linux::enc_worker::BackendFactory;
use host_linux::sw_encoder::{bgrx_to_i420, scale_bgrx, ApiLoader, OpenH264Factory, I420};
use rsp_core::pb::Codec;

fn bgrx_image(
    w: usize,
    h: usize,
    stride: usize,
    bgrx: impl Fn(usize, usize) -> [u8; 4],
) -> Vec<u8> {
    let mut image = vec![0; stride * h];
    for y in 0..h {
        for x in 0..w {
            let start = y * stride + x * 4;
            image[start..start + 4].copy_from_slice(&bgrx(x, y));
        }
    }
    image
}

#[test]
fn i420_colors() {
    let colors = [
        ([255, 255, 255, 255], [235, 128, 128]),
        ([0, 0, 0, 255], [16, 128, 128]),
        ([0, 0, 255, 255], [63, 102, 240]),
        ([0, 255, 0, 255], [173, 42, 26]),
        ([255, 0, 0, 255], [32, 240, 118]),
    ];
    for (pixel, expected) in colors {
        let src = pixel.repeat(4);
        let mut out = I420 {
            width: 0,
            height: 0,
            y: Vec::new(),
            u: Vec::new(),
            v: Vec::new(),
        };
        bgrx_to_i420(&src, 8, 2, 2, &mut out);
        assert!(out
            .y
            .iter()
            .all(|value| (*value as i16 - expected[0]).abs() <= 1));
        assert!((out.u[0] as i16 - expected[1]).abs() <= 1);
        assert!((out.v[0] as i16 - expected[2]).abs() <= 1);
    }
}

#[test]
fn i420_chroma_average() {
    // BGRx: red, blue, blue, red.
    let src = [
        0, 0, 255, 255, 255, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0, 255,
    ];
    let mut out = I420 {
        width: 0,
        height: 0,
        y: Vec::new(),
        u: Vec::new(),
        v: Vec::new(),
    };
    bgrx_to_i420(&src, 8, 2, 2, &mut out);
    assert!((out.u[0] as i16 - 171).abs() <= 1);
    assert!((out.v[0] as i16 - 179).abs() <= 1);
}

#[test]
fn scale_down_average() {
    let src = bgrx_image(4, 4, 32, |x, y| [x as u8 * 10, y as u8 * 10, 0, 255]);
    let mut dst = vec![0; 2 * 2 * 4];
    scale_bgrx(&src, 4, 4, 32, &mut dst, 2, 2);
    assert_eq!(&dst[0..4], &[5, 5, 0, 255]);
    assert_eq!(&dst[4..8], &[25, 5, 0, 255]);
    assert_eq!(&dst[12..16], &[25, 25, 0, 255]);
}

#[test]
fn scale_up_nearest_and_boundary() {
    let src = bgrx_image(2, 2, 8, |x, y| [x as u8, y as u8, 0, 255]);
    let mut dst = vec![0; 4 * 4 * 4];
    scale_bgrx(&src, 2, 2, 8, &mut dst, 4, 4);
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                &dst[(y * 4 + x) * 4..(y * 4 + x + 1) * 4],
                [(x / 2) as u8, (y / 2) as u8, 0, 255,]
            );
        }
    }

    let src = bgrx_image(3, 3, 12, |x, y| [x as u8, y as u8, 0, 255]);
    let mut dst = vec![0; 2 * 2 * 4];
    scale_bgrx(&src, 3, 3, 12, &mut dst, 2, 2);
    assert_eq!(&dst[0..4], &[0, 0, 0, 255]);
    assert_eq!(&dst[4..8], &[2, 0, 0, 255]);
    assert_eq!(&dst[8..12], &[0, 2, 0, 255]);
    assert_eq!(&dst[12..16], &[2, 2, 0, 255]);
}

fn params(codec: Codec) -> EncParams {
    EncParams {
        codec,
        width: 640,
        height: 360,
        fps: 30,
        bitrate_bps: 2_000_000,
    }
}

#[test]
fn hevc_unsupported_before_loading() {
    let loader: ApiLoader = Arc::new(|| Err("loader should not be called".into()));
    let factory = OpenH264Factory::with_loader(loader);
    let error = factory
        .open(
            &params(Codec::Hevc),
            host_linux::enc_worker::SrcSize {
                width: 640,
                height: 360,
                stride: 2560,
            },
        )
        .err()
        .expect("HEVC must be rejected");
    assert!(matches!(
        error,
        host_linux::enc_worker::BackendError::Unsupported(_)
    ));
}

#[test]
fn blob_candidates_and_missing_blob() {
    assert_eq!(
        OpenH264Factory::blob_candidates(
            Path::new("/app/bin"),
            Path::new("/state"),
            Some(" /x.so ")
        ),
        vec![PathBuf::from("/x.so")]
    );
    assert_eq!(
        OpenH264Factory::blob_candidates(Path::new("/app/bin"), Path::new("/state"), None),
        vec![
            PathBuf::from("/app/bin/../lib/openh264/libopenh264.so.7"),
            PathBuf::from("/state/codecs/libopenh264.so.7"),
        ]
    );
    let factory = OpenH264Factory::from_blob(PathBuf::from("/nonexistent/libopenh264.so.7"));
    let error = factory
        .open(
            &params(Codec::H264),
            host_linux::enc_worker::SrcSize {
                width: 640,
                height: 360,
                stride: 2560,
            },
        )
        .err()
        .expect("missing blob must be unsupported");
    assert!(matches!(
        error,
        host_linux::enc_worker::BackendError::Unsupported(_)
    ));
}

// The source feature is intentionally opt-in: production builds only use the
// Cisco blob loader.  These smoke tests are enabled by the source-build CI job.
#[cfg(feature = "openh264-source")]
mod source_encoder {
    use super::*;
    use host_core::platform::{CapturedFrame, EncodeOpts, EncodedFrame, Encoder, EncoderConfig};
    use host_linux::enc_worker::{EncBackend, EncWorker};
    use host_linux::frame::LinuxFrame;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn api() -> Result<openh264::OpenH264API, String> {
        Ok(openh264::OpenH264API::from_source())
    }

    fn frame(width: u32, height: u32) -> CapturedFrame {
        CapturedFrame {
            display_id: 1,
            width,
            height,
            capture_ts_us: 1,
            native: Arc::new(LinuxFrame {
                width,
                height,
                stride: width * 4,
                generation: 1,
                pixels: vec![128; width as usize * height as usize * 4],
            }),
        }
    }

    fn config(codec: Codec, sink: mpsc::Sender<EncodedFrame>) -> EncoderConfig {
        EncoderConfig {
            codec,
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 2000,
            sink,
        }
    }

    #[test]
    fn encode_basic_and_scaled() {
        let factory = Arc::new(OpenH264Factory::with_loader(Arc::new(api)));
        let mut backend = factory
            .open(
                &params(Codec::H264),
                host_linux::enc_worker::SrcSize {
                    width: 640,
                    height: 360,
                    stride: 2560,
                },
            )
            .expect("source OpenH264");
        let native = LinuxFrame {
            width: 640,
            height: 360,
            stride: 2560,
            generation: 1,
            pixels: vec![128; 640 * 360 * 4],
        };
        for n in 0..5 {
            let packets = backend
                .encode(&native, n, n == 0 || n == 3)
                .expect("encode");
            assert!(!packets.is_empty());
            assert_eq!(packets[0].keyframe, n == 0 || n == 3);
        }

        let mut scaled = factory
            .open(
                &params(Codec::H264),
                host_linux::enc_worker::SrcSize {
                    width: 1280,
                    height: 720,
                    stride: 5120,
                },
            )
            .expect("source OpenH264 scaled");
        let source = LinuxFrame {
            width: 1280,
            height: 720,
            stride: 5120,
            generation: 1,
            pixels: vec![128; 1280 * 720 * 4],
        };
        assert!(!scaled
            .encode(&source, 0, true)
            .expect("scaled encode")
            .is_empty());
    }

    #[test]
    fn worker_with_openh264() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (sink, mut output) = mpsc::channel(8);
        let factory = Arc::new(OpenH264Factory::with_loader(Arc::new(api)));
        let mut worker =
            EncWorker::start(config(Codec::H264, sink), vec![factory], None).expect("worker");
        for _ in 0..3 {
            worker
                .encode(frame(640, 360), EncodeOpts::default())
                .expect("submit");
        }
        let encoded = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(10), output.recv()).await })
            .expect("encoded timeout")
            .expect("encoded frame");
        assert!(encoded.keyframe);

        let (sink, _output) = mpsc::channel(1);
        let result = EncWorker::start(config(Codec::Hevc, sink), Vec::new(), None);
        assert!(matches!(
            result,
            Err(host_core::platform::PlatformError::Unsupported)
        ));
    }
}
