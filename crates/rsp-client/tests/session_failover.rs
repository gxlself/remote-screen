//! Session level failover tests.  The two scripted listeners deliberately use
//! one Host identity, so the dial race can retain both authenticated paths.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rsp_client::session::{Session, SessionEvent, SessionParams, SessionStateKind};
use rsp_core::{
    features,
    hid::{InputMsg, Mods},
    pb, video,
};
use rsp_transport::{
    dial::{Candidate, CandidateSource, PathKind},
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    CloseCode, Conn, ControlStream, DatagramKind, QuicListener,
};
use tokio::sync::{oneshot, Notify};
use tokio::time::{advance, pause, Instant};

const TOKEN_1: [u8; 16] = [0x11; 16];
const TOKEN_2: [u8; 16] = [0x22; 16];

async fn send(control: &ControlStream, msg: impl Into<pb::Envelope>) {
    let bytes = rsp_core::control::encode(&msg.into());
    control.send(&bytes[4..]).await.expect("control send");
}

async fn recv(control: &ControlStream) -> pb::envelope::Msg {
    let bytes = control
        .recv()
        .await
        .expect("control recv")
        .expect("control record");
    pb::Envelope::decode(bytes)
        .expect("protobuf")
        .msg
        .expect("message")
}

async fn bind(identity: &LocalIdentity) -> QuicListener {
    QuicListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        server_config(identity).expect("server config"),
    )
    .await
    .expect("listener")
}

async fn accept(listener: &QuicListener) -> (Conn, ControlStream) {
    let conn = listener
        .accept()
        .await
        .expect("incoming")
        .accept()
        .await
        .expect("TLS");
    let control = conn.accept_control().await.expect("control");
    (conn, control)
}

async fn accept_conn(listener: &QuicListener) -> Conn {
    listener
        .accept()
        .await
        .expect("incoming")
        .accept()
        .await
        .expect("TLS")
}

async fn send_frame(conn: &Conn, epoch: u32, frame_id: u32, keyframe: bool) {
    let packets = video::packetize(
        &[0, 0, 0, 1, 0x26, 1, 4, 5],
        video::FrameMeta {
            flags: if keyframe { video::FLAG_KEYFRAME } else { 0 },
            display_id: 1,
            epoch: epoch as u8,
            frame_id,
            capture_ts_us: rsp_core::clock::unix_micros(),
        },
        1150,
        0.0,
    )
    .expect("packetize");
    for packet in packets {
        conn.send_datagram(DatagramKind::Video, packet.slice(1..))
            .expect("video datagram");
    }
}

async fn without_time_advancing<T>(future: impl Future<Output = T>) -> T {
    // A paused Tokio clock must not auto-advance while we wait for real QUIC
    // I/O. A blocking watchdog still detects a genuinely stalled handshake.
    let (deadline_tx, deadline_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
    let watchdog = tokio::task::spawn_blocking(move || {
        if cancel_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = deadline_tx.send(());
        }
    });
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => {
            let _ = cancel_tx.send(());
            let _ = watchdog.await;
            result
        }
        _ = deadline_rx => {
            let _ = watchdog.await;
            panic!("paused-time I/O deadline");
        }
    }
}

fn candidate(addr: std::net::SocketAddr, kind: PathKind) -> Candidate {
    Candidate {
        addr,
        kind,
        source: CandidateSource::Manual,
    }
}

async fn single_silent_session(
    identity: &LocalIdentity,
) -> (
    rsp_client::SessionHandle,
    rsp_client::EventStream,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = bind(identity).await;
    let addr = listener.local_addr().expect("address");
    let (silent_tx, silent_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let (conn, control) = accept(&listener).await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        send(
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
        let _ = silent_rx.await;
        let _ = conn.closed().await;
    });
    let params = SessionParams::from_candidates(
        vec![candidate(addr, PathKind::Tailnet)],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    let (handle, events) = Session::open(params).await.expect("open");
    (handle, events, silent_tx, host)
}

#[tokio::test]
async fn backup_resumes_after_primary_goes_silent() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let silent = Arc::new(Notify::new());
    let (ready_tx, ready_rx) = oneshot::channel();

    let silent_l1 = Arc::clone(&silent);
    let host1 = tokio::spawn(async move {
        let (conn, control) = accept(&l1).await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        send(
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
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        send_frame(&conn, 1, 0, true).await;
        let _ = ready_tx.send(());
        silent_l1.notified().await;
        // Keep the transport open and send nothing after the signal.
        let _ = conn.closed().await;
    });

    let host2 = tokio::spawn(async move {
        let (conn, control) = accept(&l2).await;
        let hello = recv(&control).await;
        let pb::envelope::Msg::ClientHello(hello) = hello else {
            panic!("expected resume ClientHello");
        };
        assert_eq!(hello.resume_token, TOKEN_1);
        assert_eq!(hello.path, pb::PathKind::Manual as i32);
        assert_eq!(hello.transport, pb::Transport::Quic as i32);
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                resumed: true,
                session_token: TOKEN_2.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        send(
            &control,
            pb::StreamConfig {
                epoch: 2,
                fps: 30,
                width: 640,
                height: 360,
                ..Default::default()
            },
        )
        .await;
        send_frame(&conn, 2, 0, false).await;
        send_frame(&conn, 2, 1, true).await;
        let _ = conn.closed().await;
    });

    let mut params = SessionParams::from_candidates(
        vec![
            Candidate {
                addr: addr1,
                kind: PathKind::Tailnet,
                source: CandidateSource::Manual,
            },
            Candidate {
                addr: addr2,
                kind: PathKind::Manual,
                source: CandidateSource::Manual,
            },
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (handle, mut events) = Session::open(params).await.expect("open");
    without_time_advancing(ready_rx)
        .await
        .expect("initial frame");

    let mut initial_frame = false;
    let mut streaming = false;
    while !(initial_frame && streaming) {
        match without_time_advancing(events.recv()).await.expect("event") {
            SessionEvent::Frame(frame) => initial_frame = frame.frame.keyframe,
            SessionEvent::StateChanged(SessionStateKind::Streaming) => streaming = true,
            _ => {}
        }
    }

    pause();
    let silent_at = Instant::now();
    silent.notify_one();
    let mut reconnecting_at = None;
    let mut resumed_frame = false;
    let mut resumed = false;
    for _ in 0..120 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    reconnecting_at = Some(Instant::now())
                }
                SessionEvent::Frame(frame) => {
                    resumed_frame = frame.frame.keyframe && frame.frame.epoch == 2;
                }
                SessionEvent::StateChanged(SessionStateKind::Streaming) => resumed = true,
                _ => {}
            }
        }
        if resumed && resumed_frame {
            break;
        }
    }
    let reconnecting_at = reconnecting_at.expect("reconnecting");
    assert!(reconnecting_at.duration_since(silent_at) >= Duration::from_millis(1500));
    assert!(reconnecting_at.duration_since(silent_at) <= Duration::from_millis(1560));
    assert!(resumed && resumed_frame);
    assert!(handle.resumed());
    assert_eq!(handle.resume_token(), Some(TOKEN_2.to_vec()));
    handle.close();
    while !matches!(events.recv().await, Some(SessionEvent::Ended { .. })) {}
    tokio::time::resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn no_backup_redials_until_ticket_expiry() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let (handle, mut events, silent, host) = single_silent_session(&identity).await;
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Authenticating)
    ) {}
    // Wait for the scripted host to finish the initial handshake before
    // freezing the clock; the first StateChanged(Streaming) follows it.
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    silent.send(()).expect("silent signal");
    let mut reconnecting = false;
    let mut ended = None;
    for _ in 0..90 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => reconnecting = true,
                SessionEvent::Ended { code, kind, .. } => ended = Some((code, kind)),
                _ => {}
            }
        }
    }
    assert!(reconnecting);
    assert!(
        ended.is_none(),
        "recovery ended before ticket expiry: {ended:?}"
    );

    // Redial keeps retrying until the 60-second recovery ticket expires. The
    // Tokio clock is paused so this advances virtual time without a real wait.
    for _ in 0..61 {
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            if let SessionEvent::Ended { code, kind, .. } = event {
                ended = Some((code, kind));
            }
        }
        if ended.is_some() {
            break;
        }
    }
    assert!(matches!(
        ended,
        Some((
            Some(rsp_core::errors::ErrorCode::Other),
            rsp_client::session::EndReason::Error(rsp_core::errors::ErrorCode::Other)
        ))
    ));
    assert_eq!(handle.state(), SessionStateKind::Ended);
    tokio::time::resume();
    host.await.expect("host");
}

#[tokio::test]
async fn denied_resume_ends_with_e199_and_clears_token() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let silent = Arc::new(Notify::new());
    let host1 = tokio::spawn({
        let silent = Arc::clone(&silent);
        async move {
            let (conn, control) = accept(&l1).await;
            assert!(matches!(
                recv(&control).await,
                pb::envelope::Msg::ClientHello(_)
            ));
            send(
                &control,
                pb::ServerHello {
                    protocol: 1,
                    features: features::RESUME,
                    ..Default::default()
                },
            )
            .await;
            send(
                &control,
                pb::AuthResult {
                    status: pb::auth_result::Status::Ok as i32,
                    role: pb::Role::Controller as i32,
                    session_token: TOKEN_1.to_vec(),
                    ..Default::default()
                },
            )
            .await;
            assert!(matches!(
                recv(&control).await,
                pb::envelope::Msg::StartStream(_)
            ));
            silent.notified().await;
            let _ = conn.closed().await;
        }
    });
    let host2 = tokio::spawn(async move {
        let (_conn, control) = accept(&l2).await;
        let hello = recv(&control).await;
        let pb::envelope::Msg::ClientHello(hello) = hello else {
            panic!("resume hello")
        };
        assert_eq!(hello.resume_token, TOKEN_1);
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                error: Some(pb::ErrorInfo {
                    code: 180,
                    message: "denied".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await;
    });
    let mut params = SessionParams::from_candidates(
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    silent.notify_one();
    let mut error = false;
    let mut ended = None;
    for _ in 0..120 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::Error { code, .. } => {
                    error = code == rsp_core::errors::ErrorCode::Other
                }
                SessionEvent::Ended { code, kind, .. } => {
                    ended = Some((code, kind));
                }
                _ => {}
            }
        }
        if ended.is_some() {
            break;
        }
    }
    assert!(error);
    assert!(matches!(
        ended,
        Some((
            Some(rsp_core::errors::ErrorCode::Other),
            rsp_client::session::EndReason::Error(rsp_core::errors::ErrorCode::Other)
        ))
    ));
    assert_eq!(handle.resume_token(), None);
    tokio::time::resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn backup_attempt_timeout_falls_back_to_redial_then_expires() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let silent = Arc::new(Notify::new());
    let (backup_closed_tx, mut backup_closed_rx) = oneshot::channel();
    let host1 = tokio::spawn({
        let silent = Arc::clone(&silent);
        async move {
            let (conn, control) = accept(&l1).await;
            let _ = recv(&control).await;
            send(
                &control,
                pb::ServerHello {
                    protocol: 1,
                    features: features::RESUME,
                    ..Default::default()
                },
            )
            .await;
            send(
                &control,
                pb::AuthResult {
                    status: pb::auth_result::Status::Ok as i32,
                    role: pb::Role::Controller as i32,
                    session_token: TOKEN_1.to_vec(),
                    ..Default::default()
                },
            )
            .await;
            assert!(matches!(
                recv(&control).await,
                pb::envelope::Msg::StartStream(_)
            ));
            silent.notified().await;
            let _ = conn.closed().await;
        }
    });
    let host2 = tokio::spawn(async move {
        let (conn, control) = accept(&l2).await;
        let hello = recv(&control).await;
        assert!(matches!(hello, pb::envelope::Msg::ClientHello(_)));
        // Keep the control stream open for the full backup attempt timeout.
        let _ = conn.closed().await;
        let _ = backup_closed_tx.send(Instant::now());
    });
    let mut params = SessionParams::from_candidates(
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    silent.notify_one();
    let mut reconnecting = false;
    let mut reconnecting_at = None;
    let mut backup_closed_at = None;
    let mut ended = None;
    for _ in 0..150 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    reconnecting = true;
                    reconnecting_at = Some(Instant::now());
                }
                SessionEvent::Ended { code, kind, .. } => ended = Some((code, kind)),
                _ => {}
            }
        }
        if backup_closed_at.is_none() {
            backup_closed_at = backup_closed_rx.try_recv().ok();
        }
    }
    assert!(reconnecting);
    let reconnecting_at = reconnecting_at.expect("reconnecting timestamp");
    let backup_closed_at = backup_closed_at.expect("backup attempt timed out");
    assert!(backup_closed_at.duration_since(reconnecting_at) >= Duration::from_millis(900));
    assert!(backup_closed_at.duration_since(reconnecting_at) <= Duration::from_millis(1200));
    assert!(
        ended.is_none(),
        "recovery ended before ticket expiry: {ended:?}"
    );

    // The backup attempt has timed out by now. Redial failures continue until
    // the recovery ticket expires; advance the paused clock past that point.
    for _ in 0..60 {
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            if let SessionEvent::Ended { code, kind, .. } = event {
                ended = Some((code, kind));
            }
        }
        if ended.is_some() {
            break;
        }
    }
    assert!(matches!(
        ended,
        Some((
            Some(rsp_core::errors::ErrorCode::Other),
            rsp_client::session::EndReason::Error(rsp_core::errors::ErrorCode::Other)
        ))
    ));
    assert_eq!(handle.state(), SessionStateKind::Ended);
    tokio::time::resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn paused_session_never_declares_dead() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let listener = bind(&identity).await;
    let addr = listener.local_addr().expect("address");
    let (pause_tx, pause_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let (conn, control) = accept(&listener).await;
        let _ = recv(&control).await;
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        let mut pause_tx = Some(pause_tx);
        loop {
            match recv(&control).await {
                pb::envelope::Msg::PauseStream(_) => {
                    let _ = pause_tx.take().map(|tx| tx.send(()));
                }
                pb::envelope::Msg::ResumeStream(_) => {
                    let _ = resume_tx.send(());
                    break;
                }
                _ => {}
            }
        }
        let _ = conn.closed().await;
    });
    let params = SessionParams::from_candidates(
        vec![candidate(addr, PathKind::Tailnet)],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    handle.pause();
    without_time_advancing(pause_rx).await.expect("pause");
    for _ in 0..500 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        assert!(!matches!(
            events.try_recv(),
            Ok(SessionEvent::StateChanged(SessionStateKind::Reconnecting))
        ));
    }
    handle.resume();
    without_time_advancing(resume_rx).await.expect("resume");
    let mut reconnecting = false;
    for _ in 0..90 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            if matches!(
                event,
                SessionEvent::StateChanged(SessionStateKind::Reconnecting)
            ) {
                reconnecting = true;
            }
        }
        if reconnecting {
            break;
        }
    }
    assert!(reconnecting);
    handle.close();
    tokio::time::resume();
    host.await.expect("host");
}

#[tokio::test]
async fn close_code_does_not_trigger_failover() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let host1 = tokio::spawn(async move {
        let (conn, control) = accept(&l1).await;
        let _ = recv(&control).await;
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        conn.close(CloseCode::Revoked, b"revoked");
    });
    let host2 = tokio::spawn(async move {
        let conn = accept_conn(&l2).await;
        let _ = conn.closed().await;
    });
    let mut params = SessionParams::from_candidates(
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let mut reconnecting = false;
    let mut ended = None;
    while ended.is_none() {
        match without_time_advancing(events.recv()).await.expect("event") {
            SessionEvent::StateChanged(SessionStateKind::Reconnecting) => reconnecting = true,
            SessionEvent::Ended { code, kind, .. } => ended = Some((code, kind)),
            _ => {}
        }
    }
    assert!(!reconnecting);
    assert!(matches!(
        ended,
        Some((
            Some(rsp_core::errors::ErrorCode::DeviceRevoked),
            rsp_client::session::EndReason::Close(6)
        ))
    ));
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn backup_closed_when_session_ends() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let host1 = tokio::spawn(async move {
        let (conn, control) = accept(&l1).await;
        let _ = recv(&control).await;
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        let _ = conn.closed().await;
    });
    let host2 = tokio::spawn(async move {
        let conn = accept_conn(&l2).await;
        let _ = conn.closed().await;
    });
    let mut params = SessionParams::from_candidates(
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    handle.close();
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::Ended { .. }
    ) {}
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn datagrams_without_pongs_keep_path_alive() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let listener = bind(&identity).await;
    let addr = listener.local_addr().expect("address");
    let host = tokio::spawn(async move {
        let (conn, control) = accept(&listener).await;
        let _ = recv(&control).await;
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                session_token: TOKEN_1.to_vec(),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                _ = conn.closed() => break,
                _ = interval.tick() => send_frame(&conn, 1, 0, true).await,
            }
        }
    });
    let params = SessionParams::from_candidates(
        vec![candidate(addr, PathKind::Tailnet)],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    let mut reconnecting = false;
    for _ in 0..250 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            if matches!(
                event,
                SessionEvent::StateChanged(SessionStateKind::Reconnecting)
            ) {
                reconnecting = true;
            }
        }
    }
    assert!(!reconnecting);
    tokio::time::resume();
    handle.close();
    while !matches!(events.recv().await, Some(SessionEvent::Ended { .. })) {}
    host.await.expect("host");
}

#[tokio::test]
async fn input_during_switch_is_not_replayed() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let silent = Arc::new(Notify::new());
    let host1 = tokio::spawn({
        let silent = Arc::clone(&silent);
        async move {
            let (conn, control) = accept(&l1).await;
            let _ = recv(&control).await;
            send(
                &control,
                pb::ServerHello {
                    protocol: 1,
                    features: features::RESUME,
                    ..Default::default()
                },
            )
            .await;
            send(
                &control,
                pb::AuthResult {
                    status: pb::auth_result::Status::Ok as i32,
                    role: pb::Role::Controller as i32,
                    session_token: TOKEN_1.to_vec(),
                    ..Default::default()
                },
            )
            .await;
            assert!(matches!(
                recv(&control).await,
                pb::envelope::Msg::StartStream(_)
            ));
            silent.notified().await;
            let _ = conn.closed().await;
        }
    });
    let host2 = tokio::spawn(async move {
        let (conn, control) = accept(&l2).await;
        let _ = recv(&control).await;
        send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RESUME,
                ..Default::default()
            },
        )
        .await;
        send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                resumed: true,
                session_token: TOKEN_2.to_vec(),
                ..Default::default()
            },
        )
        .await;
        let (kind, mut input) = conn.accept_uni().await.expect("input stream");
        assert_eq!(kind, rsp_transport::StreamKind::Input);
        let first = InputMsg::decode(&input.recv_record().await.expect("record").expect("release"))
            .expect("release decode");
        assert_eq!(first, InputMsg::ReleaseAll);
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        let second = InputMsg::decode(&input.recv_record().await.expect("record").expect("key"))
            .expect("key decode");
        assert!(matches!(
            second,
            InputMsg::Key {
                usage: 0x05,
                down: true,
                ..
            }
        ));
    });
    let mut params = SessionParams::from_candidates(
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        without_time_advancing(events.recv()).await.expect("event"),
        SessionEvent::StateChanged(SessionStateKind::Streaming)
    ) {}
    pause();
    silent.notify_one();
    let mut sent_old = false;
    let mut recovered = false;
    for _ in 0..120 {
        advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) if !sent_old => {
                    for _ in 0..3 {
                        handle
                            .send_input(InputMsg::Key {
                                usage: 0x04,
                                down: true,
                                mods: Mods::default(),
                            })
                            .expect("old input");
                    }
                    sent_old = true;
                }
                SessionEvent::StateChanged(SessionStateKind::Streaming) if sent_old => {
                    handle
                        .send_input(InputMsg::Key {
                            usage: 0x05,
                            down: true,
                            mods: Mods::default(),
                        })
                        .expect("new input");
                    recovered = true;
                }
                _ => {}
            }
        }
        if recovered {
            break;
        }
    }
    assert!(recovered);
    handle.close();
    tokio::time::resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
}
