use std::sync::Arc;
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, CapturedFrame, DisplayInfo, EncodeOpts, EncodedFrame, Encoder, EncoderConfig,
    Platform, PlatformError,
};
use host_fake::clip::{Clip, ClipEncoder, ClipError, ClipPlatform};
use host_fake::harness::{app_identity, direct_params, open, TestHost, TestHostOptions};
use host_fake::SyntheticFrame;
use rsp_client::session::SessionEvent;
use rsp_core::nal::{hevc_nal_type, hevc_to_avcc, split_annexb};
use rsp_core::pb;
use rsp_core::trust::Permission;
use tokio::sync::mpsc;

fn encoder_config(codec: pb::Codec) -> (EncoderConfig, mpsc::Receiver<EncodedFrame>) {
    let (sink, output) = mpsc::channel(256);
    (
        EncoderConfig {
            codec,
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 400,
            sink,
        },
        output,
    )
}

fn captured_frame(capture_ts_us: u64) -> CapturedFrame {
    CapturedFrame {
        display_id: 1,
        width: 640,
        height: 360,
        capture_ts_us,
        native: Arc::new(SyntheticFrame {
            frame_number: capture_ts_us,
            capture_ts_us,
        }),
    }
}

fn contains_irap(data: &[u8]) -> bool {
    split_annexb(data)
        .into_iter()
        .any(|nal| matches!(hevc_nal_type(nal), Some(kind) if (16..=21).contains(&kind)))
}

#[test]
fn bundled_clip_has_expected_access_units() {
    let clip = Clip::bundled();
    assert_eq!(clip.len(), 120);
    assert_eq!(clip.keyframe_indices(), &[0, 30, 60, 90]);

    for &index in clip.keyframe_indices() {
        let au = hevc_to_avcc(clip.au(index));
        assert!(
            au.has_parameter_sets(),
            "keyframe AU {index} lacks VPS/SPS/PPS"
        );
        assert!(contains_irap(clip.au(index)), "AU {index} lacks an IRAP");
    }
    for index in 0..clip.len() {
        if !clip.is_keyframe(index) {
            assert!(
                !contains_irap(clip.au(index)),
                "non-keyframe AU {index} contains an IRAP"
            );
        }
    }
}

#[test]
fn parse_requires_an_access_unit_delimiter() {
    assert!(matches!(
        Clip::parse(b"\x00\x00\x00\x01\x40\x01"),
        Err(ClipError::NoAud)
    ));
}

#[test]
fn parse_keeps_pre_aud_bytes_with_the_first_access_unit() {
    let mut stream = Vec::new();
    for nal in [
        &[0x40, 0x01][..],
        &[0x46, 0x01, 0x50][..],
        &[0x26, 0x01, 0x01][..],
        &[0x46, 0x01, 0x50][..],
        &[0x02, 0x01, 0x02][..],
    ] {
        stream.extend_from_slice(&[0, 0, 0, 1]);
        stream.extend_from_slice(nal);
    }
    let clip = Clip::parse(&stream).expect("valid clip");
    assert_eq!(clip.len(), 2);
    assert_eq!(clip.keyframe_indices(), &[0]);
    assert_eq!(
        split_annexb(clip.au(0))
            .first()
            .and_then(|nal| hevc_nal_type(nal)),
        Some(32)
    );
    assert_eq!(
        split_annexb(clip.au(1))
            .first()
            .and_then(|nal| hevc_nal_type(nal)),
        Some(35)
    );
}

#[test]
fn parse_rejects_a_clip_without_an_irap() {
    let stream = [0, 0, 0, 1, 0x46, 0x01, 0x50, 0, 0, 0, 1, 0x02, 0x01, 0x01];
    assert!(matches!(Clip::parse(&stream), Err(ClipError::NoKeyframe)));
}

#[test]
fn keyframe_lookup_wraps_across_the_clip() {
    let clip = Clip::bundled();
    assert_eq!(clip.next_keyframe_at_or_after(45), 60);
    assert_eq!(clip.next_keyframe_at_or_after(90), 90);
    assert_eq!(clip.next_keyframe_at_or_after(100), 0);
}

#[test]
fn encoder_replays_access_units_in_order() {
    let clip = Clip::bundled();
    let (cfg, mut output) = encoder_config(pb::Codec::Hevc);
    let mut encoder = ClipEncoder::new(clip, cfg).expect("HEVC encoder");
    for call in 0..125usize {
        let capture_ts_us = 1_000 + call as u64;
        encoder
            .encode(captured_frame(capture_ts_us), EncodeOpts::default())
            .expect("encode");
        let encoded = output.try_recv().expect("one output per encode");
        let expected = call % clip.len();
        assert_eq!(encoded.data, clip.au(expected), "call {call}");
        assert_eq!(encoded.keyframe, clip.is_keyframe(expected), "call {call}");
        assert_eq!(encoded.capture_ts_us, capture_ts_us);
    }
}

#[test]
fn forced_keyframes_jump_to_the_next_irap() {
    let clip = Clip::bundled();
    let (cfg, mut output) = encoder_config(pb::Codec::Hevc);
    let mut encoder = ClipEncoder::new(clip, cfg).expect("HEVC encoder");
    for call in 0..45u64 {
        encoder
            .encode(captured_frame(1_000 + call), EncodeOpts::default())
            .expect("encode");
        output.try_recv().expect("advance output");
    }
    encoder.force_keyframe();
    encoder
        .encode(captured_frame(2_000), EncodeOpts::default())
        .expect("encode forced keyframe");
    let forced = output.try_recv().expect("forced output");
    assert!(forced.keyframe);
    assert_eq!(forced.data, clip.au(60));

    for call in 0..60u64 {
        encoder
            .encode(captured_frame(3_000 + call), EncodeOpts::default())
            .expect("encode");
        output.try_recv().expect("advance output");
    }
    encoder
        .encode(
            captured_frame(4_000),
            EncodeOpts {
                force_keyframe: true,
                quality_boost: false,
            },
        )
        .expect("encode option-forced keyframe");
    let forced = output.try_recv().expect("forced output");
    assert!(forced.keyframe);
    assert_eq!(forced.data, clip.au(30));
}

#[test]
fn a_full_sink_forces_a_keyframe_on_the_next_output() {
    let clip = Clip::bundled();
    let (sink, mut output) = mpsc::channel(1);
    let cfg = EncoderConfig {
        codec: pb::Codec::Hevc,
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 400,
        sink,
    };
    let mut encoder = ClipEncoder::new(clip, cfg).expect("HEVC encoder");
    encoder
        .encode(captured_frame(1), EncodeOpts::default())
        .expect("first encode");
    encoder
        .encode(captured_frame(2), EncodeOpts::default())
        .expect("full sink is recoverable");
    let first = output.try_recv().expect("drain the first output");
    assert_eq!(first.data, clip.au(0));
    encoder
        .encode(captured_frame(3), EncodeOpts::default())
        .expect("next encode");
    let forced = output.try_recv().expect("next output");
    assert!(forced.keyframe);
    assert_eq!(forced.data, clip.au(30));
}

#[test]
fn rate_and_quality_settings_are_recorded() {
    let (cfg, _output) = encoder_config(pb::Codec::Hevc);
    let mut encoder = ClipEncoder::new(Clip::bundled(), cfg).expect("HEVC encoder");
    encoder.set_rate(900, 24);
    encoder.set_quality_boost(true);
    assert_eq!(encoder.rate(), (900, 24));
    assert!(encoder.quality_boost());
}

#[test]
fn platform_reports_the_clip_display() {
    let platform = ClipPlatform::new();
    let expected = DisplayInfo {
        id: 1,
        name: "Clip display".into(),
        width_px: 640,
        height_px: 360,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 30,
        primary: true,
        rotation: 0,
    };
    assert_eq!(platform.displays(), vec![expected]);
}

#[test]
fn platform_rejects_unsupported_codecs() {
    let platform = ClipPlatform::new();
    let (cfg, _output) = encoder_config(pb::Codec::H264);
    assert!(matches!(
        platform.encoder(cfg),
        Err(PlatformError::Unsupported)
    ));
}

#[test]
fn platform_capture_targets_the_clip_display() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let platform = ClipPlatform::new();
        let (sink, mut frames) = mpsc::channel(2);
        let _capture = platform
            .capture(
                1,
                CaptureConfig {
                    fps: 60,
                    max_width: 0,
                    max_height: 0,
                },
                sink,
            )
            .expect("start capture");
        let frame = tokio::time::timeout(Duration::from_secs(1), frames.recv())
            .await
            .expect("capture emits a frame")
            .expect("capture sink stays open");
        assert_eq!((frame.display_id, frame.width, frame.height), (1, 640, 360));
        assert!(frame.native.downcast_ref::<SyntheticFrame>().is_some());
        let (sink, _frames) = mpsc::channel(1);
        assert!(platform
            .capture(
                2,
                CaptureConfig {
                    fps: 30,
                    max_width: 0,
                    max_height: 0
                },
                sink
            )
            .is_err());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clip_platform_streams_decodable_frames_end_to_end() {
    tokio::time::timeout(Duration::from_secs(9), async {
        let host =
            TestHost::start_with_platform(ClipPlatform::new(), TestHostOptions::default()).await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let (session, mut events) = open(direct_params(&host, app))
            .await
            .expect("open paired clip session");

        let (frames, keyframes, stream) = tokio::time::timeout(Duration::from_secs(3), async {
            let mut frames = 0u64;
            let mut keyframes = 0u64;
            let mut stream = None;
            while frames < 60 {
                match events.recv().await {
                    Some(SessionEvent::Frame(received)) => {
                        if received.frame.keyframe {
                            keyframes += 1;
                            let au = hevc_to_avcc(&received.frame.data);
                            assert!(
                                au.has_parameter_sets(),
                                "streamed keyframe lacks VPS/SPS/PPS"
                            );
                        }
                        frames += 1;
                    }
                    Some(SessionEvent::StreamConfig(config)) => stream = Some(config),
                    Some(SessionEvent::Error { code, message }) => {
                        panic!("Host error {code:?}: {message}")
                    }
                    Some(SessionEvent::Ended { kind, .. }) => panic!("session ended: {kind:?}"),
                    Some(_) => {}
                    None => panic!("client event stream closed"),
                }
            }
            (frames, keyframes, stream)
        })
        .await
        .expect("receive 60 frames within three seconds");

        assert!(frames >= 60, "received {frames} frames");
        assert!(keyframes >= 1, "received no keyframes");
        let stream = stream.expect("received StreamConfig");
        assert_eq!((stream.width, stream.height), (640, 360));
        assert_eq!(stream.codec, pb::Codec::Hevc as i32);

        session.close();
        host.stop().await;
    })
    .await
    .expect("clip end-to-end test completes within nine seconds");
}
