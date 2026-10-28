//! Session-level redial and backup scripts, with no shared test support.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use prost::Message;
use rsp_client::session::{Session, SessionEvent, SessionParams, SessionStateKind};
use rsp_core::{errors::ErrorCode, features, pb, video};
use rsp_transport::{
    dial::{Candidate, CandidateSource, DefaultDialer, IfaceInfo, PathKind},
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    Conn, ConnError, ControlStream, DatagramKind, QuicListener, TransportKind,
};
use tokio::sync::oneshot;
use tokio::time::{advance, pause, resume, timeout, Instant};

const T1: [u8; 16] = [0x11; 16];
const T2: [u8; 16] = [0x22; 16];
const STEP: Duration = Duration::from_millis(20);

async fn bind(identity: &LocalIdentity) -> QuicListener {
    QuicListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        server_config(identity).expect("server config"),
    )
    .await
    .expect("listener")
}

async fn accept_tls(listener: &QuicListener) -> Conn {
    listener
        .accept()
        .await
        .expect("incoming")
        .accept()
        .await
        .expect("TLS")
}

async fn accept(listener: &QuicListener) -> (Conn, ControlStream) {
    let conn = accept_tls(listener).await;
    let control = conn.accept_control().await.expect("control");
    (conn, control)
}

async fn recv(control: &ControlStream) -> pb::envelope::Msg {
    let bytes = control.recv().await.expect("control read").expect("record");
    pb::Envelope::decode(bytes)
        .expect("protobuf")
        .msg
        .expect("message")
}

async fn send(control: &ControlStream, message: impl Into<pb::Envelope>) {
    control
        .send(&message.into().encode_to_vec())
        .await
        .expect("control write");
}

async fn hello(control: &ControlStream, expected: &[u8]) {
    let pb::envelope::Msg::ClientHello(hello) = recv(control).await else {
        panic!("ClientHello required");
    };
    assert_eq!(hello.resume_token, expected);
    send(
        control,
        pb::ServerHello {
            protocol: features::PROTOCOL_VERSION,
            features: features::RESUME,
            ..Default::default()
        },
    )
    .await;
}

async fn auth(conn: &Conn, control: &ControlStream, token: [u8; 16], resumed: bool) {
    send(
        control,
        pb::AuthResult {
            status: pb::auth_result::Status::Ok as i32,
            role: pb::Role::Controller as i32,
            session_token: token.to_vec(),
            resumed,
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(
        recv(control).await,
        pb::envelope::Msg::StartStream(_)
    ));
    let epoch = if resumed { 2 } else { 1 };
    send(
        control,
        pb::StreamConfig {
            epoch,
            fps: 30,
            width: 640,
            height: 360,
            ..Default::default()
        },
    )
    .await;
    for packet in video::packetize(
        &[0, 0, 0, 1, 0x26, 1, 4, 5],
        video::FrameMeta {
            flags: video::FLAG_KEYFRAME,
            display_id: 0,
            epoch: epoch as u8,
            frame_id: 0,
            capture_ts_us: rsp_core::clock::unix_micros(),
        },
        1150,
        0.0,
    )
    .expect("packetize")
    {
        conn.send_datagram(DatagramKind::Video, packet.slice(1..))
            .expect("keyframe");
    }
}

async fn serve_pings(conn: &Conn, control: &ControlStream) {
    loop {
        tokio::select! {
            _ = conn.closed() => break,
            record = control.recv() => {
                let Ok(Some(bytes)) = record else { break };
                let envelope = pb::Envelope::decode(bytes).expect("protobuf");
                if let Some(pb::envelope::Msg::Ping(ping)) = envelope.msg {
                    send(control, pb::Pong {
                        t1_us: ping.t1_us,
                        t2_us: rsp_core::clock::unix_micros(),
                    }).await;
                }
            }
        }
    }
}

fn candidate(addr: SocketAddr, kind: PathKind) -> Candidate {
    Candidate {
        addr,
        kind,
        source: CandidateSource::Manual,
    }
}

fn params(identity: &LocalIdentity, candidates: Vec<Candidate>) -> SessionParams {
    let mut params = SessionParams::from_candidates(
        candidates,
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    params
}

async fn initial_frame(events: &mut rsp_client::EventStream) {
    timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.expect("initial event") {
                SessionEvent::Frame(frame) if frame.frame.epoch == 1 => break,
                SessionEvent::Ended { kind, .. } => panic!("ended before initial frame: {kind:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("initial frame deadline");
}

async fn step() {
    advance(STEP).await;
    // Give real loopback QUIC I/O a chance to complete without auto-advancing
    // the paused clock through the driver's handshake deadlines.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn without_time_advancing<T>(future: impl Future<Output = T>) -> T {
    let (expired_tx, expired_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
    let watchdog = tokio::task::spawn_blocking(move || {
        if cancel_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = expired_tx.send(());
        }
    });
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => {
            let _ = cancel_tx.send(());
            watchdog.await.expect("watchdog");
            result
        }
        _ = expired_rx => panic!("loopback I/O stalled"),
    }
}

async fn close_promptly(handle: &rsp_client::SessionHandle, events: &mut rsp_client::EventStream) {
    let started = Instant::now();
    handle.close();
    for _ in 0..50 {
        step().await;
        while let Ok(event) = events.try_recv() {
            if matches!(event, SessionEvent::Ended { .. }) {
                assert!(started.elapsed() <= Duration::from_secs(1));
                assert!(events.recv().await.is_none(), "Ended must be last");
                return;
            }
        }
    }
    panic!("close did not produce Ended within one second");
}

struct RecordedDialer {
    inner: DefaultDialer,
    starts: Mutex<Vec<Instant>>,
    refuse: AtomicBool,
}

impl Dialer for RecordedDialer {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async move {
            if kind == TransportKind::Quic {
                self.starts.lock().expect("dial times").push(Instant::now());
            }
            if self.refuse.load(Ordering::Acquire) {
                return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
            }
            self.inner.dial(addr, kind).await
        })
    }
}

fn record_dials(params: &mut SessionParams) -> Arc<RecordedDialer> {
    let endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("client endpoint");
    let dialer = Arc::new(RecordedDialer {
        inner: DefaultDialer::with_net(
            rsp_transport::system_net(),
            Some(endpoint),
            &params.local_identity,
        )
        .expect("dialer"),
        starts: Mutex::new(Vec::new()),
        refuse: AtomicBool::new(false),
    });
    params.dialer = Some(dialer.clone());
    dialer
}

#[tokio::test]
async fn no_backup_redials_and_resumes() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let listener = bind(&identity).await;
    let addr = listener.local_addr().expect("address");
    let host = tokio::spawn(async move {
        let (first, control) = accept(&listener).await;
        hello(&control, &[]).await;
        auth(&first, &control, T1, false).await;
        let (second, control) = accept(&listener).await;
        hello(&control, &T1).await;
        auth(&second, &control, T2, true).await;
        let _ = first.closed().await;
        serve_pings(&second, &control).await;
        let _ = second.closed().await;
    });
    let mut params = params(&identity, vec![candidate(addr, PathKind::Manual)]);
    let dialer = record_dials(&mut params);
    let (handle, mut events) = Session::open(params).await.expect("open");
    initial_frame(&mut events).await;
    pause();
    let silent_at = Instant::now();
    dialer.starts.lock().expect("dial times").clear();
    let mut reconnecting_at = None;
    let mut recovered = false;
    for _ in 0..250 {
        step().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    reconnecting_at = Some(Instant::now())
                }
                SessionEvent::Frame(frame) if frame.frame.epoch == 2 => recovered = true,
                SessionEvent::Ended { kind, .. } => panic!("unexpected ending: {kind:?}"),
                _ => {}
            }
        }
        if recovered {
            break;
        }
    }
    let reconnecting_at = reconnecting_at.expect("Reconnecting");
    assert!(reconnecting_at.duration_since(silent_at) >= Duration::from_millis(1480));
    assert!(reconnecting_at.duration_since(silent_at) <= Duration::from_millis(1560));
    let redial_at = dialer.starts.lock().expect("dial times")[0];
    assert!(redial_at >= reconnecting_at);
    assert!(redial_at.duration_since(reconnecting_at) <= STEP);
    assert!(recovered);
    assert_eq!(handle.state(), SessionStateKind::Streaming);
    assert_eq!(handle.resume_token(), Some(T2.to_vec()));
    close_promptly(&handle, &mut events).await;
    resume();
    host.await.expect("Host");
}

#[tokio::test]
async fn redial_backoff_then_ticket_expiry_is_e199() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let listener = bind(&identity).await;
    let addr = listener.local_addr().expect("address");
    let (drop_tx, drop_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let (conn, control) = accept(&listener).await;
        hello(&control, &[]).await;
        auth(&conn, &control, T1, false).await;
        drop_rx.await.expect("drop listener");
        drop(listener);
        let _ = conn.closed().await;
    });
    let mut params = params(&identity, vec![candidate(addr, PathKind::Manual)]);
    params.dial_options.tcp_after = Duration::ZERO;
    let dialer = record_dials(&mut params);
    let (handle, mut events) = Session::open(params).await.expect("open");
    initial_frame(&mut events).await;
    pause();
    dialer.starts.lock().expect("dial times").clear();
    // Immediate refusals isolate the driver's backoff from transport timeouts.
    dialer.refuse.store(true, Ordering::Release);
    drop_tx.send(()).expect("drop listener");
    let mut lost_at = None;
    let mut ended = None;
    for _ in 0..3200 {
        step().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    lost_at = Some(Instant::now())
                }
                SessionEvent::Ended { code, .. } => ended = Some((Instant::now(), code)),
                _ => {}
            }
        }
        if ended.is_some() {
            break;
        }
    }
    let (ended_at, code) = ended.expect("ticket expiry");
    let since_loss = ended_at.duration_since(lost_at.expect("Reconnecting"));
    assert!(since_loss >= Duration::from_secs(60));
    assert!(since_loss <= Duration::from_millis(60040));
    assert_eq!(code, Some(ErrorCode::Other));
    assert_eq!(handle.resume_token(), None);
    let starts = dialer.starts.lock().expect("dial times").clone();
    assert!(starts.len() >= 8, "redial starts: {starts:?}");
    for (index, pair) in starts.windows(2).enumerate() {
        let minimum = Duration::from_secs([1, 2, 4, 8][index.min(3)]);
        assert!(
            pair[1].duration_since(pair[0]) >= minimum,
            "attempt {} retried too early: {:?}",
            index + 2,
            pair[1].duration_since(pair[0])
        );
    }
    resume();
    host.await.expect("Host");
}

#[tokio::test]
async fn backup_attempt_failure_falls_back_to_redial() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let (attempt_tx, mut attempt_rx) = oneshot::channel();
    let (closed_tx, mut closed_rx) = oneshot::channel();
    let host1 = tokio::spawn(async move {
        let (first, control) = accept(&l1).await;
        hello(&control, &[]).await;
        auth(&first, &control, T1, false).await;
        let (second, control) = accept(&l1).await;
        hello(&control, &T1).await;
        auth(&second, &control, T2, true).await;
        let _ = first.closed().await;
        serve_pings(&second, &control).await;
        let _ = second.closed().await;
    });
    let host2 = tokio::spawn(async move {
        let (conn, control) = accept(&l2).await;
        // The listener is no longer dialable; the already-held backup stays
        // open and answers ServerHello, deliberately withholding AuthResult.
        drop(l2);
        hello(&control, &T1).await;
        let _ = attempt_tx.send(Instant::now());
        let _ = conn.closed().await;
        let _ = closed_tx.send(Instant::now());
    });
    let params = params(
        &identity,
        vec![
            candidate(addr1, PathKind::Tailnet),
            candidate(addr2, PathKind::Manual),
        ],
    );
    let (handle, mut events) = Session::open(params).await.expect("open");
    initial_frame(&mut events).await;
    pause();
    let mut attempted_at = None;
    let mut closed_at = None;
    let mut reconnecting = false;
    let mut recovered = false;
    for _ in 0..350 {
        step().await;
        if attempted_at.is_none() {
            attempted_at = attempt_rx.try_recv().ok();
        }
        if closed_at.is_none() {
            closed_at = closed_rx.try_recv().ok();
        }
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => reconnecting = true,
                SessionEvent::Frame(frame) if frame.frame.epoch == 2 => recovered = true,
                SessionEvent::Ended { kind, .. } => panic!("unexpected ending: {kind:?}"),
                _ => {}
            }
        }
        if recovered && closed_at.is_some() {
            break;
        }
    }
    let attempt_time = closed_at
        .expect("L2 closed")
        .duration_since(attempted_at.expect("L2 used"));
    assert!(attempt_time >= Duration::from_millis(900));
    assert!(attempt_time <= Duration::from_millis(1100));
    assert!(reconnecting && recovered);
    assert_eq!(handle.resume_token(), Some(T2.to_vec()));
    close_promptly(&handle, &mut events).await;
    resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
}

#[tokio::test]
async fn backup_is_replenished_after_use() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let l1 = bind(&identity).await;
    let l2 = bind(&identity).await;
    let l3 = bind(&identity).await;
    let addr1 = l1.local_addr().expect("L1 address");
    let addr2 = l2.local_addr().expect("L2 address");
    let addr3 = l3.local_addr().expect("L3 address");
    let (replacement_tx, mut replacement_rx) = oneshot::channel();
    let (no_control_tx, mut no_control_rx) = oneshot::channel();
    let host1 = tokio::spawn(async move {
        let (first, control) = accept(&l1).await;
        hello(&control, &[]).await;
        auth(&first, &control, T1, false).await;
        let conn = accept_tls(&l1).await;
        let _ = replacement_tx.send(Instant::now());
        let no_control = timeout(Duration::from_secs(1), conn.accept_control())
            .await
            .is_err();
        let _ = no_control_tx.send(no_control);
        let _ = first.closed().await;
        let _ = conn.closed().await;
    });
    let host2 = tokio::spawn(async move {
        let (conn, control) = accept(&l2).await;
        hello(&control, &T1).await;
        auth(&conn, &control, T2, true).await;
        serve_pings(&conn, &control).await;
        let _ = conn.closed().await;
    });
    let host3 = tokio::spawn(async move {
        // Both the initial race and replenishment discard the third winner.
        for _ in 0..2 {
            let conn = accept_tls(&l3).await;
            assert!(conn.accept_control().await.is_err());
            let _ = conn.closed().await;
        }
    });
    let mut params = params(
        &identity,
        vec![
            candidate(addr1, PathKind::Lan),
            candidate(addr2, PathKind::Tailnet),
            candidate(addr3, PathKind::Manual),
        ],
    );
    params.local_net.interfaces.push(IfaceInfo {
        name: "loopback".into(),
        addrs: vec!["127.0.0.0/8".parse().expect("loopback subnet")],
        is_vpn: false,
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    initial_frame(&mut events).await;
    pause();
    let mut recovered_at = None;
    let mut replacement_at = None;
    let mut no_control = None;
    for _ in 0..450 {
        step().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::Frame(frame) if frame.frame.epoch == 2 => {
                    recovered_at = Some(Instant::now())
                }
                SessionEvent::Ended { kind, .. } => panic!("unexpected ending: {kind:?}"),
                _ => {}
            }
        }
        if replacement_at.is_none() {
            replacement_at = replacement_rx.try_recv().ok();
        }
        if no_control.is_none() {
            no_control = no_control_rx.try_recv().ok();
        }
        if no_control.is_some() {
            break;
        }
    }
    let recovered_at = recovered_at.expect("L2 resumed with keyframe");
    let replacement_at = replacement_at.expect("different path backup dialed");
    assert!(replacement_at >= recovered_at);
    assert!(replacement_at.duration_since(recovered_at) <= Duration::from_secs(5));
    assert_eq!(no_control, Some(true));
    assert_eq!(handle.resume_token(), Some(T2.to_vec()));
    close_promptly(&handle, &mut events).await;
    resume();
    host1.await.expect("L1");
    host2.await.expect("L2");
    host3.await.expect("L3");
}

#[tokio::test]
async fn close_during_redial_ends_promptly() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("identity");
    let listener = bind(&identity).await;
    let addr = listener.local_addr().expect("address");
    let (redial_tx, mut redial_rx) = oneshot::channel();
    let (closed_tx, mut closed_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let (first, control) = accept(&listener).await;
        hello(&control, &[]).await;
        auth(&first, &control, T1, false).await;
        let second = accept_tls(&listener).await;
        let _ = redial_tx.send(());
        // Close while race still owns this TLS connection during its grace
        // period, before any ClientHello/control stream has been opened.
        assert!(second.accept_control().await.is_err());
        let _ = first.closed().await;
        let _ = second.closed().await;
        let _ = closed_tx.send(());
    });
    let params = params(&identity, vec![candidate(addr, PathKind::Manual)]);
    let (handle, mut events) = Session::open(params).await.expect("open");
    initial_frame(&mut events).await;
    pause();
    let mut reconnecting = false;
    for _ in 0..100 {
        step().await;
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
    without_time_advancing(&mut redial_rx)
        .await
        .expect("redial TLS established");
    close_promptly(&handle, &mut events).await;
    for _ in 0..10 {
        if closed_rx.try_recv().is_ok() {
            resume();
            host.await.expect("Host");
            return;
        }
        step().await;
    }
    panic!("script still has an open connection after close");
}
