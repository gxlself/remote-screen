use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use host_core::pipeline::{CropPlan, VideoPipeline};
use host_core::platform::{DisplayInfo, PixelRect, Platform, PlatformError, RoiRegion};
use host_fake::viewport::RegionCapture;
use host_fake::FakePlatform;
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::tls::{client_config, server_config, ServerPin};
use rsp_transport::{connect_quic, connect_tcp, QuicListener, TcpListener};
use tokio::time::{sleep, timeout};

struct ConnectionPair {
    server: rsp_transport::Conn,
    _client: rsp_transport::Conn,
    _endpoint: Option<quinn::Endpoint>,
}

async fn quic_pair() -> ConnectionPair {
    let server_id = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let listener = QuicListener::bind(
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        server_config(&server_id).unwrap(),
    )
    .await
    .unwrap();
    let client_id = LocalIdentity::generate_software(CertKind::App).unwrap();
    let client_cfg = Arc::new(
        client_config(&client_id, ServerPin::Fingerprint(server_id.fingerprint())).unwrap(),
    );
    let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    let (client, server) = tokio::join!(
        connect_quic(&endpoint, listener.local_addr().unwrap(), client_cfg, false),
        async { listener.accept().await.unwrap().accept().await.unwrap() },
    );
    ConnectionPair {
        server,
        _client: client.unwrap(),
        _endpoint: Some(endpoint),
    }
}

async fn tcp_pair() -> ConnectionPair {
    let server_id = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let listener = TcpListener::bind(
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        server_config(&server_id).unwrap(),
    )
    .await
    .unwrap();
    let client_id = LocalIdentity::generate_software(CertKind::App).unwrap();
    let client_cfg =
        client_config(&client_id, ServerPin::Fingerprint(server_id.fingerprint())).unwrap();
    let (client, server) = tokio::join!(
        async {
            let client = connect_tcp(listener.local_addr().unwrap(), client_cfg.into())
                .await
                .unwrap();
            let control = client.open_control().await.unwrap();
            control.send(&[]).await.unwrap();
            client
        },
        async {
            listener
                .accept()
                .await
                .unwrap()
                .accept()
                .await
                .unwrap()
                .unwrap()
        },
    );
    ConnectionPair {
        server,
        _client: client,
        _endpoint: None,
    }
}

fn plan() -> CropPlan {
    CropPlan {
        rect: PixelRect {
            x: 736,
            y: 352,
            w: 608,
            h: 352,
        },
        out_w: 608,
        out_h: 352,
    }
}

fn start_crop(
    platform: &FakePlatform,
    conn: rsp_transport::Conn,
    crop: Option<CropPlan>,
) -> host_core::platform::Result<VideoPipeline> {
    VideoPipeline::start_with_crop(
        platform,
        &platform.displays()[0],
        0,
        conn,
        1,
        1920,
        30,
        4_000,
        &[],
        false,
        None,
        crop,
    )
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        loop {
            if condition() {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unsupported_region_fails_before_encoder() {
    let platform = FakePlatform::new();
    let conn = quic_pair().await;
    let error = match start_crop(&platform, conn.server, Some(plan())) {
        Ok(_) => panic!("unsupported region unexpectedly started"),
        Err(error) => error,
    };
    assert!(matches!(error, PlatformError::Unsupported));
    assert!(platform.encoders_started().is_empty());
}

#[tokio::test]
async fn crop_opens_encoder_at_out_size() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let conn = quic_pair().await;
    let pipeline = start_crop(&platform, conn.server, Some(plan())).unwrap();
    assert_eq!(
        platform.encoders_started().last().copied(),
        Some((608, 352, 30, 4_000))
    );
    assert_eq!(
        platform.viewport().region_captures(),
        vec![RegionCapture {
            display: 1,
            rect: plan().rect,
            width: 608,
            height: 352,
        }]
    );
    assert_eq!(pipeline.crop(), Some(plan()));
    assert_eq!((pipeline.width, pipeline.height), (608, 352));
    assert_eq!(platform.active_captures(), 0);
}

#[tokio::test]
async fn crop_on_tcp_caps_height() {
    let display = DisplayInfo {
        id: 1,
        name: "large synthetic display".into(),
        width_px: 3840,
        height_px: 2160,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    };
    let platform = FakePlatform::with_displays(vec![display]);
    platform.viewport().set_region_supported(true);
    let tcp_plan = CropPlan {
        rect: PixelRect {
            x: 0,
            y: 0,
            w: 3840,
            h: 2160,
        },
        out_w: 2560,
        out_h: 1440,
    };
    let tcp = tcp_pair().await;
    let pipeline = start_crop(&platform, tcp.server, Some(tcp_plan)).unwrap();
    assert_eq!((pipeline.width, pipeline.height), (1920, 1080));
    assert_eq!(
        platform.viewport().region_captures().last().unwrap().width,
        1920
    );
    assert_eq!(
        platform.viewport().region_captures().last().unwrap().height,
        1080
    );
    drop(pipeline);

    let quic = quic_pair().await;
    let pipeline = start_crop(&platform, quic.server, Some(tcp_plan)).unwrap();
    assert_eq!((pipeline.width, pipeline.height), (2560, 1440));
    assert_eq!(
        platform.viewport().region_captures().last().unwrap().width,
        2560
    );
    assert_eq!(
        platform.viewport().region_captures().last().unwrap().height,
        1440
    );
}

#[tokio::test]
async fn invalid_plan_rejected() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let invalid = CropPlan {
        rect: PixelRect {
            x: 1_800,
            y: 0,
            w: 200,
            h: 100,
        },
        out_w: 608,
        out_h: 352,
    };
    let conn = quic_pair().await;
    assert!(matches!(
        start_crop(&platform, conn.server, Some(invalid)),
        Err(PlatformError::Platform(message)) if message == "invalid crop plan"
    ));

    let invalid = CropPlan {
        out_w: 607,
        ..plan()
    };
    let conn = quic_pair().await;
    assert!(matches!(
        start_crop(&platform, conn.server, Some(invalid)),
        Err(PlatformError::Platform(message)) if message == "invalid crop plan"
    ));
}

#[tokio::test]
async fn region_failure_propagates() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_fails(true);
    let conn = quic_pair().await;
    let error = match start_crop(&platform, conn.server, Some(plan())) {
        Ok(_) => panic!("injected region failure unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(error, PlatformError::Platform(message) if message.contains("injected")));
}

#[tokio::test]
async fn drop_stops_region_capture() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let conn = quic_pair().await;
    let pipeline = start_crop(&platform, conn.server, Some(plan())).unwrap();
    assert_eq!(platform.viewport().active_region_captures(), 1);
    drop(pipeline);
    wait_until(|| platform.viewport().active_region_captures() == 0).await;
}

#[tokio::test]
async fn roi_forwarded_when_supported() {
    let platform = FakePlatform::new();
    platform.viewport().set_roi_supported(true);
    let conn = quic_pair().await;
    let pipeline = start_crop(&platform, conn.server, None).unwrap();
    assert!(pipeline.roi_supported());
    let regions = vec![RoiRegion {
        rect: PixelRect {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
        },
        qp_delta: -6,
    }];
    pipeline.set_roi(regions.clone());
    wait_until(|| platform.viewport().roi_calls().len() == 1).await;
    pipeline.set_roi(Vec::new());
    wait_until(|| platform.viewport().roi_calls().len() == 2).await;
    assert_eq!(platform.viewport().roi_calls(), vec![regions, Vec::new()]);
}

#[tokio::test]
async fn roi_ignored_when_unsupported() {
    let platform = FakePlatform::new();
    let conn = quic_pair().await;
    let pipeline = start_crop(&platform, conn.server, None).unwrap();
    assert!(!pipeline.roi_supported());
    pipeline.set_roi(vec![RoiRegion {
        rect: PixelRect {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
        },
        qp_delta: -6,
    }]);
    sleep(Duration::from_millis(300)).await;
    assert!(platform.viewport().roi_calls().is_empty());
}

#[tokio::test]
async fn full_frame_unchanged() {
    let platform = FakePlatform::new();
    let conn = quic_pair().await;
    let pipeline = start_crop(&platform, conn.server, None).unwrap();
    assert_eq!(pipeline.crop(), None);
    assert!(platform.viewport().region_captures().is_empty());
    assert_eq!(platform.active_captures(), 1);
}
