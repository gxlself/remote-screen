use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use host_core::platform::{CaptureConfig, EncodeOpts, EncoderConfig, Platform};
use host_core::{HostRuntime, RuntimeOptions};
use host_fake::{FakePlatform, SyntheticFrame};
use rsp_client::{SessionEvent as ClientEvent, SessionHandle, SessionParams, StartParams};
use rsp_core::input::{InputEvent, MouseButton};
use rsp_core::{clock::unix_micros, pb};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
mod common;

struct RunningHost {
    endpoint: quinn::Endpoint,
    task: JoinHandle<host_core::RuntimeResult<()>>,
    addr: SocketAddr,
    platform: FakePlatform,
}

impl RunningHost {
    fn start(view_only: bool, dump: Option<PathBuf>) -> Self {
        let (endpoint, _) = host_core::listen::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = endpoint.local_addr().unwrap();
        let platform = FakePlatform::new();
        let runtime = HostRuntime::new(
            platform.clone(),
            RuntimeOptions {
                bind: Some(addr),
                view_only,
                dump,
                ..RuntimeOptions::default()
            },
        )
        .with_store(common::paired_store());
        let task = tokio::spawn(runtime.serve(endpoint.clone()));
        Self {
            endpoint,
            task,
            addr,
            platform,
        }
    }

    async fn connect(&self) -> (SessionHandle, mpsc::UnboundedReceiver<ClientEvent>) {
        tokio::time::timeout(
            Duration::from_secs(3),
            rsp_client::Session::open({
                let mut params = SessionParams::single_shared(
                    self.addr,
                    common::host_fingerprint(&self.platform),
                    std::sync::Arc::clone(common::device()),
                );
                params.device_name = "loopback-test".into();
                params.legacy_quic_input = true;
                params.start = StartParams {
                    max_width: 1280,
                    bitrate_kbps: 2_000,
                    ..StartParams::default()
                };
                params
            }),
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn stop(mut self) {
        self.endpoint.close(0u32.into(), b"test complete");
        tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        wait_for(|| self.platform.active_captures() == 0).await;
    }
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test dropped");
        self.task.abort();
    }
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn frame(events: &mut mpsc::UnboundedReceiver<ClientEvent>) -> rsp_client::ReceivedFrame {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await.unwrap() {
                ClientEvent::Frame(frame) => return frame,
                ClientEvent::Ended { reason, .. } => panic!("unexpected close: {reason}"),
                ClientEvent::Error { code, message } => panic!("host error {code:?}: {message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn capture_metadata_and_encoder_cadence_match_the_requested_rate() {
    let platform = FakePlatform::new();
    let (tx, mut frames) = mpsc::channel(4);
    let capture = platform
        .capture(
            1,
            CaptureConfig {
                fps: 60,
                max_width: 1280,
                max_height: 0,
            },
            tx,
        )
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), frames.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(1), frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((first.width, first.height), (1280, 720));
    let a = first.native.downcast_ref::<SyntheticFrame>().unwrap();
    let b = second.native.downcast_ref::<SyntheticFrame>().unwrap();
    assert_eq!(b.frame_number, a.frame_number + 1);
    assert!(b.capture_ts_us > a.capture_ts_us);
    drop(capture);
    assert_eq!(platform.active_captures(), 0);

    let (tx, mut encoded) = mpsc::channel(1);
    let mut encoder = platform
        .encoder(EncoderConfig {
            codec: pb::Codec::Hevc,
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_kbps: 12_000,
            sink: tx,
        })
        .unwrap();
    let mut bytes = 0;
    let start = unix_micros();
    for i in 0..=120 {
        let mut sample = first.clone();
        sample.capture_ts_us = start + i * 1_000_000 / 60;
        sample.native = std::sync::Arc::new(SyntheticFrame {
            frame_number: i,
            capture_ts_us: sample.capture_ts_us,
        });
        encoder.encode(sample, EncodeOpts::default()).unwrap();
        let output = encoded.recv().await.unwrap();
        assert_eq!(output.keyframe, i == 0 || i == 120);
        assert_eq!(&output.data[..4], &[0, 0, 0, 1]);
        assert_eq!(
            u64::from_be_bytes(output.data[6..14].try_into().unwrap()),
            i
        );
        if i < 120 {
            bytes += output.data.len();
        }
    }
    assert!(
        (2_990_000..=3_010_000).contains(&bytes),
        "two-second GOP bytes: {bytes}"
    );
    encoder.force_keyframe();
    encoder.encode(second, EncodeOpts::default()).unwrap();
    assert!(encoded.recv().await.unwrap().keyframe);
}

#[tokio::test]
async fn m0_client_receives_frames_routes_input_requests_keys_and_writes_dump() {
    let dump = std::env::temp_dir().join(format!(
        "rs-h1a-{}-{}.h265",
        std::process::id(),
        unix_micros()
    ));
    let host = RunningHost::start(false, Some(dump.clone()));
    let (client, mut events) = host.connect().await;
    let mut hello = false;
    let mut config = false;
    let mut clock = false;
    let mut keys = 0;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await.unwrap() {
                ClientEvent::ServerHello(server) => {
                    let display = server
                        .displays
                        .iter()
                        .find(|display| display.primary)
                        .unwrap();
                    assert_eq!((display.width_px, display.height_px), (1920, 1080));
                    hello = true;
                }
                ClientEvent::StreamConfig(stream) => {
                    assert_eq!((stream.width, stream.height, stream.fps), (1280, 720, 60));
                    config = true;
                }
                ClientEvent::ClockSync { .. } => clock = true,
                ClientEvent::Frame(frame) if frame.frame.keyframe => {
                    keys += 1;
                    if keys == 1 {
                        client.request_keyframe();
                    }
                }
                ClientEvent::Error { code, message } => panic!("host error {code:?}: {message}"),
                ClientEvent::Ended { reason, .. } => panic!("unexpected close: {reason}"),
                _ => {}
            }
            if hello && config && clock && keys >= 2 {
                break;
            }
        }
    })
    .await
    .unwrap();
    let press = InputEvent::MouseButton {
        button: MouseButton::Left,
        down: true,
        x: 0.25,
        y: 0.75,
        clicks: 1,
    };
    client
        .send_input(rsp_core::hid::InputMsg::MouseButton {
            button: rsp_core::hid::Button::Left,
            down: true,
            x: 0.25,
            y: 0.75,
            clicks: 1,
        })
        .unwrap();
    wait_for(|| host.platform.injected_events().contains(&press)).await;
    client.close();
    wait_for(|| host.platform.active_captures() == 0 && host.platform.release_count() > 0).await;
    assert!(std::fs::metadata(&dump).unwrap().len() > 0);
    host.stop().await;
    std::fs::remove_file(dump).unwrap();
}

#[tokio::test]
async fn view_only_suppresses_input_and_replacement_cleans_up_the_old_session() {
    let host = RunningHost::start(true, None);
    let (first, mut first_events) = host.connect().await;
    frame(&mut first_events).await;
    first
        .send_input(rsp_core::hid::InputMsg::MouseMove { x: 0.1, y: 0.9 })
        .unwrap();
    for _ in 0..8 {
        frame(&mut first_events).await;
    }
    assert!(host.platform.injected_events().is_empty());
    let (second, mut second_events) = host.connect().await;
    frame(&mut second_events).await;
    assert_eq!(host.platform.active_captures(), 1);
    assert!(host.platform.release_count() >= 1);
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = first_events.recv().await {
            if matches!(event, ClientEvent::Ended { .. }) {
                return;
            }
        }
    })
    .await
    .unwrap();
    second.close();
    wait_for(|| host.platform.active_captures() == 0 && host.platform.release_count() >= 2).await;
    host.stop().await;
}
