use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, CaptureConfig, CaptureHandle, CapturedFrame, ConsoleState,
    EncodeOpts, EncodedFrame, Encoder, EncoderConfig, FrameSink, HostInfo, KeyStore, Nic,
    PixelRect, Platform, PlatformError, Result, RoiRegion,
};
use host_win::pipe_proto::{
    AudioPacket, Ctl, PipeFrame, RegionWire, RoiWire, VideoPacket, WireError,
};
use host_win::proxy::{PipePlatform, ProxyTimeouts, ServiceLocal};
use host_win::proxy_media::RemoteFrame;
use rsp_core::pb;
use tokio::sync::mpsc;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Default)]
struct FakeLocal {
    keys: MemoryKeyStore,
}

impl ServiceLocal for FakeLocal {
    fn info(&self) -> HostInfo {
        HostInfo {
            name: "test-host".into(),
            os: pb::Platform::Windows,
            os_version: "11".into(),
            model: "fake".into(),
            arch: "x86_64".into(),
        }
    }

    fn console_state(&self) -> ConsoleState {
        ConsoleState::Locked
    }

    fn nics(&self) -> Vec<Nic> {
        Vec::new()
    }

    fn send_wol(&self, _mac: [u8; 6], _via: &Nic) -> Result<()> {
        Ok(())
    }

    fn keystore(&self) -> &dyn KeyStore {
        &self.keys
    }

    fn ctrl_alt_del(&self) -> Result<()> {
        Ok(())
    }
}

type TestPlatform = PipePlatform<FakeLocal>;

fn platform() -> Arc<TestPlatform> {
    Arc::new(PipePlatform::with_timeouts(
        FakeLocal::default(),
        ProxyTimeouts {
            request: Duration::from_secs(5),
            open: Duration::from_secs(5),
        },
    ))
}

fn short_platform() -> Arc<TestPlatform> {
    Arc::new(PipePlatform::with_timeouts(
        FakeLocal::default(),
        ProxyTimeouts {
            request: Duration::from_millis(50),
            open: Duration::from_millis(50),
        },
    ))
}

struct FakeSession {
    platform: Arc<TestPlatform>,
    generation: u64,
    rx: mpsc::UnboundedReceiver<PipeFrame>,
}

impl FakeSession {
    fn attach(platform: &Arc<TestPlatform>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = platform.attach(tx);
        Self {
            platform: platform.clone(),
            generation,
            rx,
        }
    }

    fn recv(&mut self) -> Ctl {
        let start = Instant::now();
        loop {
            match self.rx.try_recv() {
                Ok(PipeFrame::Ctl(ctl)) => return ctl,
                Ok(_) => panic!("expected a control frame"),
                Err(mpsc::error::TryRecvError::Disconnected) => panic!("session disconnected"),
                Err(mpsc::error::TryRecvError::Empty) => {
                    assert!(start.elapsed() < Duration::from_secs(2), "no control frame");
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn reply(&self, frame: PipeFrame) {
        self.platform.on_frame(self.generation, frame);
    }

    fn done(&self, req: u32, error: Option<WireError>) {
        self.reply(PipeFrame::Ctl(Ctl::Done { req, error }));
    }

    fn with_reply<T>(&mut self, error: Option<WireError>, call: impl FnOnce() -> T) -> (T, Ctl) {
        thread::scope(|scope| {
            let session = scope.spawn(|| {
                let ctl = self.recv();
                let req = open_req(&ctl);
                self.done(req, error);
                ctl
            });
            let result = call();
            (result, session.join().unwrap())
        })
    }

    fn assert_empty(&mut self) {
        assert!(matches!(
            self.rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

fn open_req(ctl: &Ctl) -> u32 {
    match ctl {
        Ctl::CaptureStart { req, .. }
        | Ctl::EncoderOpen { req, .. }
        | Ctl::AudioStart { req, .. } => *req,
        _ => panic!("expected an open request"),
    }
}

fn capture_cfg() -> CaptureConfig {
    CaptureConfig {
        fps: 60,
        max_width: 1920,
        max_height: 1080,
    }
}

fn audio_cfg() -> AudioCaptureConfig {
    AudioCaptureConfig {
        sample_rate: 48_000,
        channels: 2,
        exclude_self: true,
    }
}

fn encoder_cfg(sink: mpsc::Sender<EncodedFrame>) -> EncoderConfig {
    EncoderConfig {
        codec: pb::Codec::Hevc,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 8000,
        sink,
    }
}

fn frame(cap: u32, seq: u64) -> PipeFrame {
    PipeFrame::Ctl(Ctl::Frame {
        cap,
        seq,
        width: 1920,
        height: 1080,
        capture_ts_us: 123_456,
    })
}

fn remote_frame() -> CapturedFrame {
    CapturedFrame {
        display_id: 1,
        width: 1920,
        height: 1080,
        capture_ts_us: 123_456,
        native: Arc::new(RemoteFrame { cap: 1, seq: 9 }),
    }
}

fn video(enc: u32, keyframe: bool) -> PipeFrame {
    PipeFrame::Video(VideoPacket {
        enc,
        keyframe,
        still_refine: !keyframe,
        capture_ts_us: 123_456,
        encoded_ts_us: 123_789,
        data: vec![1, 2],
    })
}

fn audio() -> PipeFrame {
    PipeFrame::Audio(AudioPacket {
        capture_ts_us: 123_456,
        channels: 2,
        frames: 1,
        samples: vec![0.5, -0.5],
    })
}

fn error(message: &str) -> WireError {
    WireError::Platform {
        message: message.into(),
    }
}

fn assert_error<T>(result: Result<T>, expected: &str) {
    assert!(matches!(result, Err(PlatformError::Platform(message)) if message.contains(expected)));
}

fn capture(
    platform: &TestPlatform,
    session: &mut FakeSession,
    sink: FrameSink,
) -> Box<dyn CaptureHandle> {
    let (result, ctl) = session.with_reply(None, || platform.capture(1, capture_cfg(), sink));
    assert!(matches!(
        ctl,
        Ctl::CaptureStart {
            display: 1,
            fps: 60,
            max_width: 1920,
            max_height: 1080,
            ..
        }
    ));
    result.unwrap()
}

#[test]
fn capture_region_sends_region_and_replays() {
    let platform = platform();
    let mut first = FakeSession::attach(&platform);
    let (tx, _rx) = mpsc::channel(1);
    let region = PixelRect {
        x: 736,
        y: 352,
        w: 608,
        h: 352,
    };
    let (result, ctl) = first.with_reply(None, || {
        platform.capture_region(1, region, capture_cfg(), tx)
    });
    assert!(matches!(
        ctl,
        Ctl::CaptureStart {
            region: Some(RegionWire {
                x: 736,
                y: 352,
                w: 608,
                h: 352
            }),
            ..
        }
    ));
    let _capture = result.unwrap();
    platform.detach(first.generation);
    let mut second = FakeSession::attach(&platform);
    assert!(matches!(
        second.recv(),
        Ctl::CaptureStart {
            region: Some(RegionWire {
                x: 736,
                y: 352,
                w: 608,
                h: 352
            }),
            ..
        }
    ));
}

#[test]
fn proxy_encoder_roi() {
    let platform = platform();
    let mut first = FakeSession::attach(&platform);
    let (tx, _rx) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut first, tx);
    first.reply(PipeFrame::Ctl(Ctl::EncoderInfo { enc: 1, roi: false }));
    let regions = [RoiRegion {
        rect: PixelRect {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
        },
        qp_delta: -6,
    }];
    assert!(matches!(
        encoder.set_roi(&regions),
        Err(PlatformError::Unsupported)
    ));
    first.assert_empty();

    first.reply(PipeFrame::Ctl(Ctl::EncoderInfo { enc: 1, roi: true }));
    encoder.set_roi(&regions).unwrap();
    assert_eq!(
        first.recv(),
        Ctl::EncoderRoi {
            enc: 1,
            regions: vec![RoiWire {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
                qp: -6,
            }]
        }
    );

    platform.detach(first.generation);
    let mut second = FakeSession::attach(&platform);
    let open = second.recv();
    let req = open_req(&open);
    assert!(matches!(open, Ctl::EncoderOpen { enc: 1, .. }));
    assert_eq!(second.recv(), Ctl::EncoderKeyframe { enc: 1 });
    second.reply(PipeFrame::Ctl(Ctl::EncoderInfo { enc: 1, roi: true }));
    second.done(req, None);
    assert_eq!(
        second.recv(),
        Ctl::EncoderRoi {
            enc: 1,
            regions: vec![RoiWire {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
                qp: -6,
            }]
        }
    );
}

#[test]
fn pipe_platform_region_flag() {
    let platform = platform();
    assert_eq!(
        platform.capture_region_supported(),
        host_win::viewport_crop::CROP_SUPPORTED
    );
}

fn encoder(
    platform: &TestPlatform,
    session: &mut FakeSession,
    sink: mpsc::Sender<EncodedFrame>,
) -> Box<dyn Encoder> {
    let (result, ctl) = session.with_reply(None, || platform.encoder(encoder_cfg(sink)));
    assert!(
        matches!(ctl, Ctl::EncoderOpen { codec, width: 1920, height: 1080, fps: 60, bitrate_kbps: 8000, .. } if codec == pb::Codec::Hevc as i32)
    );
    result.unwrap()
}

#[test]
fn capture_roundtrip_drop_and_unknown_frames() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let handle = capture(&platform, &mut session, tx);
    session.reply(frame(99, 1));
    assert!(rx.try_recv().is_err());
    session.reply(frame(1, 9));
    session.reply(frame(1, 10)); // Full capture queues discard the newest notification.
    let frame = rx.try_recv().unwrap();
    assert_eq!(
        (
            frame.display_id,
            frame.width,
            frame.height,
            frame.capture_ts_us
        ),
        (1, 1920, 1080, 123_456)
    );
    assert_eq!(
        frame.native.downcast_ref::<RemoteFrame>(),
        Some(&RemoteFrame { cap: 1, seq: 9 })
    );
    drop(handle);
    assert_eq!(session.recv(), Ctl::CaptureStop { cap: 1 });
    session.reply(crate::frame(1, 11));
    assert!(rx.blocking_recv().is_none());
}

#[test]
fn capture_error_timeout_cleanup_and_nonreused_ids() {
    let platform = short_platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let (result, first) = session.with_reply(Some(error("no display")), || {
        platform.capture(1, capture_cfg(), tx)
    });
    assert_error(result, "no display");
    assert!(matches!(first, Ctl::CaptureStart { cap: 1, .. }));
    assert!(rx.blocking_recv().is_none());
    let (tx, mut rx) = mpsc::channel(1);
    assert_error(
        platform.capture(1, capture_cfg(), tx),
        "session process timeout",
    );
    assert!(matches!(session.recv(), Ctl::CaptureStart { cap: 2, .. }));
    assert_eq!(session.recv(), Ctl::CaptureStop { cap: 2 });
    assert!(rx.blocking_recv().is_none());
    let (tx, _rx) = mpsc::channel(1);
    let (result, third) = session.with_reply(None, || platform.capture(1, capture_cfg(), tx));
    assert!(matches!(third, Ctl::CaptureStart { cap: 3, .. }));
    drop(result.unwrap());
    assert_eq!(session.recv(), Ctl::CaptureStop { cap: 3 });
}

#[test]
fn encoder_unsupported_preserves_codec_fallback() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let (result, ctl) = session.with_reply(Some(WireError::Unsupported), || {
        platform.encoder(encoder_cfg(tx))
    });
    assert!(matches!(ctl, Ctl::EncoderOpen { enc: 1, .. }));
    assert!(matches!(result, Err(PlatformError::Unsupported)));
    assert!(rx.blocking_recv().is_none());
    let (tx, _rx) = mpsc::channel(1);
    let handle = encoder(&platform, &mut session, tx);
    drop(handle);
    assert_eq!(session.recv(), Ctl::EncoderClose { enc: 2 });
}

#[test]
fn encode_roundtrip_and_foreign_frame_rejection() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let _capture = capture(&platform, &mut session, tx);
    session.reply(frame(1, 9));
    let frame = rx.try_recv().unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut session, tx);
    encoder
        .encode(
            frame,
            EncodeOpts {
                force_keyframe: true,
                quality_boost: true,
            },
        )
        .unwrap();
    assert_eq!(
        session.recv(),
        Ctl::Encode {
            enc: 1,
            cap: 1,
            seq: 9,
            force_keyframe: true,
            quality_boost: true
        }
    );
    session.reply(video(99, true));
    assert!(rx.try_recv().is_err());
    session.reply(video(1, true));
    let encoded = rx.try_recv().unwrap();
    assert_eq!(encoded.data, [1, 2]);
    assert!(encoded.keyframe);
    assert!(!encoded.still_refine);
    assert_eq!(
        (encoded.capture_ts_us, encoded.encoded_ts_us),
        (123_456, 123_789)
    );
    let mut frame = remote_frame();
    frame.native = Arc::new(());
    assert_error(
        encoder.encode(frame, EncodeOpts::default()),
        "not from session",
    );
    session.assert_empty();
}

#[test]
fn backpressure_requests_one_keyframe_until_recovery() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let _encoder = encoder(&platform, &mut session, tx);
    session.reply(video(1, false));
    session.reply(video(1, false));
    assert_eq!(session.recv(), Ctl::EncoderKeyframe { enc: 1 });
    session.reply(video(1, false));
    session.reply(video(1, true)); // A keyframe also cannot recover while the sink is full.
    session.assert_empty();
    assert!(!rx.try_recv().unwrap().keyframe);
    session.reply(video(1, false));
    assert!(rx.try_recv().is_err());
    session.assert_empty();
    session.reply(video(1, true));
    assert!(rx.try_recv().unwrap().keyframe);
    session.reply(video(1, false));
    let frame = rx.try_recv().unwrap();
    assert!(!frame.keyframe);
    assert!(frame.still_refine);
    session.reply(video(1, false));
    session.reply(video(1, false));
    assert_eq!(session.recv(), Ctl::EncoderKeyframe { enc: 1 });
    session.assert_empty();
}

#[test]
fn encoder_failure_rate_boost_keyframe_and_drop() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut session, tx);
    encoder.set_rate(4000, 30);
    assert_eq!(
        session.recv(),
        Ctl::EncoderRate {
            enc: 1,
            kbps: 4000,
            fps: 30
        }
    );
    encoder.set_quality_boost(true);
    assert_eq!(session.recv(), Ctl::EncoderBoost { enc: 1, on: true });
    encoder.force_keyframe();
    assert_eq!(session.recv(), Ctl::EncoderKeyframe { enc: 1 });
    session.reply(PipeFrame::Ctl(Ctl::EncoderFailed {
        enc: 1,
        error: error("gpu lost"),
    }));
    assert_error(
        encoder.encode(remote_frame(), EncodeOpts::default()),
        "gpu lost",
    );
    session.assert_empty();
    drop(encoder);
    assert_eq!(session.recv(), Ctl::EncoderClose { enc: 1 });
    session.reply(video(1, true));
    assert!(rx.blocking_recv().is_none());
}

#[test]
fn audio_roundtrip_exclusivity_backpressure_and_drop() {
    let platform = platform();
    assert!(platform.audio_capture_supported());
    let mut session = FakeSession::attach(&platform);
    session.reply(audio()); // Unregistered audio is ignored.
    let (tx, mut rx) = mpsc::channel(1);
    let (result, ctl) = session.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    assert!(matches!(
        ctl,
        Ctl::AudioStart {
            sample_rate: 48_000,
            channels: 2,
            exclude_self: true,
            ..
        }
    ));
    let handle = result.unwrap();
    let (tx, _rx) = mpsc::channel(1);
    assert_error(platform.capture_audio(audio_cfg(), tx), "already active");
    session.assert_empty();
    session.reply(audio());
    session.reply(audio());
    assert_eq!(
        rx.try_recv().unwrap(),
        AudioChunk {
            capture_ts_us: 123_456,
            channels: 2,
            frames: 1,
            samples: vec![0.5, -0.5]
        }
    );
    assert!(rx.try_recv().is_err());
    drop(handle);
    assert_eq!(session.recv(), Ctl::AudioStop);
    session.reply(audio());
    assert!(rx.blocking_recv().is_none());
    let (tx, _rx) = mpsc::channel(1);
    let (result, _) = session.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    drop(result.unwrap());
    assert_eq!(session.recv(), Ctl::AudioStop);
}

#[test]
fn reconnect_replays_latest_config_and_capture_error_closes_sink() {
    let platform = platform();
    let mut old = FakeSession::attach(&platform);
    let (tx, mut frames) = mpsc::channel(1);
    let _capture = capture(&platform, &mut old, tx);
    let (tx, mut videos) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut old, tx);
    encoder.set_rate(3000, 20);
    assert_eq!(
        old.recv(),
        Ctl::EncoderRate {
            enc: 1,
            kbps: 3000,
            fps: 20
        }
    );
    let (tx, mut audio_rx) = mpsc::channel(1);
    let (result, _) = old.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    let _audio = result.unwrap();
    platform.detach(old.generation);
    assert_error(
        encoder.encode(remote_frame(), EncodeOpts::default()),
        "not connected",
    );
    assert!(!frames.is_closed());
    assert!(!videos.is_closed());
    assert!(!audio_rx.is_closed());
    let mut session = FakeSession::attach(&platform);
    let mut replay_cap = None;
    let mut reqs = Vec::new();
    for _ in 0..4 {
        match session.recv() {
            Ctl::CaptureStart {
                req,
                cap: 1,
                display: 1,
                fps: 60,
                max_width: 1920,
                max_height: 1080,
                region: _,
            } => {
                replay_cap = Some(req);
                reqs.push(req);
            }
            Ctl::EncoderOpen {
                req,
                enc: 1,
                codec,
                width: 1920,
                height: 1080,
                bitrate_kbps: 3000,
                fps: 20,
            } => {
                assert_eq!(codec, pb::Codec::Hevc as i32);
                reqs.push(req);
                session.done(req, None);
            }
            Ctl::EncoderKeyframe { enc: 1 } => {}
            Ctl::AudioStart {
                req,
                sample_rate: 48_000,
                channels: 2,
                exclude_self: true,
            } => {
                reqs.push(req);
                session.done(req, None);
            }
            ctl => panic!("unexpected replay: {ctl:?}"),
        }
    }
    reqs.sort_unstable();
    reqs.dedup();
    assert_eq!(reqs.len(), 3);
    platform.detach(old.generation); // Stale connections cannot tear down the replacement.
    old.reply(frame(1, 99));
    old.reply(video(1, true));
    old.reply(audio());
    assert!(frames.try_recv().is_err());
    assert!(videos.try_recv().is_err());
    assert!(audio_rx.try_recv().is_err());
    session.reply(video(1, false));
    assert!(videos.try_recv().is_err());
    session.reply(video(1, true));
    assert!(videos.try_recv().unwrap().keyframe);
    session.reply(audio());
    assert!(audio_rx.try_recv().is_ok());
    session.done(replay_cap.unwrap(), Some(error("display removed")));
    assert!(frames.blocking_recv().is_none());
    session.reply(frame(1, 100));
    assert!(frames.blocking_recv().is_none());
    session.assert_empty();
}

#[test]
fn capture_ended_closes_sink_and_does_not_replay() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let handle = capture(&platform, &mut session, tx);
    session.reply(PipeFrame::Ctl(Ctl::CaptureEnded {
        cap: 1,
        error: None,
    }));
    assert!(rx.blocking_recv().is_none());
    platform.detach(session.generation);
    let mut next = FakeSession::attach(&platform);
    next.assert_empty();
    drop(handle);
}

#[test]
fn encoder_and_audio_timeout_cleanup() {
    let platform = short_platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    assert_error(platform.encoder(encoder_cfg(tx)), "timeout");
    let req = open_req(&session.recv());
    assert_eq!(session.recv(), Ctl::EncoderClose { enc: 1 });
    session.done(req, None); // A late success cannot resurrect the timed-out registration.
    assert!(rx.blocking_recv().is_none());
    let (tx, mut rx) = mpsc::channel(1);
    assert_error(platform.capture_audio(audio_cfg(), tx), "timeout");
    let req = open_req(&session.recv());
    assert_eq!(session.recv(), Ctl::AudioStop);
    session.done(req, None);
    assert!(rx.blocking_recv().is_none());
}

// Replies here must beat the open timeout, so they run on the normal 5 s platform:
// with the 50 ms one a loaded machine turns the reply into a timeout.
#[test]
fn audio_open_reports_permission_denied_then_succeeds() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let (result, _) = session.with_reply(
        Some(WireError::PermissionDenied {
            message: "denied".into(),
        }),
        || platform.capture_audio(audio_cfg(), tx),
    );
    assert!(matches!(result, Err(PlatformError::PermissionDenied(message)) if message == "denied"));
    assert!(rx.blocking_recv().is_none());
    let (tx, _rx) = mpsc::channel(1);
    let (result, _) = session.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    assert!(result.is_ok());
}

#[test]
fn disconnected_opens_and_dropped_platform_handles() {
    let platform = platform();
    let (tx, _rx) = mpsc::channel(1);
    assert_error(
        platform.capture(1, capture_cfg(), tx),
        "session process not connected",
    );
    let (tx, _rx) = mpsc::channel(1);
    assert_error(
        platform.encoder(encoder_cfg(tx)),
        "session process not connected",
    );
    let (tx, _rx) = mpsc::channel(1);
    assert_error(
        platform.capture_audio(audio_cfg(), tx),
        "session process not connected",
    );
    let mut session = FakeSession::attach(&platform);
    let (tx, mut frames) = mpsc::channel(1);
    let capture = capture(&platform, &mut session, tx);
    let (tx, mut videos) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut session, tx);
    drop(session);
    drop(platform);
    assert!(frames.blocking_recv().is_none());
    assert!(videos.blocking_recv().is_none());
    assert_error(
        encoder.encode(remote_frame(), EncodeOpts::default()),
        "not connected",
    );
    encoder.set_rate(4000, 30);
    encoder.set_quality_boost(false);
    encoder.force_keyframe();
    drop(capture);
    drop(encoder);
}

#[test]
fn replay_failures_recover_encoder_and_do_not_let_old_audio_stop_replacement() {
    let platform = platform();
    let mut first = FakeSession::attach(&platform);
    let (tx, mut videos) = mpsc::channel(1);
    let mut encoder = encoder(&platform, &mut first, tx);
    first.reply(PipeFrame::Ctl(Ctl::EncoderFailed {
        enc: 1,
        error: error("gpu lost"),
    }));
    let (tx, mut old_audio_rx) = mpsc::channel(1);
    let (result, _) = first.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    let old_audio = result.unwrap();
    platform.detach(first.generation);
    let (tx, _rx) = mpsc::channel(1);
    assert_error(platform.capture_audio(audio_cfg(), tx), "already active");
    encoder.set_rate(4500, 25); // Rate changes made offline must be replayed too.
    let mut second = FakeSession::attach(&platform);
    for _ in 0..3 {
        match second.recv() {
            Ctl::EncoderOpen {
                req,
                bitrate_kbps: 4500,
                fps: 25,
                ..
            } => second.done(req, Some(error("reopen failed"))),
            Ctl::EncoderKeyframe { enc: 1 } => {}
            Ctl::AudioStart { req, .. } => second.done(req, Some(error("audio unavailable"))),
            ctl => panic!("unexpected replay: {ctl:?}"),
        }
    }
    assert_error(
        encoder.encode(remote_frame(), EncodeOpts::default()),
        "reopen failed",
    );
    assert!(old_audio_rx.blocking_recv().is_none());
    let (tx, mut new_audio_rx) = mpsc::channel(1);
    let (result, _) = second.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    let new_audio = result.unwrap();
    drop(old_audio);
    second.assert_empty();
    second.reply(audio());
    assert!(new_audio_rx.try_recv().is_ok());
    drop(new_audio);
    assert_eq!(second.recv(), Ctl::AudioStop);

    // attach alone also replaces a session and resets failed encoders.
    let mut third = FakeSession::attach(&platform);
    let ctl = third.recv();
    assert!(matches!(
        ctl,
        Ctl::EncoderOpen {
            enc: 1,
            bitrate_kbps: 4500,
            fps: 25,
            ..
        }
    ));
    third.done(open_req(&ctl), None);
    assert_eq!(third.recv(), Ctl::EncoderKeyframe { enc: 1 });
    encoder
        .encode(remote_frame(), EncodeOpts::default())
        .unwrap();
    assert!(matches!(third.recv(), Ctl::Encode { enc: 1, .. }));
    third.reply(video(1, false));
    assert!(videos.try_recv().is_err());
    third.reply(video(1, true));
    assert!(videos.try_recv().unwrap().keyframe);
}

#[test]
fn detach_wakes_media_open_and_dropped_handles_are_not_replayed() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let result = thread::scope(|scope| {
        let waiter = scope.spawn(|| platform.capture(1, capture_cfg(), tx));
        assert!(matches!(session.recv(), Ctl::CaptureStart { cap: 1, .. }));
        platform.detach(session.generation);
        waiter.join().unwrap()
    });
    assert_error(result, "disconnected");
    assert!(rx.blocking_recv().is_none());
    let mut session = FakeSession::attach(&platform);
    session.assert_empty();
    let (tx, _rx) = mpsc::channel(1);
    let capture = capture(&platform, &mut session, tx);
    let (tx, _rx) = mpsc::channel(1);
    let encoder = encoder(&platform, &mut session, tx);
    platform.detach(session.generation);
    drop(capture);
    drop(encoder);
    let mut session = FakeSession::attach(&platform);
    session.assert_empty();
}

#[tokio::test(flavor = "current_thread")]
async fn media_open_works_on_a_tokio_thread() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx, _rx) = mpsc::channel(1);
    let capture = capture(&platform, &mut session, tx);
    let (tx, _rx) = mpsc::channel(1);
    let encoder = encoder(&platform, &mut session, tx);
    let (tx, _rx) = mpsc::channel(1);
    let (result, _) = session.with_reply(None, || platform.capture_audio(audio_cfg(), tx));
    drop(result.unwrap());
    drop(encoder);
    drop(capture);
}
