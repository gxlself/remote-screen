#[allow(dead_code)]
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use rsp_client::audio::AudioSink;
use rsp_client::session::{Session, SessionHandle, SessionParams, SessionStateKind};
use rsp_client::{EventStream, SessionEvent};
use rsp_core::{audio, features, pb, video};
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::{Conn, ControlStream, DatagramKind};
use tokio::sync::Notify;
use tokio::time::timeout;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sample {
    seq: u16,
    capture_ts_us: u64,
    payload: Vec<u8>,
    recv_us: u64,
}

#[derive(Default)]
struct RecordingSink {
    samples: Mutex<Vec<Sample>>,
    resets: AtomicUsize,
    changed: Notify,
}

impl AudioSink for RecordingSink {
    fn push(&self, seq: u16, capture_ts_us: u64, payload: &[u8], recv_us: u64) {
        self.samples.lock().expect("samples").push(Sample {
            seq,
            capture_ts_us,
            payload: payload.to_vec(),
            recv_us,
        });
        self.changed.notify_one();
    }

    fn reset(&self) {
        self.samples.lock().expect("samples").clear();
        self.resets.fetch_add(1, Ordering::SeqCst);
    }
}

impl RecordingSink {
    async fn wait_samples(&self, count: usize) -> Vec<Sample> {
        timeout(support::TEST_TIMEOUT, async {
            loop {
                let changed = self.changed.notified();
                {
                    let samples = self.samples.lock().expect("samples");
                    if samples.len() >= count {
                        return samples.clone();
                    }
                }
                changed.await;
            }
        })
        .await
        .expect("audio delivery deadline")
    }
}

struct TestSession {
    conn: Conn,
    control: ControlStream,
    handle: SessionHandle,
    events: EventStream,
    hello: pb::ClientHello,
}

impl TestSession {
    async fn new(
        conn: Conn,
        control: ControlStream,
        handle: SessionHandle,
        events: EventStream,
    ) -> Self {
        let pb::envelope::Msg::ClientHello(hello) = support::recv(&control).await else {
            panic!("expected ClientHello");
        };
        Self {
            conn,
            control,
            handle,
            events,
            hello,
        }
    }

    async fn start(&mut self, features: u64) {
        support::send(
            &self.control,
            pb::ServerHello {
                protocol: features::PROTOCOL_VERSION,
                features,
                ..Default::default()
            },
        )
        .await;
        support::auth(&self.control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&self.control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        loop {
            if matches!(
                self.next_event().await,
                SessionEvent::StateChanged(SessionStateKind::Streaming)
            ) {
                break;
            }
        }
    }

    async fn next_event(&mut self) -> SessionEvent {
        let event = timeout(support::TEST_TIMEOUT, self.events.recv())
            .await
            .expect("session event deadline")
            .expect("session event");
        assert!(
            !matches!(event, SessionEvent::Ended { .. }),
            "session ended unexpectedly: {event:?}"
        );
        event
    }

    async fn finish(&mut self) {
        support::finish(&self.conn, &self.control).await;
        timeout(support::TEST_TIMEOUT, async {
            loop {
                if let SessionEvent::Ended { code, .. } =
                    self.events.recv().await.expect("session end")
                {
                    assert_eq!(code, None);
                    break;
                }
            }
        })
        .await
        .expect("session end deadline");
    }
}

async fn open_quic(
    host: &support::Host,
    audio: bool,
    sink: Option<Arc<RecordingSink>>,
) -> TestSession {
    let mut params = SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    params.audio = audio;
    params.audio_sink = sink.map(|sink| sink as Arc<dyn AudioSink>);
    // Even an explicitly requested feature bit must be masked without a sink.
    params.features |= features::AUDIO_OPUS;
    let (client, (conn, control)) = tokio::join!(Session::open(params), host.accept());
    let (handle, events) = client.expect("open session");
    TestSession::new(conn, control, handle, events).await
}

async fn check_hello(requested: bool, capable: bool) {
    let host = support::Host::bind().await;
    let sink = capable.then(|| Arc::new(RecordingSink::default()));
    let mut session = open_quic(&host, requested, sink).await;
    assert_eq!(session.hello.features & features::AUDIO_OPUS != 0, capable);
    assert_eq!(
        session.hello.caps.as_ref().expect("ClientCaps").audio_opus,
        requested && capable
    );
    session.start(0).await;
    session.finish().await;
}

#[tokio::test]
async fn requested_audio_with_sink_advertises_feature_and_capability() {
    check_hello(true, true).await;
}

#[tokio::test]
async fn disabled_audio_with_sink_advertises_feature_without_requesting_audio() {
    check_hello(false, true).await;
}

#[tokio::test]
async fn requested_audio_without_sink_advertises_neither_feature_nor_capability() {
    check_hello(true, false).await;
}

#[tokio::test]
async fn default_audio_without_sink_advertises_neither_feature_nor_capability() {
    check_hello(false, false).await;
}

fn send_audio(conn: &Conn, seq: u16) {
    let wire = audio::encode(
        audio::AudioHeader {
            seq,
            capture_ts_us: 1_000_000 + u64::from(seq) * 10_000,
        },
        &[0xf8, seq as u8, 0xff],
    )
    .expect("audio encode");
    conn.send_datagram(DatagramKind::Audio, wire.slice(1..))
        .expect("audio send");
}

fn assert_samples(samples: &[Sample]) {
    assert_eq!(samples.len(), 5);
    for (sample, seq) in samples.iter().zip(10..=14) {
        assert_eq!(sample.seq, seq);
        assert_eq!(sample.capture_ts_us, 1_000_000 + u64::from(seq) * 10_000);
        assert_eq!(sample.payload, [0xf8, seq as u8, 0xff]);
        assert_ne!(sample.recv_us, 0);
    }
    assert!(samples
        .windows(2)
        .all(|pair| pair[0].recv_us <= pair[1].recv_us));
}

#[tokio::test]
async fn quic_delivers_five_audio_datagrams_in_order() {
    let host = support::Host::bind().await;
    let sink = Arc::new(RecordingSink::default());
    let mut session = open_quic(&host, true, Some(Arc::clone(&sink))).await;
    session.start(features::AUDIO_OPUS).await;
    for seq in 10..=14 {
        send_audio(&session.conn, seq);
    }
    assert_samples(&sink.wait_samples(5).await);
    assert_eq!(session.handle.stats().frames_received, 0);
    session.finish().await;
}

fn send_video(conn: &Conn, frame_id: u32) {
    let packets = video::packetize(
        &[0, 0, 0, 1, 0x26, frame_id as u8],
        video::FrameMeta {
            flags: video::FLAG_KEYFRAME,
            display_id: 1,
            epoch: 1,
            frame_id,
            capture_ts_us: rsp_core::clock::unix_micros(),
        },
        1200,
        0.0,
    )
    .expect("video encode");
    for packet in packets {
        conn.send_datagram(DatagramKind::Video, packet.slice(1..))
            .expect("video send");
    }
}

async fn wait_frames(session: &mut TestSession, count: u32) {
    for frame_id in 0..count {
        loop {
            if let SessionEvent::Frame(frame) = session.next_event().await {
                assert_eq!(frame.frame.frame_id, frame_id);
                assert_eq!(
                    frame.frame.data.as_ref(),
                    [0, 0, 0, 1, 0x26, frame_id as u8]
                );
                break;
            }
        }
    }
    assert_eq!(session.handle.stats().frames_received, u64::from(count));
    assert_eq!(session.handle.stats().frames_lost, 0);
}

// Observe video parse failures without adding a production statistics field.
#[derive(Default)]
struct InvalidDatagramLogs {
    video: AtomicUsize,
    audio: AtomicUsize,
}

impl tracing::Subscriber for InvalidDatagramLogs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Visitor<'a>(&'a InvalidDatagramLogs);
        impl tracing::field::Visit for Visitor<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    match format!("{value:?}").as_str() {
                        "invalid video datagram" => &self.0.video,
                        "invalid audio datagram" => &self.0.audio,
                        _ => return,
                    }
                    .fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        event.record(&mut Visitor(self));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn audio_between_video_datagrams_preserves_every_video_frame() {
    let logs = Arc::new(InvalidDatagramLogs::default());
    let _subscriber = tracing::subscriber::set_default(Arc::clone(&logs));
    let host = support::Host::bind().await;
    let sink = Arc::new(RecordingSink::default());
    let mut session = open_quic(&host, true, Some(Arc::clone(&sink))).await;
    session.start(features::AUDIO_OPUS).await;
    for frame_id in 0..6 {
        send_video(&session.conn, frame_id);
        if frame_id < 5 {
            send_audio(&session.conn, 10 + frame_id as u16);
        }
    }
    wait_frames(&mut session, 6).await;
    assert_samples(&sink.wait_samples(5).await);
    session.finish().await;
    assert_eq!(logs.video.load(Ordering::SeqCst), 0);
    assert_eq!(logs.audio.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_audio_is_dropped_and_streaming_continues() {
    let logs = Arc::new(InvalidDatagramLogs::default());
    let _subscriber = tracing::subscriber::set_default(Arc::clone(&logs));
    let host = support::Host::bind().await;
    let sink = Arc::new(RecordingSink::default());
    let mut session = open_quic(&host, true, Some(Arc::clone(&sink))).await;
    session.start(features::AUDIO_OPUS).await;
    // The mux adds 0x11, making a complete but truncated five-byte datagram.
    session
        .conn
        .send_datagram(DatagramKind::Audio, Bytes::from_static(&[0; 4]))
        .expect("bad audio send");
    send_video(&session.conn, 0);
    wait_frames(&mut session, 1).await;
    assert!(sink.samples.lock().expect("samples").is_empty());
    for seq in 10..=14 {
        send_audio(&session.conn, seq);
    }
    assert_samples(&sink.wait_samples(5).await);
    assert_eq!(session.handle.state(), SessionStateKind::Streaming);
    session.finish().await;
    assert_eq!(logs.video.load(Ordering::SeqCst), 0);
    assert_eq!(logs.audio.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn audio_without_sink_is_discarded_before_decoding() {
    let logs = Arc::new(InvalidDatagramLogs::default());
    let _subscriber = tracing::subscriber::set_default(Arc::clone(&logs));
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, false, None).await;
    session.start(features::AUDIO_OPUS).await;
    send_audio(&session.conn, 10);
    session
        .conn
        .send_datagram(DatagramKind::Audio, Bytes::from_static(&[0; 4]))
        .expect("bad audio send");
    send_video(&session.conn, 0);
    wait_frames(&mut session, 1).await;
    session.finish().await;
    assert_eq!(logs.video.load(Ordering::SeqCst), 0);
    assert_eq!(logs.audio.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn set_audio_sends_both_values_when_host_advertises_audio() {
    let host = support::Host::bind().await;
    let sink = Arc::new(RecordingSink::default());
    let mut session = open_quic(&host, true, Some(sink)).await;
    session.start(features::AUDIO_OPUS).await;
    for enabled in [false, true] {
        session.handle.set_audio(enabled);
        assert!(matches!(
            support::recv_command(&session.control).await,
            pb::envelope::Msg::SetAudio(value) if value.enabled == enabled
        ));
    }
    session.finish().await;
    session.handle.set_audio(true);
    session.handle.set_audio(false);
}

#[tokio::test]
async fn set_audio_is_silent_for_hosts_without_audio() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, false, Some(Arc::new(RecordingSink::default()))).await;
    session.start(0).await;
    session.handle.set_audio(true);
    assert!(timeout(Duration::from_millis(500), async {
        loop {
            assert!(!matches!(
                support::recv_command(&session.control).await,
                pb::envelope::Msg::SetAudio(_)
            ));
        }
    })
    .await
    .is_err());
    assert_eq!(session.handle.state(), SessionStateKind::Streaming);
    session.finish().await;
}

#[tokio::test]
async fn set_audio_before_authentication_is_silently_ignored() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, false, Some(Arc::new(RecordingSink::default()))).await;
    session.handle.set_audio(true);
    // Wait for a control round trip while the Runner is still authenticating.
    support::send(&session.control, pb::Ping { t1_us: 1 }).await;
    assert!(matches!(
        support::recv_command(&session.control).await,
        pb::envelope::Msg::Pong(_)
    ));
    session.start(features::AUDIO_OPUS).await;
    session.handle.pause();
    assert!(matches!(
        support::recv_command(&session.control).await,
        pb::envelope::Msg::PauseStream(_)
    ));
    session.finish().await;
}

#[tokio::test]
async fn sink_resets_once_per_connection_and_not_on_pause_or_resume() {
    let host = support::Host::bind().await;
    let sink = Arc::new(RecordingSink::default());
    for expected_resets in 1..=2 {
        let mut session = open_quic(&host, true, Some(Arc::clone(&sink))).await;
        session.start(features::AUDIO_OPUS).await;
        assert_eq!(sink.resets.load(Ordering::SeqCst), expected_resets);
        assert!(sink.samples.lock().expect("samples").is_empty());
        send_audio(&session.conn, 10);
        sink.wait_samples(1).await;
        session.handle.pause();
        assert!(matches!(
            support::recv_command(&session.control).await,
            pb::envelope::Msg::PauseStream(_)
        ));
        session.handle.resume();
        assert!(matches!(
            support::recv_command(&session.control).await,
            pb::envelope::Msg::ResumeStream(_)
        ));
        send_audio(&session.conn, 11);
        let samples = sink.wait_samples(2).await;
        assert_eq!(
            samples.iter().map(|sample| sample.seq).collect::<Vec<_>>(),
            [10, 11]
        );
        assert_eq!(sink.resets.load(Ordering::SeqCst), expected_resets);
        session.finish().await;
    }
    assert_eq!(sink.resets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn tcp_delivers_five_audio_records_in_order() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("Host identity");
    let listener = rsp_transport::TcpListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        rsp_transport::tls::server_config(&identity).expect("TLS"),
    )
    .await
    .expect("TCP listener");
    let sink = Arc::new(RecordingSink::default());
    let mut params = SessionParams::single(
        listener.local_addr().expect("TCP address"),
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    params.audio = true;
    params.audio_sink = Some(sink.clone());
    let (client, (conn, control)) = tokio::join!(Session::open(params), async {
        let conn = timeout(support::TEST_TIMEOUT, async {
            listener
                .accept()
                .await
                .expect("incoming")
                .accept()
                .await
                .expect("TLS")
                .expect("main connection")
        })
        .await
        .expect("TCP accept deadline");
        let control = conn.accept_control().await.expect("control");
        (conn, control)
    });
    let (handle, events) = client.expect("TCP fallback");
    let mut session = TestSession::new(conn, control, handle, events).await;
    assert_eq!(session.hello.transport, pb::Transport::Tcp as i32);
    assert_ne!(session.hello.features & features::AUDIO_OPUS, 0);
    assert!(session.hello.caps.as_ref().expect("caps").audio_opus);
    session
        .start(features::TCP_TRANSPORT | features::AUDIO_OPUS)
        .await;
    for seq in 10..=14 {
        send_audio(&session.conn, seq);
    }
    assert_samples(&sink.wait_samples(5).await);
    session.finish().await;
}
