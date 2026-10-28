//! End-to-end smoke coverage for the fake Host session fixture.
//!
//! The M0 wrapper exercises loopback development mode; Session uses strict mTLS.

use std::{sync::Arc, time::Duration};

use host_core::{HostRuntime, RuntimeOptions};
use host_fake::FakePlatform;
use rsp_client::{ClientOptions, StartParams};
use tokio::time::timeout;
mod common;

#[tokio::test]
async fn fake_host_delivers_at_least_thirty_frames() {
    let platform = FakePlatform::new();
    let (endpoint, _) =
        host_core::listen::bind("127.0.0.1:0".parse().expect("loopback")).expect("bind fake host");
    let addr = endpoint.local_addr().expect("host address");
    let runtime = HostRuntime::new(
        platform.clone(),
        RuntimeOptions {
            bind: Some(addr),
            dev_allow_unpaired: true,
            store_dir: common::state_dir(),
            ..RuntimeOptions::default()
        },
    );
    let task = tokio::spawn(runtime.serve(endpoint.clone()));

    let (client, mut events) = timeout(
        Duration::from_secs(3),
        rsp_client::connect(ClientOptions {
            addr,
            client_name: "session-smoke".to_owned(),
            start: StartParams {
                max_width: 640,
                fps: 30,
                bitrate_kbps: 1_000,
                ..StartParams::default()
            },
            pin_sha256: None,
        }),
    )
    .await
    .expect("client connects")
    .expect("client handshake");

    let frames = timeout(Duration::from_secs(7), async {
        let mut count = 0usize;
        while let Some(event) = events.recv().await {
            if matches!(event, rsp_client::ClientEvent::Frame(_)) {
                count += 1;
                if count >= 30 {
                    break;
                }
            }
        }
        count
    })
    .await
    .expect("frames arrive");
    assert!(frames >= 30, "received {frames} frames");
    client.close();
    timeout(Duration::from_secs(3), async {
        while platform.active_captures() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("capture stops");
    endpoint.close(0u32.into(), b"test complete");
    timeout(Duration::from_secs(3), task)
        .await
        .expect("Host stops")
        .expect("task joins")
        .expect("Host runtime");
    assert_eq!(platform.active_captures(), 0);
}

/// Exercise the new session engine with the real Host identity and paired store.
#[tokio::test]
async fn session_api_with_real_fake_platform_and_user_close() {
    let platform = FakePlatform::new();
    let (endpoint, _) =
        host_core::listen::bind("127.0.0.1:0".parse().expect("loopback")).expect("bind fake host");
    let addr = endpoint.local_addr().expect("host address");
    let host_fp = common::host_fingerprint(&platform);
    let runtime = HostRuntime::new(
        platform.clone(),
        RuntimeOptions {
            bind: Some(addr),
            store_dir: common::state_dir(),
            ..RuntimeOptions::default()
        },
    )
    .with_store(common::paired_store());
    let task = tokio::spawn(runtime.serve(endpoint.clone()));
    let params =
        rsp_client::SessionParams::single_shared(addr, host_fp, Arc::clone(common::device()));
    let (session, mut events) = rsp_client::Session::open(params)
        .await
        .expect("Session open");
    timeout(Duration::from_secs(3), async {
        let mut count = 0;
        while let Some(event) = events.recv().await {
            match event {
                rsp_client::SessionEvent::Frame(frame) => {
                    if count == 0 {
                        assert!(frame.frame.keyframe);
                    }
                    count += 1;
                    if count == 30 {
                        break;
                    }
                }
                rsp_client::SessionEvent::Ended { reason, .. } => panic!("early ending: {reason}"),
                _ => {}
            }
        }
        assert_eq!(count, 30);
    })
    .await
    .expect("30 frames within deadline");
    session.close();
    timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if let rsp_client::SessionEvent::Ended { code, reason, .. } = event {
                assert_eq!(code, None);
                assert_eq!(reason, "USER_CLOSED");
                return;
            }
        }
        panic!("missing normal close");
    })
    .await
    .expect("normal close");
    assert_eq!(platform.active_captures(), 0);
    endpoint.close(0u32.into(), b"test complete");
    timeout(Duration::from_secs(3), task)
        .await
        .expect("Host stops")
        .expect("task joins")
        .expect("Host runtime");
}

/// §4.4: PROBE returns only the minimal hello and USER_CLOSED, without capture.
#[tokio::test]
async fn real_host_probe_returns_minimal_hello_and_goodbye_without_starting_capture() {
    use rsp_core::pb;
    use rsp_transport::identity::{CertKind, LocalIdentity};
    let platform = FakePlatform::new();
    let (endpoint, _) =
        host_core::listen::bind("127.0.0.1:0".parse().expect("loopback")).expect("bind fake host");
    let addr = endpoint.local_addr().expect("host address");
    let host_fp = common::host_fingerprint(&platform);
    let host_id = rsp_core::ids::id_from_fp(&host_fp);
    let runtime = HostRuntime::new(
        platform.clone(),
        RuntimeOptions {
            bind: Some(addr),
            store_dir: common::state_dir(),
            ..RuntimeOptions::default()
        },
    );
    let server = tokio::spawn(runtime.serve(endpoint.clone()));
    let mut params = rsp_client::SessionParams::single(
        addr,
        host_fp,
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    params.intent = pb::Intent::Probe;
    let (_session, mut events) = rsp_client::Session::open(params).await.expect("probe");
    timeout(Duration::from_secs(3), async {
        let mut seen_hello = false;
        while let Some(event) = events.recv().await {
            match event {
                rsp_client::SessionEvent::ServerHello(hello) => {
                    assert_eq!(hello.protocol, rsp_core::features::PROTOCOL_VERSION);
                    assert!(!hello.host_version.is_empty());
                    assert_eq!(hello.host_id, host_id);
                    assert!(hello.peer.is_some());
                    assert!(hello.host_name.is_empty());
                    assert!(hello.displays.is_empty());
                    assert_eq!(hello.features, 0);
                    assert_eq!(hello.required_features, 0);
                    assert_eq!(hello.os, 0);
                    seen_hello = true;
                }
                rsp_client::SessionEvent::Ended { code, kind, .. } => {
                    assert!(seen_hello);
                    assert_eq!(code, None);
                    assert_eq!(
                        kind,
                        rsp_client::EndReason::Goodbye(pb::goodbye::Reason::UserClosed)
                    );
                    return;
                }
                rsp_client::SessionEvent::Frame(_) | rsp_client::SessionEvent::Role(_) => {
                    panic!("probe must not authenticate or stream")
                }
                _ => {}
            }
        }
        panic!("missing probe goodbye");
    })
    .await
    .expect("probe response");
    assert_eq!(platform.active_captures(), 0);
    endpoint.close(0u32.into(), b"test complete");
    timeout(Duration::from_secs(3), server)
        .await
        .expect("Host stops")
        .expect("task joins")
        .expect("Host runtime");
}
