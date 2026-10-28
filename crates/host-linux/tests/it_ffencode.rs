#![cfg(feature = "desktop-it")]
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use host_core::encode_model::{au_is_key, nal_type};
use host_core::platform::{
    CapturedFrame, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, PlatformError,
};
use host_linux::enc_worker::{BackendError, BackendFactory, EncWorker, SrcSize};
use host_linux::ff_encoder::FfFactory;
use host_linux::ff_model::{Candidate, EncParams, LinuxBackend};
use host_linux::ffmpeg::FfLib;
use host_linux::frame::{FramePool, BYTES_PER_PIXEL};
use rsp_core::{clock::unix_micros, nal::split_annexb, pb::Codec};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, Receiver};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

struct Environment {
    lib: Arc<FfLib>,
    backend: Option<LinuxBackend>,
}

impl Environment {
    fn load() -> Self {
        let dir = std::env::var_os("RS_FFMPEG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .expect("set RS_FFMPEG_DIR to the FFmpeg 8 LGPL library directory");
        let gpu = std::env::var("RS_IT_GPU").expect("set RS_IT_GPU to vaapi, nvenc, or none");
        let backend = match gpu.as_str() {
            "vaapi" => Some(LinuxBackend::Vaapi),
            "nvenc" => Some(LinuxBackend::Nvenc),
            "none" => None,
            _ => panic!("RS_IT_GPU must be vaapi, nvenc, or none; got {gpu:?}"),
        };
        let lib = FfLib::load(&dir).unwrap_or_else(|error| {
            panic!("FFmpeg libraries in {} must load: {error}", dir.display())
        });
        Self { lib, backend }
    }

    fn factory(&self, backend: LinuxBackend) -> FfFactory {
        let candidate = Candidate {
            backend,
            render_node: (backend == LinuxBackend::Vaapi)
                .then(|| PathBuf::from("/dev/dri/renderD128")),
        };
        let factory = FfFactory::new(Arc::clone(&self.lib), &candidate);
        assert_eq!(factory.name(), backend.name());
        factory
    }

    fn start(&self, codec: Codec) -> Option<(EncWorker, Receiver<EncodedFrame>)> {
        let backends = match self.backend {
            Some(backend) => vec![backend],
            None => vec![LinuxBackend::Vaapi, LinuxBackend::Nvenc],
        };
        // In none mode each hardware factory must fail independently; a later
        // candidate cannot hide an unexpectedly successful probe.
        for backend in backends {
            let (sink, output) = mpsc::channel(4);
            let params = params(codec);
            let cfg = EncoderConfig {
                codec: params.codec,
                width: params.width,
                height: params.height,
                fps: params.fps,
                bitrate_kbps: 8000,
                sink,
            };
            let result = EncWorker::start(
                cfg,
                vec![Arc::new(self.factory(backend))],
                Some(Handle::current()),
            );
            if self.backend.is_some() {
                let worker = result.unwrap_or_else(|error| {
                    panic!("selected {backend:?} worker must start: {error}")
                });
                assert_eq!(worker.status().backend, Some(backend.name()));
                return Some((worker, output));
            }
            assert!(
                matches!(result, Err(PlatformError::Unsupported)),
                "RS_IT_GPU=none: {backend:?} {codec:?} worker must return Unsupported"
            );
        }
        None
    }
}

fn params(codec: Codec) -> EncParams {
    EncParams {
        codec,
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_bps: 8_000_000,
    }
}

struct Frames {
    pool: FramePool,
    width: u32,
    height: u32,
    generation: u64,
    number: u8,
    start_us: u64,
}

impl Frames {
    fn new() -> Self {
        Self {
            pool: FramePool::new(3),
            width: 1920,
            height: 1080,
            generation: 1,
            number: 0,
            start_us: unix_micros(),
        }
    }

    fn next(&mut self) -> CapturedFrame {
        let gray = self.number;
        let native = self
            .pool
            .acquire(
                self.width,
                self.height,
                self.width * BYTES_PER_PIXEL,
                self.generation,
                |pixels| pixels.fill(gray),
            )
            .expect("sequential encoding must release frame pool slots");
        let frame = CapturedFrame {
            display_id: 1,
            width: self.width,
            height: self.height,
            capture_ts_us: self.start_us + u64::from(self.number) * 1_000_000 / 60,
            native,
        };
        self.number += 1;
        frame
    }

    fn resize(&mut self) {
        self.width = 1280;
        self.height = 720;
        self.generation += 1;
    }
}

async fn round_trip(
    worker: &mut EncWorker,
    output: &mut Receiver<EncodedFrame>,
    frame: CapturedFrame,
    opts: EncodeOpts,
) -> EncodedFrame {
    let capture_ts_us = frame.capture_ts_us;
    worker.encode(frame, opts).expect("submit capture frame");
    // Receiving each low-latency output before submitting the next input keeps
    // the worker's latest-frame mailbox from overwriting frames under test.
    // The timeout is only a deadlock guard, not a latency/performance assertion.
    let encoded = timeout(WAIT, output.recv())
        .await
        .unwrap_or_else(|error| panic!("encoder output timed out: {error}; {:?}", worker.status()))
        .unwrap_or_else(|| panic!("encoder output closed: {:?}", worker.status()));
    assert_eq!(encoded.capture_ts_us, capture_ts_us);
    assert!(!encoded.data.is_empty());
    assert!(!encoded.still_refine);
    encoded
}

#[tokio::test]
async fn worker_end_to_end() {
    let env = Environment::load();
    let Some((mut worker, mut output)) = env.start(Codec::Hevc) else {
        return;
    };
    let mut frames = Frames::new();
    let mut nonkey_seen = false;
    for index in 0..30 {
        let encoded = round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts::default(),
        )
        .await;
        if index == 0 {
            assert!(encoded.keyframe);
            assert!(au_is_key(Codec::Hevc, &encoded.data));
            let prefix: Vec<_> = split_annexb(&encoded.data)
                .into_iter()
                .take(3)
                .map(|nal| nal_type(Codec::Hevc, nal))
                .collect();
            assert_eq!(prefix, [Some(32), Some(33), Some(34)], "VPS/SPS/PPS prefix");
        } else {
            nonkey_seen |= !encoded.keyframe;
        }
    }
    assert!(
        nonkey_seen,
        "the stream must contain inter frames after the IDR"
    );
    assert_eq!(worker.status().stats.mailbox_dropped, 0);
    assert_eq!(worker.status().stats.sink_dropped, 0);
}

#[tokio::test]
async fn h264_and_force_key() {
    let env = Environment::load();
    let Some((mut worker, mut output)) = env.start(Codec::H264) else {
        return;
    };
    let mut frames = Frames::new();
    for index in 0..10 {
        let encoded = round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts {
                force_keyframe: index == 5,
                ..EncodeOpts::default()
            },
        )
        .await;
        if index == 0 || index == 5 {
            assert!(encoded.keyframe, "H.264 frame {} must be key", index + 1);
            assert!(au_is_key(Codec::H264, &encoded.data));
        }
    }
}

#[tokio::test]
async fn rate_change() {
    let env = Environment::load();
    if env.backend.is_none() {
        for backend in [LinuxBackend::Vaapi, LinuxBackend::Nvenc] {
            let result = env.factory(backend).open(
                &params(Codec::Hevc),
                SrcSize {
                    width: 1920,
                    height: 1080,
                    stride: 1920 * BYTES_PER_PIXEL,
                },
            );
            assert!(
                matches!(result, Err(BackendError::Unsupported(_))),
                "RS_IT_GPU=none: {backend:?} factory must return Unsupported"
            );
        }
        return;
    }
    let (mut worker, mut output) = env.start(Codec::Hevc).expect("GPU mode");
    let mut frames = Frames::new();
    for _ in 0..5 {
        round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts::default(),
        )
        .await;
    }
    let before = worker.status().stats;
    // Fill pixels first so the next capture is ready as soon as rate is queued.
    let frame = frames.next();
    worker.set_rate(3000, 60);
    let encoded = round_trip(&mut worker, &mut output, frame, EncodeOpts::default()).await;
    assert!(
        encoded.keyframe,
        "first frame after rate change must be key"
    );
    assert!(au_is_key(Codec::Hevc, &encoded.data));
    let after = worker.status().stats;
    assert!(after.rate_applied > before.rate_applied);
    if env.backend == Some(LinuxBackend::Vaapi) {
        assert!(
            after.reopens > before.reopens,
            "VA-API rate change must reopen"
        );
    } else {
        assert_eq!(after.reopens, before.reopens, "NVENC changes rate in place");
    }
    for _ in 0..5 {
        round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts::default(),
        )
        .await;
    }
}

#[tokio::test]
async fn source_resize() {
    let env = Environment::load();
    if env.backend.is_none() {
        let factory = env.factory(LinuxBackend::OpenH264);
        for codec in [Codec::H264, Codec::Hevc] {
            assert!(matches!(
                factory.open(
                    &params(codec),
                    SrcSize {
                        width: 1920,
                        height: 1080,
                        stride: 1920 * BYTES_PER_PIXEL,
                    },
                ),
                Err(BackendError::Unsupported(_))
            ));
        }
        return;
    }
    let (mut worker, mut output) = env.start(Codec::Hevc).expect("GPU mode");
    let mut frames = Frames::new();
    for _ in 0..5 {
        round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts::default(),
        )
        .await;
    }
    let before = worker.status().stats.reopens;
    frames.resize();
    let encoded = round_trip(
        &mut worker,
        &mut output,
        frames.next(),
        EncodeOpts::default(),
    )
    .await;
    assert!(
        encoded.keyframe,
        "first frame after source resize must be key"
    );
    assert!(au_is_key(Codec::Hevc, &encoded.data));
    assert!(worker.status().stats.reopens > before);
    for _ in 0..5 {
        round_trip(
            &mut worker,
            &mut output,
            frames.next(),
            EncodeOpts::default(),
        )
        .await;
    }
}
