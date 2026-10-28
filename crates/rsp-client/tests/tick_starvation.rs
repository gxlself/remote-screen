//! Keep the real QUIC receive queue nonempty while advancing a paused clock.
#[allow(dead_code)]
mod support;

use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Waker},
    time::Duration,
};

use rsp_client::{
    audio::AudioSink,
    session::{Session, SessionHandle, SessionParams},
};
use rsp_core::{audio, features, pb, video};
use rsp_transport::{
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::{client_config, ServerPin},
    Conn, ConnError, ControlStream, DatagramKind, TransportKind,
};
use tokio::{sync::oneshot, time::Instant};

const QUEUED: u16 = 8;
const PACKETS: u16 = 8_000;
const STEP: Duration = Duration::from_millis(1);
// One 20 ms poll plus a bounded receive batch and the eight queued packets.
const TOLERANCE_MS: u64 = 50;

struct Connected(Conn);
impl Dialer for Connected {
    fn dial<'a>(
        &'a self,
        _: SocketAddr,
        _: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}

#[derive(Default, Debug)]
struct Samples {
    pings: Vec<u64>,
    requests: Vec<u64>,
    losses: Vec<u64>,
}

struct Flood {
    sender: Conn,
    queued: u16,
    packets: u16,
    elapsed_ms: Arc<AtomicU64>,
    receiver: Conn,
    session: Mutex<Option<SessionHandle>>,
    samples: Mutex<Samples>,
    done: Mutex<Option<oneshot::Sender<()>>>,
}

fn send_audio(conn: &Conn, seq: u16) {
    let wire = audio::encode(
        audio::AudioHeader {
            seq,
            capture_ts_us: 0,
        },
        &[0],
    )
    .expect("audio packet");
    conn.send_datagram(DatagramKind::Audio, wire.slice(1..))
        .expect("queue audio");
}

impl AudioSink for Flood {
    fn push(&self, seq: u16, _: u64, _: &[u8], _: u64) {
        self.elapsed_ms.store(u64::from(seq), Ordering::SeqCst);
        let stats = self.session.lock().unwrap().as_ref().unwrap().stats();
        let mut samples = self.samples.lock().unwrap();
        while samples.pings.len() < stats.pings_sent as usize {
            samples.pings.push(u64::from(seq));
        }
        while samples.requests.len() < stats.keyframe_requests as usize {
            samples.requests.push(u64::from(seq));
        }
        while samples.losses.len() < stats.frames_lost as usize {
            samples.losses.push(u64::from(seq));
        }
        drop(samples);

        if seq + self.queued < self.packets {
            let before = self.receiver.as_quic().unwrap().stats().frame_rx.datagram;
            send_audio(&self.sender, seq + self.queued);
            // QUIC drivers run on a separate real-time runtime. Wait for the
            // replacement to arrive BEFORE returning to the session loop:
            // the next datagram is ready even if this task never yields.
            let wall = std::time::Instant::now();
            while self.receiver.as_quic().unwrap().stats().frame_rx.datagram <= before {
                assert!(wall.elapsed() < support::TEST_TIMEOUT, "refill deadline");
                std::thread::yield_now();
            }
        }

        // advance() moves the clock on its first poll, then yields. Poll only
        // that first step so this synchronous sink cannot yield the session
        // task and accidentally make the old biased select fair.
        let before = Instant::now();
        let mut advance = std::pin::pin!(tokio::time::advance(STEP));
        let _ = advance
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        assert_eq!(Instant::now() - before, STEP);
        if seq == self.packets - 1 {
            self.done.lock().unwrap().take().unwrap().send(()).unwrap();
        }
    }

    fn reset(&self) {}
}

fn video_packet(id: u32, len: usize, flags: u8, shard: usize) -> bytes::Bytes {
    video::packetize(
        &vec![0; len],
        video::FrameMeta {
            flags,
            display_id: 1,
            epoch: 1,
            frame_id: id,
            capture_ts_us: 0,
        },
        1150,
        0.0,
    )
    .unwrap()
    .remove(shard)
}

async fn serve(
    control: ControlStream,
    start: oneshot::Receiver<()>,
    elapsed_ms: Arc<AtomicU64>,
) -> Vec<u64> {
    start.await.unwrap();
    assert!(matches!(
        support::recv(&control).await,
        pb::envelope::Msg::ClientHello(_)
    ));
    support::send(
        &control,
        pb::ServerHello {
            protocol: 1,
            features: features::RATE_FEEDBACK,
            ..Default::default()
        },
    )
    .await;
    support::auth(&control, pb::auth_result::Status::Ok).await;
    let mut feedback = Vec::new();
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
                .await
            }
            pb::envelope::Msg::Feedback(_) => feedback.push(elapsed_ms.load(Ordering::SeqCst)),
            pb::envelope::Msg::Goodbye(_) => return feedback,
            pb::envelope::Msg::StartStream(_) | pb::envelope::Msg::KeyframeRequest(_) => {}
            message => panic!("unexpected control: {message:?}"),
        }
    }
}

async fn run_flood(queued: u16, packets: u16, complete_queued_frame: bool) -> (Samples, Vec<u64>) {
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let peer_elapsed_ms = elapsed_ms.clone();
    let (setup_tx, setup_rx) = std::sync::mpsc::channel();
    let (start_tx, start_rx) = oneshot::channel();
    let (report_tx, report_rx) = oneshot::channel();
    let network = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let host = support::Host::bind().await;
                let identity = LocalIdentity::generate_software(CertKind::App).unwrap();
                let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
                let tls =
                    client_config(&identity, ServerPin::Fingerprint(host.fingerprint)).unwrap();
                let (client, server) = tokio::join!(
                    rsp_transport::quic::connect_quic(&endpoint, host.addr, Arc::new(tls), false),
                    async {
                        host.listener
                            .accept()
                            .await
                            .unwrap()
                            .accept()
                            .await
                            .unwrap()
                    },
                );
                let client = client.unwrap();
                for packet in [
                    video_packet(0, 100, video::FLAG_KEYFRAME, 0),
                    video_packet(1, 2244, 0, 0),
                ] {
                    server
                        .send_datagram(DatagramKind::Video, packet.slice(1..))
                        .unwrap();
                }
                for seq in 0..queued {
                    send_audio(&server, seq);
                }
                if complete_queued_frame {
                    let tail = video_packet(1, 2244, 0, 1);
                    server
                        .send_datagram(DatagramKind::Video, tail.slice(1..))
                        .unwrap();
                }
                while client.as_quic().unwrap().stats().frame_rx.datagram
                    < u64::from(queued) + 2 + u64::from(complete_queued_frame)
                {
                    tokio::task::yield_now().await;
                }
                setup_tx
                    .send((
                        host.addr,
                        host.fingerprint,
                        identity,
                        client,
                        server.clone(),
                    ))
                    .unwrap();
                let control = server.accept_control().await.unwrap();
                let feedback = serve(control, start_rx, peer_elapsed_ms).await;
                report_tx.send(feedback).unwrap();
            });
    });
    let (addr, fingerprint, identity, receiver, sender) =
        setup_rx.recv_timeout(support::TEST_TIMEOUT).unwrap();
    let (done_tx, done_rx) = oneshot::channel();
    let flood = Arc::new(Flood {
        sender,
        queued,
        packets,
        elapsed_ms,
        receiver: receiver.clone(),
        session: Mutex::new(None),
        samples: Mutex::new(Samples::default()),
        done: Mutex::new(Some(done_tx)),
    });
    let mut params = SessionParams::single(addr, fingerprint, identity);
    params.dialer = Some(Arc::new(Connected(receiver)));
    params.audio_sink = Some(flood.clone());
    let (session, _events) = Session::open(params).await.unwrap();
    *flood.session.lock().unwrap() = Some(session.clone());
    tokio::time::pause();
    start_tx.send(()).unwrap();
    support::without_time_advancing(done_rx).await.unwrap();
    session.close();
    let feedback = support::without_time_advancing(report_rx).await.unwrap();
    network.join().unwrap();
    if complete_queued_frame {
        assert_eq!(
            session.stats().frames_received,
            2,
            "queued frame must complete"
        );
    }
    let samples = std::mem::take(&mut *flood.samples.lock().unwrap());
    (samples, feedback)
}

#[tokio::test]
async fn continuously_ready_datagrams_do_not_starve_session_timers() {
    let (samples, feedback) = run_flood(QUEUED, PACKETS, false).await;
    eprintln!("8 s continuously ready: {samples:?}; feedback at ms={feedback:?}");
    assert!(samples.pings.len() >= 30, "Ping starved: {samples:?}");
    assert!(samples.pings[0] <= 60 + TOLERANCE_MS);
    for (index, pair) in samples.pings.windows(2).enumerate() {
        let interval = if index < 7 {
            60
        } else if pair[0] < 5_060 {
            200
        } else {
            1_000
        };
        assert!(
            (interval..=interval + TOLERANCE_MS).contains(&(pair[1] - pair[0])),
            "Ping cadence: {pair:?}"
        );
    }
    assert!(feedback.len() >= 32, "feedback starved: {}", feedback.len());
    assert!(feedback[0] <= 200 + TOLERANCE_MS);
    for pair in feedback.windows(2) {
        // A peer can observe a write one packet later; allow 1 ms of
        // observation skew, never a missed 200 ms feedback interval.
        assert!(
            (199..=201 + TOLERANCE_MS).contains(&(pair[1] - pair[0])),
            "feedback cadence: {pair:?}"
        );
    }
    assert!(u64::from(PACKETS) - feedback.last().unwrap() <= 200 + TOLERANCE_MS);
    assert!(u64::from(PACKETS) - samples.pings.last().unwrap() <= 1_000 + TOLERANCE_MS);
    assert_eq!(samples.losses.len(), 1, "missing sparse-frame timeout");
    assert!(
        (50..=50 + TOLERANCE_MS).contains(&samples.losses[0]),
        "late reassembly: {samples:?}"
    );
    assert!(
        samples.requests.len() >= 14,
        "keyframe retry starved: {samples:?}"
    );
    for pair in samples.requests.windows(2) {
        assert!(
            (500..=500 + TOLERANCE_MS).contains(&(pair[1] - pair[0])),
            "keyframe retry cadence: {pair:?}"
        );
    }
}

#[tokio::test]
async fn queued_fragment_beyond_receive_batch_precedes_reassembly_expiry() {
    // The last shard is behind 128 audio packets (128 ms of processing),
    // beyond both the 16-packet yield budget and the 50 ms loss deadline.
    // Replacements keep the queue ready after the original prefix is drained.
    let (samples, _) = run_flood(128, 512, true).await;
    assert!(
        samples.losses.is_empty(),
        "expired an already queued shard: {samples:?}"
    );
    assert!(
        samples.requests.is_empty(),
        "spurious keyframe request: {samples:?}"
    );
    assert!(
        !samples.pings.is_empty(),
        "periodic work must run during the drain"
    );
}
