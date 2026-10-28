//! TCP-to-QUIC upgrade of a live session . The scripted Host keeps
//! one identity on TCP port P and later binds QUIC on UDP port P.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prost::Message;
use rsp_client::session::{Session, SessionEvent, SessionParams, SessionStateKind};
use rsp_client::{EventStream, SessionHandle};
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::{features, pb, video};
use rsp_transport::{
    dial::DefaultDialer,
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    CloseCode, Conn, ConnError, ControlStream, DatagramKind, QuicListener, StreamKind, TcpListener,
    TransportKind,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{advance, pause, resume, timeout, Instant};

const T1: [u8; 16] = [0x11; 16];
const T2: [u8; 16] = [0x22; 16];
const STEP: Duration = Duration::from_millis(20);

#[derive(Debug)]
enum Note {
    Paused,
    Replaced,
    QuicAccepted,
    QuicHello {
        transport: i32,
        token: Vec<u8>,
        at: Instant,
    },
    QuicClosed(Instant),
    StartStream,
    Input(InputMsg),
}

async fn send(control: &ControlStream, message: impl Into<pb::Envelope>) {
    control
        .send(&message.into().encode_to_vec())
        .await
        .expect("control write");
}

async fn recv(control: &ControlStream) -> pb::envelope::Msg {
    let bytes = control.recv().await.expect("control read").expect("record");
    pb::Envelope::decode(bytes)
        .expect("protobuf")
        .msg
        .expect("message")
}

fn server_hello(features: u64) -> pb::ServerHello {
    pb::ServerHello {
        protocol: features::PROTOCOL_VERSION,
        features,
        ..Default::default()
    }
}

fn auth_ok(token: [u8; 16], resumed: bool) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        role: pb::Role::Controller as i32,
        session_token: token.to_vec(),
        resumed,
        ..Default::default()
    }
}

fn stream_config(epoch: u32) -> pb::StreamConfig {
    pb::StreamConfig {
        epoch,
        fps: 30,
        width: 640,
        height: 360,
        ..Default::default()
    }
}

fn send_frame(conn: &Conn, epoch: u32, frame_id: u32) {
    let meta = video::FrameMeta {
        flags: video::FLAG_KEYFRAME,
        display_id: 0,
        epoch: epoch as u8,
        frame_id,
        capture_ts_us: rsp_core::clock::unix_micros(),
    };
    let data = [0, 0, 0, 1, 0x26, 1, 4, 5];
    let packets = match conn.kind() {
        TransportKind::Tcp => video::packetize_tcp(&data, meta),
        TransportKind::Quic => video::packetize(&data, meta, 1150, 0.0),
    }
    .expect("packetize");
    for packet in packets {
        conn.send_datagram(DatagramKind::Video, packet.slice(1..))
            .expect("video");
    }
}

fn pong(ping: pb::Ping) -> pb::Pong {
    pb::Pong {
        t1_us: ping.t1_us,
        t2_us: rsp_core::clock::unix_micros(),
    }
}

/// The original TCP session. `replace` makes it act as a Host that has
/// resumed the session elsewhere: `Goodbye(REPLACED)`, then close code 3.
async fn tcp_host(
    listener: TcpListener,
    frames: bool,
    mut replace: Option<oneshot::Receiver<oneshot::Sender<()>>>,
    notes: mpsc::UnboundedSender<Note>,
) {
    let conn = listener
        .accept()
        .await
        .expect("incoming")
        .accept()
        .await
        .expect("TLS")
        .expect("main connection");
    let control = conn.accept_control().await.expect("control");
    let pb::envelope::Msg::ClientHello(hello) = recv(&control).await else {
        panic!("ClientHello required");
    };
    assert_eq!(hello.transport, pb::Transport::Tcp as i32);
    assert!(hello.resume_token.is_empty());
    send(
        &control,
        server_hello(features::RESUME | features::TCP_TRANSPORT),
    )
    .await;
    send(&control, auth_ok(T1, false)).await;
    assert!(matches!(
        recv(&control).await,
        pb::envelope::Msg::StartStream(_)
    ));
    send(&control, stream_config(1)).await;
    let mut frame_id = 0;
    send_frame(&conn, 1, frame_id);
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            ack = async {
                match replace.as_mut() {
                    Some(rx) => rx.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                replace = None;
                let Some(ack) = ack else { continue };
                send(&control, pb::Goodbye {
                    reason: pb::goodbye::Reason::Replaced as i32,
                    message: "replaced by a new connection".into(),
                }).await;
                let _ = control.flush().await;
                conn.close(CloseCode::Replaced, b"session replaced");
                let _ = notes.send(Note::Replaced);
                let _ = ack.send(());
                return;
            }
            _ = ticker.tick(), if frames => {
                frame_id += 1;
                send_frame(&conn, 1, frame_id);
            }
            record = control.recv() => {
                let Ok(Some(bytes)) = record else { return };
                match pb::Envelope::decode(bytes).expect("protobuf").msg {
                    Some(pb::envelope::Msg::Ping(ping)) => send(&control, pong(ping)).await,
                    Some(pb::envelope::Msg::PauseStream(_)) => {
                        let _ = notes.send(Note::Paused);
                    }
                    Some(pb::envelope::Msg::Goodbye(_)) => {
                        conn.close(CloseCode::Normal, b"TCP script finished");
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn bind_quic(identity: &LocalIdentity, addr: SocketAddr) -> QuicListener {
    QuicListener::bind(addr, server_config(identity).expect("server config"))
        .await
        .expect("QUIC on the TCP port")
}

async fn accept_quic(listener: &QuicListener) -> Conn {
    listener
        .accept()
        .await
        .expect("incoming")
        .accept()
        .await
        .expect("TLS")
}

async fn quic_hello(conn: &Conn, notes: &mpsc::UnboundedSender<Note>) -> ControlStream {
    let control = conn.accept_control().await.expect("candidate control");
    let pb::envelope::Msg::ClientHello(hello) = recv(&control).await else {
        panic!("ClientHello required");
    };
    let at = Instant::now();
    send(&control, server_hello(features::RESUME)).await;
    let _ = notes.send(Note::QuicHello {
        transport: hello.transport,
        token: hello.resume_token,
        at,
    });
    control
}

async fn serve_quic(conn: &Conn, control: &ControlStream) {
    loop {
        tokio::select! {
            _ = conn.closed() => return,
            record = control.recv() => {
                let Ok(Some(bytes)) = record else { return };
                match pb::Envelope::decode(bytes).expect("protobuf").msg {
                    Some(pb::envelope::Msg::Ping(ping)) => send(control, pong(ping)).await,
                    Some(pb::envelope::Msg::Goodbye(_)) => {
                        conn.close(CloseCode::Normal, b"QUIC script finished");
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}

struct RecordedDialer {
    inner: DefaultDialer,
    quic_starts: Mutex<Vec<Instant>>,
}

impl Dialer for RecordedDialer {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async move {
            if kind == TransportKind::Quic {
                self.quic_starts.lock().expect("dials").push(Instant::now());
            }
            self.inner.dial(addr, kind).await
        })
    }
}

impl RecordedDialer {
    fn starts(&self) -> Vec<Instant> {
        self.quic_starts.lock().expect("dials").clone()
    }
}

fn params(identity: &LocalIdentity, addr: SocketAddr) -> (SessionParams, Arc<RecordedDialer>) {
    let mut params = SessionParams::single(
        addr,
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    let endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("client endpoint");
    let dialer = Arc::new(RecordedDialer {
        inner: DefaultDialer::with_net(
            rsp_transport::system_net(),
            Some(endpoint),
            &params.local_identity,
        )
        .expect("dialer"),
        quic_starts: Mutex::new(Vec::new()),
    });
    params.dialer = Some(dialer.clone());
    (params, dialer)
}

struct Host {
    identity: LocalIdentity,
    addr: SocketAddr,
}

/// Binds TCP only; UDP on the same port stays closed until `bind_quic`.
async fn bind_host() -> (Host, TcpListener) {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("Host identity");
    let listener = TcpListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        server_config(&identity).expect("server config"),
    )
    .await
    .expect("TCP listener");
    let addr = listener.local_addr().expect("address");
    (Host { identity, addr }, listener)
}

/// Real-time setup: the QUIC attempt gets no answer and TCP wins at 250 ms.
async fn tcp_streaming(events: &mut EventStream) {
    timeout(Duration::from_secs(10), async {
        let mut frame = false;
        loop {
            match events.recv().await.expect("event") {
                SessionEvent::Frame(received) if received.frame.epoch == 1 => frame = true,
                SessionEvent::Stats(stats)
                    if frame
                        && stats
                            .hud
                            .is_some_and(|hud| hud.transport == pb::Transport::Tcp) =>
                {
                    break
                }
                SessionEvent::Ended { kind, .. } => panic!("ended during setup: {kind:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("TCP streaming with HUD");
}

async fn step() {
    advance(STEP).await;
    // Give real loopback I/O a chance to complete without auto-advancing
    // the paused clock through the session's deadlines.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[derive(Default)]
struct Seen {
    epoch1_frames: u32,
    epoch2_frames: u32,
}

/// Fails on any ending or Reconnecting: the upgrade must stay invisible.
fn drain(events: &mut EventStream, seen: &mut Seen) {
    while let Ok(event) = events.try_recv() {
        match event {
            SessionEvent::Ended { kind, .. } => panic!("session ended: {kind:?}"),
            SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                panic!("session entered Reconnecting")
            }
            SessionEvent::Frame(received) if received.frame.epoch == 1 => seen.epoch1_frames += 1,
            SessionEvent::Frame(received) if received.frame.epoch == 2 => seen.epoch2_frames += 1,
            _ => {}
        }
    }
}

async fn close_session(handle: &SessionHandle, events: &mut EventStream) {
    handle.close();
    for _ in 0..100 {
        step().await;
        while let Ok(event) = events.try_recv() {
            if let SessionEvent::Ended { kind, .. } = event {
                assert_eq!(
                    kind,
                    rsp_client::session::EndReason::Goodbye(pb::goodbye::Reason::UserClosed)
                );
                return;
            }
        }
    }
    panic!("close did not end the session");
}

async fn join(task: tokio::task::JoinHandle<()>) {
    timeout(Duration::from_secs(10), task)
        .await
        .expect("script finished")
        .expect("script");
}

#[tokio::test]
async fn tcp_session_upgrades_to_quic_with_resume() {
    let (host, listener) = bind_host().await;
    let (notes_tx, mut notes) = mpsc::unbounded_channel();
    let (replace_tx, replace_rx) = oneshot::channel();
    let tcp = tokio::spawn(tcp_host(
        listener,
        false,
        Some(replace_rx),
        notes_tx.clone(),
    ));
    let (params, _dialer) = params(&host.identity, host.addr);
    let (handle, mut events) = Session::open(params).await.expect("open");
    tcp_streaming(&mut events).await;
    assert_eq!(handle.resume_token(), Some(T1.to_vec()));
    pause();
    let quic_listener = bind_quic(&host.identity, host.addr).await;
    let quic = tokio::spawn(async move {
        let conn = accept_quic(&quic_listener).await;
        let control = quic_hello(&conn, &notes_tx).await;
        let (ack_tx, ack_rx) = oneshot::channel();
        replace_tx.send(ack_tx).expect("replace TCP");
        ack_rx.await.expect("TCP replaced");
        // The client sees the replacement before AuthResult arrives.
        tokio::time::sleep(Duration::from_millis(200)).await;
        send(&control, auth_ok(T2, true)).await;
        assert!(matches!(
            recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        let _ = notes_tx.send(Note::StartStream);
        send(&control, stream_config(2)).await;
        send_frame(&conn, 2, 0);
        let input_conn = conn.clone();
        let input_notes = notes_tx.clone();
        let input = tokio::spawn(async move {
            let (kind, mut stream) = input_conn.accept_uni().await.expect("input stream");
            assert_eq!(kind, StreamKind::Input);
            let record = stream
                .recv_record()
                .await
                .expect("input read")
                .expect("input record");
            let _ = input_notes.send(Note::Input(InputMsg::decode(&record).expect("input")));
        });
        serve_quic(&conn, &control).await;
        input.abort();
    });

    let mut seen = Seen::default();
    let mut hello = None;
    let mut replaced = false;
    let mut start_stream = false;
    for _ in 0..2500 {
        step().await;
        drain(&mut events, &mut seen);
        while let Ok(note) = notes.try_recv() {
            match note {
                Note::QuicHello {
                    transport, token, ..
                } => hello = Some((transport, token)),
                Note::Replaced => replaced = true,
                Note::StartStream => start_stream = true,
                other => panic!("unexpected script note: {other:?}"),
            }
        }
        if start_stream && seen.epoch2_frames > 0 {
            break;
        }
    }
    let (transport, token) = hello.expect("QUIC resume ClientHello");
    assert_eq!(transport, pb::Transport::Quic as i32);
    assert_eq!(token, T1.to_vec());
    assert!(replaced, "old TCP connection replaced");
    assert!(start_stream, "StartStream on QUIC");
    assert!(seen.epoch2_frames > 0, "epoch 2 keyframe from QUIC");
    for _ in 0..100 {
        if handle.stats().hud.map(|hud| hud.transport) == Some(pb::Transport::Quic) {
            break;
        }
        step().await;
        drain(&mut events, &mut seen);
    }
    assert_eq!(
        handle.stats().hud.map(|hud| hud.transport),
        Some(pb::Transport::Quic)
    );
    assert_eq!(handle.state(), SessionStateKind::Streaming);
    assert_eq!(handle.resume_token(), Some(T2.to_vec()));
    assert!(handle.resumed());

    let key = InputMsg::Key {
        usage: 0x04,
        down: true,
        mods: Mods::NONE,
    };
    handle.send_input(key.clone()).expect("input encode");
    let mut input = None;
    for _ in 0..100 {
        step().await;
        drain(&mut events, &mut seen);
        if let Ok(note) = notes.try_recv() {
            match note {
                Note::Input(message) => {
                    input = Some(message);
                    break;
                }
                other => panic!("unexpected script note: {other:?}"),
            }
        }
    }
    assert_eq!(input, Some(key));
    close_session(&handle, &mut events).await;
    resume();
    join(tcp).await;
    join(quic).await;
}

#[tokio::test]
async fn probe_failure_keeps_tcp_and_retries() {
    let (host, listener) = bind_host().await;
    let (notes_tx, mut notes) = mpsc::unbounded_channel();
    let tcp = tokio::spawn(tcp_host(listener, true, None, notes_tx.clone()));
    let (params, dialer) = params(&host.identity, host.addr);
    let (handle, mut events) = Session::open(params).await.expect("open");
    tcp_streaming(&mut events).await;
    pause();
    assert_eq!(dialer.starts().len(), 1, "initial race dials QUIC once");
    let quic_listener = bind_quic(&host.identity, host.addr).await;
    let quic = tokio::spawn(async move {
        for _ in 0..2 {
            let conn = accept_quic(&quic_listener).await;
            let _ = notes_tx.send(Note::QuicAccepted);
            // Handshake and ServerHello, but never an AuthResult.
            let _control = quic_hello(&conn, &notes_tx).await;
            let _ = conn.closed().await;
            let _ = notes_tx.send(Note::QuicClosed(Instant::now()));
        }
    });

    let mut seen = Seen::default();
    let mut hello_at = None;
    let mut closed_at = None;
    let mut frames_while_resuming = 0;
    let mut accepted = 0;
    let mut hellos = 0;
    for _ in 0..4000 {
        step().await;
        let before = seen.epoch1_frames;
        drain(&mut events, &mut seen);
        if hello_at.is_some() && closed_at.is_none() {
            frames_while_resuming += seen.epoch1_frames - before;
        }
        while let Ok(note) = notes.try_recv() {
            match note {
                Note::QuicAccepted => accepted += 1,
                Note::QuicHello { token, at, .. } => {
                    assert_eq!(token, T1.to_vec());
                    hello_at.get_or_insert(at);
                    hellos += 1;
                }
                Note::QuicClosed(at) if closed_at.is_none() => closed_at = Some(at),
                other => panic!("unexpected script note: {other:?}"),
            }
        }
        if accepted == 2 && hellos == 2 {
            break;
        }
    }
    let hello_at = hello_at.expect("first resume ClientHello");
    let closed_at = closed_at.expect("candidate closed without AuthResult");
    let waited = closed_at.duration_since(hello_at);
    assert!(
        waited >= Duration::from_millis(2900) && waited <= Duration::from_millis(3100),
        "candidate closed after {waited:?}"
    );
    assert!(
        frames_while_resuming >= 20,
        "TCP frames while resuming: {frames_while_resuming}"
    );
    let starts = dialer.starts();
    assert_eq!(starts.len(), 3, "QUIC dials: {starts:?}");
    let retry = starts[2].duration_since(starts[1]);
    assert!(
        retry >= Duration::from_secs(30) && retry <= Duration::from_secs(30) + 5 * STEP,
        "second probe after {retry:?}"
    );
    assert_eq!(handle.state(), SessionStateKind::Streaming);
    assert_eq!(handle.resume_token(), Some(T1.to_vec()));
    close_session(&handle, &mut events).await;
    resume();
    join(tcp).await;
    join(quic).await;
}

#[tokio::test]
async fn background_pause_blocks_upgrade() {
    let (host, listener) = bind_host().await;
    let (notes_tx, mut notes) = mpsc::unbounded_channel();
    let tcp = tokio::spawn(tcp_host(listener, false, None, notes_tx.clone()));
    let (params, dialer) = params(&host.identity, host.addr);
    let (handle, mut events) = Session::open(params).await.expect("open");
    tcp_streaming(&mut events).await;
    pause();
    let quic_listener = bind_quic(&host.identity, host.addr).await;
    let quic = tokio::spawn(async move {
        let conn = accept_quic(&quic_listener).await;
        let _ = notes_tx.send(Note::QuicAccepted);
        let _ = conn.closed().await;
    });

    handle.pause();
    let mut paused = false;
    for _ in 0..50 {
        step().await;
        drain(&mut events, &mut Seen::default());
        if let Ok(note) = notes.try_recv() {
            assert!(matches!(note, Note::Paused), "{note:?}");
            paused = true;
            break;
        }
    }
    assert!(paused, "Host received PauseStream");
    let paused_at = Instant::now();
    while Instant::now().duration_since(paused_at) < Duration::from_secs(60) {
        step().await;
        drain(&mut events, &mut Seen::default());
        if let Ok(note) = notes.try_recv() {
            panic!("QUIC activity while paused: {note:?}");
        }
    }
    assert_eq!(dialer.starts().len(), 1, "no QUIC probe while paused");

    let resumed_at = Instant::now();
    handle.resume();
    let mut accepted = false;
    for _ in 0..100 {
        step().await;
        drain(&mut events, &mut Seen::default());
        if let Ok(note) = notes.try_recv() {
            assert!(matches!(note, Note::QuicAccepted), "{note:?}");
            accepted = true;
            break;
        }
    }
    assert!(accepted, "probe after resume");
    let starts = dialer.starts();
    assert_eq!(starts.len(), 2, "QUIC dials: {starts:?}");
    assert!(starts[1] >= resumed_at);
    assert!(starts[1].duration_since(resumed_at) <= 5 * STEP);
    close_session(&handle, &mut events).await;
    resume();
    join(tcp).await;
    join(quic).await;
}

#[tokio::test]
async fn replaced_primary_without_commit_fails_over_after_resume_timeout() {
    let (host, listener) = bind_host().await;
    let (notes_tx, mut notes) = mpsc::unbounded_channel();
    let (replace_tx, replace_rx) = oneshot::channel();
    let tcp = tokio::spawn(tcp_host(
        listener,
        false,
        Some(replace_rx),
        notes_tx.clone(),
    ));
    let (params, _dialer) = params(&host.identity, host.addr);
    let (handle, mut events) = Session::open(params).await.expect("open");
    tcp_streaming(&mut events).await;
    pause();
    let quic_listener = bind_quic(&host.identity, host.addr).await;
    let quic = tokio::spawn(async move {
        let conn = accept_quic(&quic_listener).await;
        // Replace the TCP primary as a resuming Host would, but lose the
        // AuthResult.
        let _control = quic_hello(&conn, &notes_tx).await;
        let (ack_tx, ack_rx) = oneshot::channel();
        replace_tx.send(ack_tx).expect("replace TCP");
        ack_rx.await.expect("TCP replaced");
        let _ = conn.closed().await;
        let _ = notes_tx.send(Note::QuicClosed(Instant::now()));
    });

    let mut hello_at = None;
    let mut closed_at = None;
    let mut reconnecting_at = None;
    for _ in 0..2500 {
        step().await;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::Ended { kind, .. } => panic!("session ended: {kind:?}"),
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    reconnecting_at.get_or_insert(Instant::now());
                }
                _ => {}
            }
        }
        while let Ok(note) = notes.try_recv() {
            match note {
                Note::QuicHello { at, .. } => hello_at = Some(at),
                Note::QuicClosed(at) => closed_at = Some(at),
                Note::Replaced => {}
                other => panic!("unexpected script note: {other:?}"),
            }
        }
        if reconnecting_at.is_some() && closed_at.is_some() {
            break;
        }
    }
    let hello_at = hello_at.expect("resume ClientHello");
    let reconnecting = reconnecting_at
        .expect("replaced primary handed to failover")
        .duration_since(hello_at);
    assert!(
        reconnecting >= Duration::from_millis(2900) && reconnecting <= Duration::from_millis(3100),
        "Reconnecting after {reconnecting:?}"
    );
    let closed = closed_at
        .expect("candidate closed")
        .duration_since(hello_at);
    assert!(
        closed <= Duration::from_millis(3100),
        "candidate closed after {closed:?}"
    );
    handle.close();
    let mut ended = None;
    for _ in 0..100 {
        step().await;
        while let Ok(event) = events.try_recv() {
            if let SessionEvent::Ended { kind, .. } = event {
                ended = Some(kind);
            }
        }
        if ended.is_some() {
            break;
        }
    }
    assert_eq!(
        ended,
        Some(rsp_client::session::EndReason::Goodbye(
            pb::goodbye::Reason::UserClosed
        ))
    );
    resume();
    join(tcp).await;
    join(quic).await;
}
