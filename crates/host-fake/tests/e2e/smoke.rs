use std::time::{Duration, Instant};

use host_fake::harness::*;
use rsp_core::{errors::ErrorCode, pb, trust::Permission};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it_01_paired_device_first_frame_and_keyframe() {
    tokio::time::timeout(Duration::from_secs(9), async {
        let host = TestHost::start().await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        assert!(host.tcp_addr.is_some(), "TCP is enabled by default");

        let params = direct_params(&host, app);
        let started = Instant::now();
        let (session, mut events) = open(params).await.expect("open paired session");
        let first = wait_frames(&mut events, 1, Duration::from_secs(2)).await;
        let first_frame = started.elapsed();
        assert_eq!(first.frames, 1, "{first:?}");
        let limit = if cfg!(debug_assertions) {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(300)
        };
        println!(
            "IT-01 {} first frame: {first_frame:?} (limit {limit:?})",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        assert!(first_frame <= limit, "first frame took {first_frame:?}");
        let report = wait_frames(&mut events, 9, Duration::from_secs(2)).await;
        assert_eq!(first.frames + report.frames, 10, "{report:?}");
        assert!(first.keyframes + report.keyframes >= 1, "{report:?}");
        assert!(first.errors.is_empty(), "{first:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(report.ended.is_none(), "{report:?}");
        assert!(first.stream.is_some(), "{first:?}");
        session.close();
        let state_dir = host.state_dir.clone();
        host.stop().await;
        assert!(
            !state_dir.exists(),
            "stopped Host removes its state directory"
        );
    })
    .await
    .expect("IT-01 completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpaired_certificate_ends_with_181() {
    tokio::time::timeout(Duration::from_secs(9), async {
        let host = TestHost::start().await;
        let (session, mut events) = open(direct_params(&host, app_identity()))
            .await
            .expect("open unpaired session");
        let (code, kind, _) = wait_end(&mut events, Duration::from_secs(3))
            .await
            .expect("unpaired session ends");
        assert_eq!(code, Some(ErrorCode::NotPaired));
        assert_eq!(code.map(|code| code as u16), Some(181));
        assert_eq!(
            kind,
            rsp_client::session::EndReason::Error(ErrorCode::NotPaired)
        );
        session.close();
        host.stop().await;
    })
    .await
    .expect("unpaired fixture check completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn correct_pairing_code_then_same_certificate_receives_frames() {
    tokio::time::timeout(Duration::from_secs(9), async {
        let host = TestHost::start().await;
        let app = app_identity();
        let ticket = host.new_pairing_code();
        let results = pair_with_code(&host, app.clone(), &ticket.code).await;
        assert_eq!(
            results
                .iter()
                .map(|result| result.status)
                .collect::<Vec<_>>(),
            vec![
                pb::pair_result::Status::WaitingLocal as i32,
                pb::pair_result::Status::Ok as i32,
            ],
            "{results:?}",
        );
        assert_eq!(
            results.last().expect("pairing result").host_id,
            host.host_id
        );
        let (session, mut events) = open(direct_params(&host, app))
            .await
            .expect("open newly paired session");
        let report = wait_frames(&mut events, 1, Duration::from_secs(2)).await;
        assert_eq!(report.frames, 1, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        session.close();
        host.stop().await;
    })
    .await
    .expect("pairing fixture check completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tcp_listener_has_a_loopback_address() {
    tokio::time::timeout(Duration::from_secs(9), async {
        let host = TestHost::start_with(TestHostOptions {
            tcp: true,
            ..TestHostOptions::default()
        })
        .await;
        let tcp = host.tcp_addr.expect("TCP listener is available");
        assert!(tcp.ip().is_loopback());
        assert_ne!(tcp.port(), 0);
        assert!(host.quic_addr.ip().is_loopback());
        assert_ne!(host.quic_addr.port(), 0);
        // Port zero allocates the TCP and QUIC ports independently.
        host.stop().await;
    })
    .await
    .expect("TCP fixture check completes within 9 seconds");
}
