mod support;

use std::time::Duration;

use rsp_client::session::{
    close_code_to_error, goodbye_reason_error, transition, Session, SessionEvent, SessionParams,
    SessionStateKind, StateTransition,
};
use rsp_core::{errors::ErrorCode, pb, video};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    DatagramKind,
};
use tokio::time::timeout;

fn params(host: &support::Host) -> SessionParams {
    SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    )
}

#[test]
fn protocol_state_table_has_only_documented_edges() {
    use SessionStateKind as S;
    use StateTransition as E;
    let cases = [
        (S::Connecting, E::Connected, S::Authenticating),
        (S::Authenticating, E::ApprovalPending, S::PendingApproval),
        (S::Authenticating, E::Authenticated, S::Streaming),
        (S::Authenticating, E::StartStreaming, S::Streaming),
        (S::PendingApproval, E::ApprovalGranted, S::Streaming),
        (S::Streaming, E::Disconnected, S::Reconnecting),
        (S::Reconnecting, E::Authenticated, S::Streaming),
        (S::Reconnecting, E::ApprovalPending, S::PendingApproval),
        (S::Connecting, E::End(ErrorCode::Other), S::Ended),
        (S::Authenticating, E::End(ErrorCode::Other), S::Ended),
        (S::PendingApproval, E::End(ErrorCode::Other), S::Ended),
        (S::Streaming, E::End(ErrorCode::Other), S::Ended),
        (S::Reconnecting, E::End(ErrorCode::Other), S::Ended),
    ];
    for state in [
        S::Connecting,
        S::Authenticating,
        S::PendingApproval,
        S::Streaming,
        S::Reconnecting,
        S::Ended,
    ] {
        for event in [
            E::Connected,
            E::Authenticated,
            E::ApprovalPending,
            E::ApprovalGranted,
            E::StartStreaming,
            E::Disconnected,
            E::End(ErrorCode::Other),
        ] {
            let expected = cases
                .iter()
                .find(|(s, e, _)| *s == state && *e == event)
                .map(|(_, _, next)| *next);
            match expected {
                Some(next) => assert_eq!(transition(state, event), Ok(next)),
                None => {
                    let error = transition(state, event).expect_err("illegal transition");
                    assert_eq!((error.from, error.event), (state, event));
                }
            }
        }
    }
}

#[test]
fn goodbye_and_quic_close_codes_share_the_error_table() {
    assert_eq!(
        goodbye_reason_error(pb::goodbye::Reason::Revoked),
        Some(ErrorCode::DeviceRevoked)
    );
    assert_eq!(
        goodbye_reason_error(pb::goodbye::Reason::PermissionLost),
        Some(ErrorCode::MissingPermissions)
    );
    for (code, expected) in [
        (0, ErrorCode::Other),
        (1, ErrorCode::Other),
        (2, ErrorCode::PolicyDenied),
        (3, ErrorCode::Other),
        (4, ErrorCode::Other),
        (5, ErrorCode::Other),
        (6, ErrorCode::DeviceRevoked),
    ] {
        assert_eq!(
            close_code_to_error(code),
            if matches!(code, 0 | 3 | 4 | 5) {
                None
            } else {
                Some(expected)
            }
        );
    }
}

#[tokio::test]
async fn normal_session_delivers_a_first_keyframe_and_retains_token() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::send(
            &control,
            pb::SessionState {
                your_role: pb::Role::Controller as i32,
                ..Default::default()
            },
        )
        .await;
        support::send(
            &control,
            pb::StreamConfig {
                epoch: 1,
                fps: 30,
                width: 640,
                height: 360,
                ..Default::default()
            },
        )
        .await;
        let packets = video::packetize(
            &[0, 0, 0, 1, 0x26, 1, 4, 5],
            video::FrameMeta {
                flags: video::FLAG_KEYFRAME,
                display_id: 1,
                epoch: 1,
                frame_id: 0,
                capture_ts_us: rsp_core::clock::unix_micros(),
            },
            1150,
            0.0,
        )
        .expect("video");
        for packet in packets {
            conn.send_datagram(DatagramKind::Video, packet.slice(1..))
                .expect("video send");
        }
        received_rx.await.expect("client receives frame and state");
        support::finish(&conn, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    let mut frame = false;
    let mut state = false;
    let mut received_tx = Some(received_tx);
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::Frame(value) => {
                    frame = value.frame.keyframe;
                }
                SessionEvent::SessionState(_) => state = true,
                SessionEvent::Ended { .. } => break,
                _ => {}
            }
            if frame && state {
                if let Some(received_tx) = received_tx.take() {
                    received_tx.send(()).expect("Host awaits receipt");
                }
            }
        }
    })
    .await
    .expect("session deadline");
    assert!(frame && state);
    assert_eq!(handle.resume_token(), Some(vec![7; 16]));
    server.await.expect("Host script");
}

#[tokio::test]
async fn pending_approval_can_be_cancelled() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (_conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::PendingApproval).await;
        assert!(
            matches!(support::recv(&control).await, pb::envelope::Msg::Goodbye(value) if value.reason == pb::goodbye::Reason::UserClosed as i32)
        );
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                SessionEvent::StateChanged(SessionStateKind::PendingApproval)
            ) {
                handle.close();
                break;
            }
        }
    })
    .await
    .expect("pending deadline");
    server.await.expect("Host script");
}

#[tokio::test]
async fn probe_completes_on_user_closed_goodbye() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        assert!(
            matches!(support::recv(&control).await, pb::envelope::Msg::ClientHello(value) if value.intent == pb::Intent::Probe as i32)
        );
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                host_name: "probe-host".to_owned(),
                ..Default::default()
            },
        )
        .await;
        support::finish(&conn, &control).await;
    });
    let result = timeout(support::TEST_TIMEOUT, rsp_client::probe(params))
        .await
        .expect("probe deadline")
        .expect("probe");
    assert!(result.completed);
    assert_eq!(result.server_hello.expect("hello").host_name, "probe-host");
    server.await.expect("Host script");
}

async fn ended(
    events: &mut rsp_client::EventStream,
) -> (Option<ErrorCode>, rsp_client::session::EndReason) {
    timeout(support::TEST_TIMEOUT, async {
        loop {
            if let Some(SessionEvent::Ended { code, kind, .. }) = events.recv().await {
                return (code, kind);
            }
        }
    })
    .await
    .expect("session ends")
}

#[tokio::test]
async fn approval_granted_cancels_the_local_deadline() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let (grant_tx, grant_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::PendingApproval).await;
        grant_rx.await.expect("client observes pending approval");
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        // Retain the real transport until the test has checked the deadline.
        let _ = conn.closed().await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        events.recv().await.expect("pending approval event"),
        SessionEvent::StateChanged(SessionStateKind::PendingApproval)
    ) {}
    grant_tx.send(()).expect("Host awaits approval grant");
    while !matches!(
        events.recv().await,
        Some(SessionEvent::StateChanged(SessionStateKind::Streaming))
    ) {}
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(7)).await;
    tokio::task::yield_now().await;
    assert_eq!(handle.state(), SessionStateKind::Streaming);
    handle.close();
    tokio::time::resume();
    let _ = ended(&mut events).await;
    server.await.expect("Host script");
}

#[tokio::test]
async fn approval_timeout_is_timeout_plus_five_seconds() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::PendingApproval).await;
        let payload = control
            .recv()
            .await
            .expect("timeout goodbye")
            .expect("record");
        use prost::Message;
        assert!(matches!(
            pb::Envelope::decode(payload).expect("decode").msg,
            Some(pb::envelope::Msg::Goodbye(_))
        ));
        conn.close(rsp_transport::CloseCode::Normal, b"timeout accepted");
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        events.recv().await,
        Some(SessionEvent::StateChanged(
            SessionStateKind::PendingApproval
        ))
    ) {}
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(handle.state(), SessionStateKind::PendingApproval);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(ended(&mut events).await.0, Some(ErrorCode::ApprovalDenied));
    tokio::time::resume();
    server.await.expect("Host script");
}

#[tokio::test]
async fn authentication_denied_propagates_140_and_142() {
    for code in [ErrorCode::ApprovalDenied, ErrorCode::SessionFull] {
        let host = support::Host::bind().await;
        let params = params(&host);
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            support::hello(&control).await;
            support::send(
                &control,
                pb::AuthResult {
                    status: pb::auth_result::Status::Denied as i32,
                    error: Some(pb::ErrorInfo {
                        code: u32::from(code.code()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await;
            conn.closed().await;
        });
        let (_handle, mut events) = Session::open(params).await.expect("open");
        assert_eq!(ended(&mut events).await.0, Some(code));
        server.await.expect("Host script");
    }
}

#[tokio::test]
async fn viewer_takeover_control_response_and_nonfatal_141() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Viewer as i32,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::send(
            &control,
            pb::SessionState {
                your_role: pb::Role::Viewer as i32,
                controller: Some(pb::Participant {
                    device_id: "d-controller".into(),
                    ..Default::default()
                }),
                viewers: vec![pb::Participant {
                    device_id: "d-viewer".into(),
                    ..Default::default()
                }],
                input_paused_local: true,
            },
        )
        .await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::RequestControl(_)
        ));
        support::send(
            &control,
            pb::ErrorInfo {
                code: 141,
                message: "takeover denied".into(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::RequestControl(_)
        ));
        support::send(
            &control,
            pb::SessionState {
                your_role: pb::Role::Controller as i32,
                ..Default::default()
            },
        )
        .await;
        support::send(
            &control,
            pb::ControlRequest {
                requester: Some(pb::Participant {
                    device_id: "d-requester".into(),
                    ..Default::default()
                }),
                timeout_s: 30,
            },
        )
        .await;
        assert!(
            matches!(support::recv_command(&control).await, pb::envelope::Msg::ControlResponse(r) if r.requester_device_id == "d-requester" && r.allow)
        );
        support::finish(&conn, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    let mut viewer = false;
    let mut controller = false;
    let mut denied = false;
    let mut local_use = false;
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::SessionState(s) if s.your_role == pb::Role::Viewer as i32 => {
                    assert_eq!(s.viewers.len(), 1);
                    viewer = true;
                    handle.request_control();
                }
                SessionEvent::Role(pb::Role::Controller) => controller = true,
                SessionEvent::LocalUse(true) => local_use = true,
                SessionEvent::Error {
                    code: ErrorCode::TakeoverDenied,
                    ..
                } => {
                    denied = true;
                    handle.request_control();
                }
                SessionEvent::ControlRequested(r) => {
                    handle.respond_control(r.requester.expect("requester").device_id, true)
                }
                SessionEvent::Ended { .. } => break,
                _ => {}
            }
        }
    })
    .await
    .expect("takeover deadline");
    assert!(viewer && controller && denied && local_use);
    server.await.expect("Host script");
}

#[tokio::test]
async fn pause_resume_select_quality_and_unexpected_disconnect() {
    let host = support::Host::bind().await;
    let mut params = params(&host);
    params.resume_token = Some(vec![3; 16]);
    let (lost_tx, lost_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        assert!(
            matches!(support::recv(&control).await, pb::envelope::Msg::ClientHello(h) if h.resume_token == vec![3;16])
        );
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                ..Default::default()
            },
        )
        .await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::PauseStream(_)
        ));
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::ResumeStream(_)
        ));
        assert!(
            matches!(support::recv_command(&control).await,pb::envelope::Msg::SelectDisplay(s) if s.display_id == 2)
        );
        assert!(
            matches!(support::recv_command(&control).await,pb::envelope::Msg::SetQuality(q) if q.quality == pb::Quality::Clear as i32)
        );
        // Rebind the server socket to simulate a vanished network path,
        // while retaining the endpoint so it cannot send an orderly close.
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("rebind socket");
        host.listener.endpoint().rebind(socket).expect("rebind");
        let _ = lost_tx.send(());
        conn.closed().await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        events.recv().await,
        Some(SessionEvent::StateChanged(SessionStateKind::Streaming))
    ) {}
    handle.pause();
    handle.resume();
    handle.select_display(2);
    handle.set_quality(pb::Quality::Clear);
    lost_rx.await.expect("server changed path");
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(1500)).await;
    tokio::task::yield_now().await;
    for _ in 0..60 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    let mut reconnect = false;
    loop {
        match events.recv().await.expect("event") {
            SessionEvent::StateChanged(SessionStateKind::Reconnecting) => reconnect = true,
            SessionEvent::Ended { code, .. } => {
                assert_eq!(code, Some(ErrorCode::Other));
                break;
            }
            _ => {}
        }
    }
    assert!(reconnect);
    tokio::time::resume();
    assert_eq!(handle.resume_token(), None);
    server.await.expect("Host script");
}

#[tokio::test]
async fn goodbye_endings_and_idle_warning() {
    use pb::goodbye::Reason;
    for reason in [
        Reason::UserClosed,
        Reason::HostClosedByLocalUser,
        Reason::IdleTimeout,
        Reason::Revoked,
    ] {
        let host = support::Host::bind().await;
        let params = params(&host);
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            support::hello(&control).await;
            if reason == Reason::IdleTimeout {
                support::send(
                    &control,
                    pb::IdleWarning {
                        seconds_left: 60,
                        lock_on_end: true,
                    },
                )
                .await;
            }
            support::send(
                &control,
                pb::Goodbye {
                    reason: reason as i32,
                    message: String::new(),
                },
            )
            .await;
            conn.closed().await;
        });
        let (_handle, mut events) = Session::open(params).await.expect("open");
        let mut warning = false;
        loop {
            match timeout(support::TEST_TIMEOUT, events.recv())
                .await
                .expect("deadline")
                .expect("event")
            {
                SessionEvent::IdleWarning(w) => {
                    warning = true;
                    assert_eq!(w.seconds_left, 60);
                }
                SessionEvent::Ended { code, kind, .. } => {
                    assert_eq!(kind, rsp_client::session::EndReason::Goodbye(reason));
                    assert_eq!(code, goodbye_reason_error(reason));
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(warning, reason == Reason::IdleTimeout);
        server.await.expect("script");
    }
}

#[tokio::test]
async fn quic_close_codes_zero_through_six() {
    for close in [
        rsp_transport::CloseCode::Normal,
        rsp_transport::CloseCode::Protocol,
        rsp_transport::CloseCode::Auth,
        rsp_transport::CloseCode::Replaced,
        rsp_transport::CloseCode::HostShutdown,
        rsp_transport::CloseCode::Idle,
        rsp_transport::CloseCode::Revoked,
    ] {
        let host = support::Host::bind().await;
        let params = params(&host);
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            support::hello(&control).await;
            conn.close(close, b"scripted application close");
            host.listener.endpoint().wait_idle().await;
        });
        let (_handle, mut events) = Session::open(params).await.expect("open");
        assert_eq!(
            ended(&mut events).await.0,
            close_code_to_error(close as u64)
        );
        server.await.expect("script");
    }
}

#[tokio::test]
async fn version_and_required_features_are_checked_both_ways() {
    for (protocol, required, server_features, client_required, code) in [
        (0, 0, 0, 0, ErrorCode::HostTooOld),
        (2, 0, 0, 0, ErrorCode::AppTooOld),
        (1, 1u64 << 63, 0, 0, ErrorCode::AppTooOld),
        (1, 0, 0, rsp_core::features::FEC_RS, ErrorCode::HostTooOld),
    ] {
        let host = support::Host::bind().await;
        let mut params = params(&host);
        params.required_features = client_required;
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            let _ = support::recv(&control).await;
            support::send(
                &control,
                pb::ServerHello {
                    protocol,
                    required_features: required,
                    features: server_features,
                    ..Default::default()
                },
            )
            .await;
            conn.closed().await;
        });
        let (_handle, mut events) = Session::open(params).await.expect("open");
        assert_eq!(ended(&mut events).await.0, Some(code));
        server.await.expect("script");
    }
}

#[tokio::test]
async fn authentication_before_server_hello_is_a_protocol_error() {
    let host = support::Host::bind().await;
    let p = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        let _ = support::recv(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert_eq!(conn.closed().await.code, Some(1));
    });
    let (_handle, mut events) = Session::open(p).await.expect("open");
    let (code, reason) = ended(&mut events).await;
    assert_eq!(code, Some(ErrorCode::Other));
    assert_eq!(reason, rsp_client::EndReason::Protocol);
    server.await.expect("script");
}

#[tokio::test]
async fn a_different_host_pin_is_classified_as_fingerprint_mismatch() {
    let host = support::Host::bind().await;
    let mut p = params(&host);
    p.host_fingerprint = [0; 32];
    let server = tokio::spawn(async move {
        let incoming = host.listener.accept().await.expect("incoming");
        // TLS can finish on the server before the racing dialer verifies the
        // SPKI. Neither outcome may reach a session control stream.
        if let Ok(conn) = incoming.accept().await {
            assert!(conn.accept_control().await.is_err());
        }
    });
    assert!(matches!(
        Session::open(p).await,
        Err(rsp_client::SessionError::Dial {
            code: ErrorCode::FingerprintMismatch,
            ..
        })
    ));
    server.await.expect("pin script");
}

#[tokio::test]
async fn prewarm_capacity_expiration_identity_and_reuse() {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    struct Clock(AtomicU64);
    impl rsp_client::prewarm::Clock for Clock {
        fn now_us(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let pool = Arc::new(rsp_client::PrewarmPool::with_clock(clock.clone()));
    let mut hosts = Vec::new();
    let mut endpoints = Vec::new();
    let mut servers = Vec::new();
    for _ in 0..5 {
        let host = support::Host::bind().await;
        let identity = LocalIdentity::generate_software(CertKind::App).expect("identity");
        let endpoint =
            quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("endpoint");
        let config = rsp_transport::tls::client_config(
            &identity,
            rsp_transport::tls::ServerPin::Fingerprint(host.fingerprint),
        )
        .expect("TLS");
        let (client, server) = tokio::join!(
            rsp_transport::connect_quic(&endpoint, host.addr, Arc::new(config), true),
            async {
                host.listener
                    .accept()
                    .await
                    .expect("incoming")
                    .accept()
                    .await
                    .expect("accept")
            }
        );
        let client = client.expect("client");
        assert!(
            timeout(Duration::from_millis(10), server.accept_control())
                .await
                .is_err(),
            "prewarm must not open control"
        );
        pool.insert(host.fingerprint, client).expect("insert");
        hosts.push(host);
        endpoints.push(endpoint);
        servers.push(server);
    }
    assert_eq!(pool.len(), 4);
    assert!(pool.take(hosts[0].fingerprint).is_none(), "oldest evicted");
    assert!(pool.take([0; 32]).is_none(), "other identity cannot reuse");
    let mut params = params(&hosts[4]);
    params.prewarm = Some(pool.clone());
    let server = servers.pop().expect("server");
    let script = tokio::spawn(async move {
        let control = server.accept_control().await.expect("reused control");
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::finish(&server, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("reuse");
    let _ = ended(&mut events).await;
    assert!(handle.handshake_timings().reused);
    assert_eq!(pool.len(), 3);
    clock.0.store(59_999_999, Ordering::Relaxed);
    assert_eq!(pool.expire(), 0);
    clock.0.store(60_000_000, Ordering::Relaxed);
    assert_eq!(pool.expire(), 3);
    assert!(pool.is_empty());
    script.await.expect("reuse script");
    drop((hosts, endpoints, servers));
}

#[tokio::test]
async fn prewarm_timer_closes_without_further_pool_access() {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    struct Clock(AtomicU64);
    impl rsp_client::prewarm::Clock for Clock {
        fn now_us(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }
    let host = support::Host::bind().await;
    let identity = LocalIdentity::generate_software(CertKind::App).expect("identity");
    let endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("endpoint");
    let config = rsp_transport::tls::client_config(
        &identity,
        rsp_transport::tls::ServerPin::Fingerprint(host.fingerprint),
    )
    .expect("TLS");
    let (client, server) = tokio::join!(
        rsp_transport::connect_quic(&endpoint, host.addr, Arc::new(config), true),
        async {
            host.listener
                .accept()
                .await
                .expect("incoming")
                .accept()
                .await
                .expect("accept")
        }
    );
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let pool = rsp_client::PrewarmPool::with_clock(clock.clone());
    pool.insert(host.fingerprint, client.expect("client"))
        .expect("insert");
    tokio::task::yield_now().await;
    tokio::time::pause();
    clock.0.store(60_000_000, Ordering::Relaxed);
    tokio::time::advance(Duration::from_secs(60)).await;
    let close = timeout(Duration::from_secs(1), server.closed())
        .await
        .expect("expiry closes remote");
    assert_eq!(close.code, Some(0));
    tokio::time::resume();
}

#[tokio::test(start_paused = true)]
async fn derp_only_uses_the_five_second_dial_budget_and_classifies_failure() {
    use std::sync::Arc;
    struct Blocked;
    impl rsp_transport::dialer::Dialer for Blocked {
        fn dial<'a>(
            &'a self,
            _: std::net::SocketAddr,
            _: rsp_transport::TransportKind,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<rsp_transport::Conn, rsp_transport::ConnError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(100)).await;
                Err(rsp_transport::ConnError::Closed)
            })
        }
    }
    for (derp, seconds) in [(false, 3), (true, 5)] {
        let mut p = SessionParams::single(
            "100.64.0.1:7400".parse().expect("example address"),
            [0; 32],
            LocalIdentity::generate_software(CertKind::App).expect("identity"),
        );
        p.derp_only = derp;
        p.dialer = Some(Arc::new(Blocked));
        let start = tokio::time::Instant::now();
        assert!(matches!(
            Session::open(p).await,
            Err(rsp_client::SessionError::Dial {
                code: ErrorCode::Other,
                ..
            })
        ));
        assert_eq!(start.elapsed(), Duration::from_secs(seconds));
    }
}

#[tokio::test]
async fn cursor_hid_pointer_feedback_and_statistics_are_connected() {
    use rsp_core::{features, hid::InputMsg, pointer::PointerDatagram};
    let host = support::Host::bind().await;
    let params = params(&host);
    let (video_tx, video_rx) = tokio::sync::oneshot::channel();
    let (feedback_tx, feedback_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        let _ = support::recv(&control).await;
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RATE_FEEDBACK
                    | features::CURSOR_SHAPE
                    | features::POINTER_DGRAM,
                ..Default::default()
            },
        )
        .await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::send(
            &control,
            pb::DisplayList {
                displays: vec![pb::Display {
                    id: 2,
                    ..Default::default()
                }],
            },
        )
        .await;
        support::send(
            &control,
            pb::RateUpdate {
                target_kbps: 1000,
                ..Default::default()
            },
        )
        .await;
        support::send(
            &control,
            pb::HostStats {
                capture_fps: 30,
                ..Default::default()
            },
        )
        .await;
        support::send(&control, pb::LocalActivity { active: true }).await;
        let mut cursor = conn
            .open_uni(rsp_transport::StreamKind::Cursor)
            .await
            .expect("cursor");
        use prost::Message;
        cursor
            .send_record(
                &pb::CursorMsg {
                    msg: Some(pb::cursor_msg::Msg::Hidden(true)),
                }
                .encode_to_vec(),
            )
            .await
            .expect("cursor record");
        let (kind, mut input) = conn.accept_uni().await.expect("input");
        assert_eq!(kind, rsp_transport::StreamKind::Input);
        assert!(matches!(
            InputMsg::decode(
                &input
                    .recv_record()
                    .await
                    .expect("input read")
                    .expect("input record")
            )
            .expect("input decode"),
            InputMsg::Key {
                usage: 4,
                down: true,
                ..
            }
        ));
        let (kind, pointer) = conn.recv_datagram().await.expect("pointer");
        assert_eq!(kind, DatagramKind::Pointer);
        let mut wire = vec![rsp_core::pointer::POINTER_TYPE];
        wire.extend_from_slice(&pointer);
        assert_eq!(
            PointerDatagram::decode(&wire).expect("pointer decode").x,
            0.25
        );
        video_rx
            .await
            .expect("client starts video reception window");
        let data = vec![1; 800];
        let packets = video::packetize(
            &data,
            video::FrameMeta {
                flags: video::FLAG_KEYFRAME,
                epoch: 1,
                display_id: 0,
                frame_id: 0,
                capture_ts_us: rsp_core::clock::unix_micros(),
            },
            256,
            0.5,
        )
        .expect("FEC packetize");
        let received_shards = packets.len() as u32 - 1;
        assert_eq!(received_shards, 5);
        for packet in packets.into_iter().skip(1) {
            conn.send_datagram(DatagramKind::Video, packet.slice(1..))
                .expect("video");
        }
        let mut pings = 0;
        let mut feedback = false;
        let mut feedback_tx = Some(feedback_tx);
        while pings < 8 || !feedback {
            match support::recv(&control).await {
                pb::envelope::Msg::Ping(p) => {
                    pings += 1;
                    support::send(
                        &control,
                        pb::Pong {
                            t1_us: p.t1_us,
                            t2_us: rsp_core::clock::unix_micros(),
                        },
                    )
                    .await;
                }
                pb::envelope::Msg::Feedback(f) if f.frames_received > 0 => {
                    assert_eq!(f.frames_recovered, 1);
                    assert_eq!(f.shards_received, received_shards);
                    assert_eq!(f.shards_lost, 1);
                    feedback = true;
                    if let Some(feedback_tx) = feedback_tx.take() {
                        feedback_tx.send(()).expect("client awaits Feedback");
                    }
                }
                _ => {}
            }
        }
        finish_rx.await.expect("client observes statistics");
        support::finish(&conn, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    let mut connected = false;
    let mut cursor = false;
    let mut display = false;
    let mut rate = false;
    let mut host_stats = false;
    let mut stats = false;
    let mut frame = false;
    // TLS uses real loopback I/O. Freeze session time after dialing, then keep
    // it fixed while waiting for real I/O, advancing only at known checkpoints.
    tokio::time::pause();
    support::without_time_advancing(async {
        while !(connected && cursor && display && rate && host_stats) {
            match events.recv().await.expect("session event") {
                SessionEvent::StateChanged(SessionStateKind::Streaming) if !connected => {
                    connected = true;
                    handle
                        .send_input(InputMsg::Key {
                            usage: 4,
                            down: true,
                            mods: rsp_core::hid::Mods::NONE,
                        })
                        .expect("key");
                    handle.send_pointer(0.25, 0.75);
                }
                SessionEvent::CursorMessage(_) => cursor = true,
                SessionEvent::Displays(d) if d.first().is_some_and(|d| d.id == 2) => display = true,
                SessionEvent::RateUpdate(_) => rate = true,
                SessionEvent::HostStats(_) => host_stats = true,
                SessionEvent::Stats(_) => stats = true,
                SessionEvent::Frame(_) => frame = true,
                SessionEvent::Ended { .. } => panic!("session ended before receiving test data"),
                _ => {}
            }
        }
        // Receive video at 100 ms so the first Feedback's trailing window has
        // room for Tokio's millisecond timer rounding on either boundary.
        tokio::time::advance(Duration::from_millis(100)).await;
        video_tx.send(()).expect("Host awaits video window");
        let mut pong = false;
        while !(frame && pong) {
            match events.recv().await.expect("video and first Pong") {
                SessionEvent::Frame(_) => frame = true,
                SessionEvent::ClockSync { .. } => pong = true,
                SessionEvent::Ended { .. } => panic!("session ended before video and first Pong"),
                _ => {}
            }
        }
        let mut feedback_rx = Some(feedback_rx);
        for expected_pings in 2..=8 {
            // The startup Ping schedule sends eight probes 60 ms apart. Wait
            // for each Pong before advancing again, so a skipped timer tick
            // cannot reduce the number of probes on a busy executor. The
            // 80 ms steps also allow for timer rounding and the 20 ms tick.
            tokio::time::advance(Duration::from_millis(80)).await;
            loop {
                match events.recv().await.expect("Ping/Pong event") {
                    SessionEvent::Stats(_) => stats = true,
                    SessionEvent::ClockSync { .. } => break,
                    SessionEvent::Ended { .. } => panic!("session ended before eight Pings"),
                    _ => {}
                }
            }
            assert!(handle.stats().pings_sent >= expected_pings);
            if expected_pings == 3 {
                // At 260 ms every shard is still in the trailing 200 ms
                // window. Wait for the Host to validate the exact counts.
                feedback_rx
                    .take()
                    .expect("Feedback receiver")
                    .await
                    .expect("Host validates Feedback");
            }
        }
        // A Stats event is produced every 500 ms; wait for its delivery before
        // permitting the Host to close the session.
        tokio::time::advance(Duration::from_millis(500)).await;
        while !stats {
            match events.recv().await.expect("statistics event") {
                SessionEvent::Stats(_) => stats = true,
                SessionEvent::Ended { .. } => panic!("session ended before statistics"),
                _ => {}
            }
        }
    })
    .await;
    tokio::time::resume();
    finish_tx.send(()).expect("Host awaits statistics");
    let _ = ended(&mut events).await;
    assert!(connected && cursor && display && rate && host_stats && stats && frame);
    assert!(handle.stats().pings_sent >= 8);
    assert_eq!(handle.stats().frames_recovered, 1);
    server.await.expect("script");
}

#[tokio::test]
async fn tcp_fallback_attaches_input_after_authentication() {
    use rsp_core::hid::InputMsg;
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("Host identity");
    let listener = rsp_transport::TcpListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        rsp_transport::tls::server_config(&identity).expect("TLS"),
    )
    .await
    .expect("TCP listener");
    let p = SessionParams::single(
        listener.local_addr().expect("addr"),
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    let server = tokio::spawn(async move {
        let conn = listener
            .accept()
            .await
            .expect("incoming")
            .accept()
            .await
            .expect("TLS")
            .expect("main connection");
        let control = conn.accept_control().await.expect("control");
        assert!(
            matches!(support::recv(&control).await,pb::envelope::Msg::ClientHello(h) if h.transport==pb::Transport::Tcp as i32)
        );
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: rsp_core::features::TCP_TRANSPORT,
                ..Default::default()
            },
        )
        .await;
        listener
            .register_token(vec![7; 16], &conn)
            .expect("register input token");
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        assert!(listener
            .accept()
            .await
            .expect("input incoming")
            .accept()
            .await
            .expect("input TLS")
            .is_none());
        let (kind, mut input) = conn.accept_uni().await.expect("input");
        assert_eq!(kind, rsp_transport::StreamKind::Input);
        assert_eq!(
            InputMsg::decode(&input.recv_record().await.expect("read").expect("record"))
                .expect("decode"),
            InputMsg::ReleaseAll
        );
        let data = video::packetize_tcp(
            &[0, 0, 0, 1, 0x26],
            video::FrameMeta {
                flags: video::FLAG_KEYFRAME,
                display_id: 1,
                epoch: 1,
                frame_id: 0,
                capture_ts_us: 0,
            },
        )
        .expect("video");
        for packet in data {
            conn.send_datagram(DatagramKind::Video, packet.slice(1..))
                .expect("video send");
        }
        loop {
            match support::recv(&control).await {
                pb::envelope::Msg::Goodbye(g) => {
                    assert_eq!(g.reason, pb::goodbye::Reason::UserClosed as i32);
                    conn.close(rsp_transport::CloseCode::Normal, b"TCP script finished");
                    break;
                }
                pb::envelope::Msg::Ping(_) => {}
                message => panic!("unexpected TCP control message: {message:?}"),
            }
        }
    });
    let (handle, mut events) = Session::open(p).await.expect("fallback");
    let mut frame = false;
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Streaming) => handle
                    .send_input(InputMsg::ReleaseAll)
                    .expect("input encode"),
                SessionEvent::Frame(_) => {
                    frame = true;
                    handle.close();
                }
                SessionEvent::Ended { .. } => break,
                _ => {}
            }
        }
    })
    .await
    .expect("TCP session deadline");
    server.await.expect("TCP script");
    assert!(frame);
}

#[tokio::test]
async fn denied_handshake_preserves_error_without_disclosing_server_hello() {
    let host = support::Host::bind().await;
    let p = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        let _ = support::recv(&control).await;
        support::send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                error: Some(pb::ErrorInfo {
                    code: ErrorCode::NotPaired.code().into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(conn.closed().await.code, Some(2));
    });
    let (_handle, mut events) = Session::open(p).await.expect("open");
    assert_eq!(ended(&mut events).await.0, Some(ErrorCode::NotPaired));
    server.await.expect("script");
}

#[tokio::test]
async fn pairing_ok_cannot_bypass_spake_key_confirmation() {
    let host = support::Host::bind().await;
    let mut p = params(&host).with_pairing_code("123456");
    p.intent = pb::Intent::Pair;
    let host_id = rsp_core::ids::id_from_fp(&host.fingerprint);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::PairStart(_)
        ));
        support::send(
            &control,
            pb::PairResult {
                status: pb::pair_result::Status::Ok as i32,
                host_id,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(conn.closed().await.code, Some(1));
    });
    let (_handle, mut events) = Session::open(p).await.expect("open");
    loop {
        match timeout(support::TEST_TIMEOUT, events.recv())
            .await
            .expect("event deadline")
            .expect("session event")
        {
            SessionEvent::PairResult(_) => panic!("unverified result was exposed"),
            SessionEvent::Ended { kind, .. } => {
                assert_eq!(kind, rsp_client::EndReason::Protocol);
                break;
            }
            _ => {}
        }
    }
    server.await.expect("script");
}

#[tokio::test]
async fn request_keyframe_for_uses_last_delivered_frame_and_is_rate_limited() {
    let host = support::Host::bind().await;
    let p = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::send(
            &control,
            pb::StreamConfig {
                epoch: 1,
                fps: 30,
                ..Default::default()
            },
        )
        .await;
        let frame = video::packetize(
            &[1; 64],
            video::FrameMeta {
                flags: video::FLAG_KEYFRAME,
                epoch: 1,
                display_id: 0,
                frame_id: 7,
                capture_ts_us: rsp_core::clock::unix_micros(),
            },
            1150,
            0.0,
        )
        .expect("video");
        for packet in frame {
            conn.send_datagram(DatagramKind::Video, packet.slice(1..))
                .expect("video send");
        }

        let first = match support::recv_command(&control).await {
            pb::envelope::Msg::KeyframeRequest(request) => request,
            message => panic!("expected KeyframeRequest, got {message:?}"),
        };
        assert_eq!(first.reason, pb::keyframe_request::Reason::Backlog as i32);
        assert_eq!(first.last_good_frame, 7);
        let suppressed = tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                match support::recv(&control).await {
                    pb::envelope::Msg::Ping(ping) => {
                        support::send(
                            &control,
                            pb::Pong {
                                t1_us: ping.t1_us,
                                t2_us: rsp_core::clock::unix_micros(),
                            },
                        )
                        .await;
                    }
                    pb::envelope::Msg::KeyframeRequest(request) => return request,
                    _ => {}
                }
            }
        })
        .await;
        assert!(suppressed.is_err(), "second request was not rate limited");
        let second = match support::recv_command(&control).await {
            pb::envelope::Msg::KeyframeRequest(request) => request,
            message => panic!("expected second KeyframeRequest, got {message:?}"),
        };
        assert_eq!(second.reason, pb::keyframe_request::Reason::Backlog as i32);
        assert_eq!(second.last_good_frame, 7);
        support::finish(&conn, &control).await;
    });
    let (handle, mut events) = Session::open(p).await.expect("open");
    timeout(support::TEST_TIMEOUT, async {
        loop {
            match events.recv().await.expect("session event") {
                SessionEvent::Frame(frame) => {
                    assert_eq!(frame.frame.frame_id, 7);
                    break;
                }
                SessionEvent::Ended { .. } => panic!("session ended before frame"),
                _ => {}
            }
        }
    })
    .await
    .expect("frame deadline");

    handle.request_keyframe_for(pb::keyframe_request::Reason::Backlog);
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.request_keyframe_for(pb::keyframe_request::Reason::Backlog);
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.request_keyframe_for(pb::keyframe_request::Reason::Backlog);
    timeout(support::TEST_TIMEOUT, async {
        while handle.stats().keyframe_requests < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("stats reflect sent requests");
    assert_eq!(handle.stats().keyframe_requests, 2);
    let _ = ended(&mut events).await;
    server.await.expect("script");
}

#[tokio::test]
async fn virtual_display_requests_and_results_use_the_control_stream() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let request = pb::CreateVirtualDisplay {
        request_id: 7,
        width_px: 2732,
        height_px: 2048,
        scale: 2.0,
        refresh_hz: 60,
        select: true,
    };
    let expected = request;
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: rsp_core::features::VIRTUAL_DISPLAY,
                ..Default::default()
            },
        )
        .await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        assert_eq!(
            support::recv_command(&control).await,
            pb::envelope::Msg::CreateVirtualDisplay(expected)
        );
        support::send(
            &control,
            pb::VirtualDisplayResult {
                request_id: 7,
                status: pb::virtual_display_result::Status::Ok as i32,
                display: Some(pb::Display {
                    id: 3,
                    is_virtual: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            support::recv_command(&control).await,
            pb::envelope::Msg::ReleaseVirtualDisplay(pb::ReleaseVirtualDisplay { display_id: 3 })
        );
        support::finish(&conn, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                SessionEvent::StateChanged(SessionStateKind::Streaming)
            ) {
                handle.create_virtual_display(request);
                return;
            }
        }
        panic!("session ended before streaming");
    })
    .await
    .expect("streaming deadline");
    let result = timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if let SessionEvent::VirtualDisplay(result) = event {
                return result;
            }
        }
        panic!("session ended before virtual display result");
    })
    .await
    .expect("virtual display result deadline");
    assert_eq!(result.request_id, 7);
    assert_eq!(result.status, pb::virtual_display_result::Status::Ok as i32);
    let display = result.display.expect("virtual display");
    assert_eq!(display.id, 3);
    assert!(display.is_virtual);
    handle.release_virtual_display(display.id);
    server.await.expect("Host script");
}
