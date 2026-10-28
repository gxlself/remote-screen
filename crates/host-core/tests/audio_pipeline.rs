use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use host_core::pipeline::audio::{
    video_encoder_kbps, AudioHub, AudioSender, AudioSubscription, EncodedAudio, AUDIO_CHUNK_QUEUE,
    AUDIO_FANOUT, AUDIO_MIN_VIDEO_KBPS, AUDIO_RESERVE_KBPS,
};
use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureHandle, PlatformError,
};
use rs_audio::codec::{AudioDecoder, EncoderConfig};
use rs_audio::gen::{sample_ts_us, Signal};
use rs_audio::{FRAME_LEN, FRAME_SAMPLES};
use rsp_core::audio::{self, AudioHeader};
use rsp_core::ratectl::Decision;
use rsp_transport::{connect_quic, Conn, ControlStream, DatagramKind, QuicListener};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout, timeout_at, Instant};

const BASE_TS_US: u64 = 1_700_000_000_000_000;
const TONE: Signal = Signal::Sine {
    freq_hz: 440,
    amp: 0.5,
};

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("audio test timed out")
}

fn check_config(cfg: AudioCaptureConfig) {
    assert_eq!(cfg.sample_rate, 48_000);
    assert_eq!(cfg.channels, 2);
    assert!(cfg.exclude_self);
}

struct TestCapture {
    task: JoinHandle<()>,
    dropped: Arc<AtomicBool>,
}

impl CaptureHandle for TestCapture {}

impl Drop for TestCapture {
    fn drop(&mut self) {
        self.task.abort();
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct TestSource {
    starts: AtomicUsize,
    dropped: Arc<AtomicBool>,
}

impl TestSource {
    fn start(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>, PlatformError> {
        check_config(cfg);
        assert_eq!(sink.max_capacity(), AUDIO_CHUNK_QUEUE);
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.dropped.store(false, Ordering::SeqCst);
        let task = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(10));
            let mut available = 0;
            let mut index = 0;
            loop {
                ticks.tick().await;
                // Poll every 10 ms, but preserve 48 kHz sample throughput:
                // a 1024-frame platform block spans 21 1/3 ms, not 10 ms.
                available += FRAME_SAMPLES;
                if available < 1024 {
                    continue;
                }
                available -= 1024;
                let mut samples = vec![0.0; 1024 * 2];
                TONE.render_index(index, 2, &mut samples);
                let chunk = AudioChunk {
                    capture_ts_us: BASE_TS_US + sample_ts_us(index),
                    channels: 2,
                    frames: 1024,
                    samples,
                };
                index += 1024;
                if sink.try_send(chunk).is_err() {
                    break;
                }
            }
        });
        Ok(Box::new(TestCapture {
            task,
            dropped: Arc::clone(&self.dropped),
        }))
    }
}

struct ManualCapture {
    _sink: AudioChunkSink,
    dropped: Arc<AtomicBool>,
}

impl CaptureHandle for ManualCapture {}

impl Drop for ManualCapture {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct Feed {
    sink: AudioChunkSink,
    index: u64,
    dropped: Arc<AtomicBool>,
}

impl Feed {
    async fn push(&mut self, signal: Signal) {
        let mut samples = vec![0.0; FRAME_LEN];
        signal.render_index(self.index, 2, &mut samples);
        self.sink
            .send(AudioChunk {
                capture_ts_us: BASE_TS_US + sample_ts_us(self.index),
                channels: 2,
                frames: FRAME_SAMPLES as u32,
                samples,
            })
            .await
            .unwrap();
        self.index += FRAME_SAMPLES as u64;
    }
}

fn manual_subscription(hub: &AudioHub) -> (AudioSubscription, Feed) {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut input = None;
    let sub = hub
        .subscribe_with(|cfg, sink| {
            check_config(cfg);
            input = Some(sink.clone());
            Ok(Box::new(ManualCapture {
                _sink: sink,
                dropped: Arc::clone(&dropped),
            }))
        })
        .unwrap();
    (
        sub,
        Feed {
            sink: input.unwrap(),
            index: 0,
            dropped,
        },
    )
}

fn decode_frame(decoder: &mut AudioDecoder, frame: &EncodedAudio) {
    assert!(!frame.payload.is_empty());
    let mut pcm = [0.0; FRAME_LEN];
    assert_eq!(decoder.decode(&frame.payload, &mut pcm).unwrap(), 480);
}

#[tokio::test]
async fn one_second_of_platform_blocks_is_one_hundred_opus_frames() {
    let source = TestSource::default();
    let hub = AudioHub::new(EncoderConfig::default());
    let mut sub = hub
        .subscribe_with(|cfg, sink| source.start(cfg, sink))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut decoder = AudioDecoder::new().unwrap();
    let mut count = 0u64;
    while let Ok(frame) = timeout_at(deadline, sub.recv()).await {
        let frame = frame.unwrap();
        assert_eq!(frame.capture_ts_us, BASE_TS_US + count * 10_000);
        assert!(!frame.after_gap);
        decode_frame(&mut decoder, &frame);
        count += 1;
    }
    assert!((95..=105).contains(&count), "received {count} packets");
    assert_eq!(sub.lagged(), 0);
}

#[tokio::test]
async fn subscribers_share_capture_and_payloads_and_last_drop_stops_capture() {
    let source = TestSource::default();
    let hub = AudioHub::new(EncoderConfig::default());
    let mut first = hub
        .subscribe_with(|cfg, sink| source.start(cfg, sink))
        .unwrap();
    let mut second = hub
        .subscribe_with(|cfg, sink| source.start(cfg, sink))
        .unwrap();
    assert_eq!(source.starts.load(Ordering::SeqCst), 1);
    assert_eq!(hub.subscribers(), 2);
    assert!(hub.running());
    for _ in 0..20 {
        let a = bounded(first.recv()).await.unwrap();
        let b = bounded(second.recv()).await.unwrap();
        assert_eq!(a.capture_ts_us, b.capture_ts_us);
        assert_eq!(a.payload, b.payload);
        assert_eq!(a.after_gap, b.after_gap);
    }
    drop(first);
    assert_eq!(hub.subscribers(), 1);
    assert!(hub.running());
    assert!(!source.dropped.load(Ordering::SeqCst));
    let stopped_at = Instant::now();
    drop(second);
    assert!(stopped_at.elapsed() < Duration::from_millis(200));
    assert!(source.dropped.load(Ordering::SeqCst));
    assert_eq!(hub.subscribers(), 0);
    assert!(!hub.running());

    let mut restarted = hub
        .subscribe_with(|cfg, sink| source.start(cfg, sink))
        .unwrap();
    assert_eq!(source.starts.load(Ordering::SeqCst), 2);
    let first = bounded(restarted.recv()).await.unwrap();
    assert_eq!(first.capture_ts_us, BASE_TS_US);
    assert!(!first.after_gap);
    decode_frame(&mut AudioDecoder::new().unwrap(), &first);
}

#[tokio::test]
async fn startup_errors_are_preserved_and_allow_retry() {
    let hub = AudioHub::new(EncoderConfig::default());
    let result =
        hub.subscribe_with(|_, _| Err(PlatformError::PermissionDenied("system audio".into())));
    assert!(
        matches!(result, Err(PlatformError::PermissionDenied(message)) if message == "system audio")
    );
    assert!(!hub.running());
    assert_eq!(hub.subscribers(), 0);
    let (_sub, _feed) = manual_subscription(&hub);
    assert!(hub.running());
    assert_eq!(hub.subscribers(), 1);
}

#[tokio::test]
async fn capture_source_ending_closes_the_subscription() {
    let hub = AudioHub::new(EncoderConfig::default());
    let dropped = Arc::new(AtomicBool::new(false));
    let mut sub = hub
        .subscribe_with(|_, sink| {
            let task = tokio::spawn(async move {
                let mut samples = vec![0.0; FRAME_LEN];
                TONE.render_index(0, 2, &mut samples);
                sink.send(AudioChunk {
                    capture_ts_us: BASE_TS_US,
                    channels: 2,
                    frames: 480,
                    samples,
                })
                .await
                .unwrap();
            });
            Ok(Box::new(TestCapture {
                task,
                dropped: Arc::clone(&dropped),
            }))
        })
        .unwrap();
    decode_frame(
        &mut AudioDecoder::new().unwrap(),
        &bounded(sub.recv()).await.unwrap(),
    );
    assert!(bounded(sub.recv()).await.is_none());
    assert!(!hub.running());
    drop(sub);
    assert_eq!(hub.subscribers(), 0);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn concurrent_subscriptions_only_start_one_capture() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AudioHub>();
    let hub = Arc::new(AudioHub::new(EncoderConfig::default()));
    let starts = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(Barrier::new(9));
    let subscribed = Arc::new(Barrier::new(9));
    let release = Arc::new(Barrier::new(9));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let hub = Arc::clone(&hub);
            let starts = Arc::clone(&starts);
            let dropped = Arc::clone(&dropped);
            let ready = Arc::clone(&ready);
            let subscribed = Arc::clone(&subscribed);
            let release = Arc::clone(&release);
            std::thread::spawn(move || {
                ready.wait();
                let sub = hub
                    .subscribe_with(|cfg, sink| {
                        check_config(cfg);
                        starts.fetch_add(1, Ordering::SeqCst);
                        Ok(Box::new(ManualCapture {
                            _sink: sink,
                            dropped,
                        }))
                    })
                    .unwrap();
                subscribed.wait();
                release.wait();
                drop(sub);
            })
        })
        .collect();
    ready.wait();
    subscribed.wait();
    let count = hub.subscribers();
    let start_count = starts.load(Ordering::SeqCst);
    release.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(count, 8);
    assert_eq!(start_count, 1);
    assert!(!hub.running());
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn silence_stops_after_two_hundred_ms_and_resume_resets_the_encoder() {
    let hub = AudioHub::new(EncoderConfig::default());
    let (mut sub, mut feed) = manual_subscription(&hub);
    let mut decoder = AudioDecoder::new().unwrap();
    for _ in 0..3 {
        feed.push(TONE).await;
        let frame = bounded(sub.recv()).await.unwrap();
        assert!(!frame.after_gap);
        decode_frame(&mut decoder, &frame);
    }
    // The first 20 quiet frames are sent; the remaining 800 ms are suppressed.
    for quiet in 0..20 {
        feed.push(Signal::Silence).await;
        let frame = bounded(sub.recv()).await.unwrap();
        assert_eq!(frame.capture_ts_us, BASE_TS_US + (3 + quiet) * 10_000);
        assert!(!frame.after_gap);
        decode_frame(&mut decoder, &frame);
    }
    for _ in 20..100 {
        feed.push(Signal::Silence).await;
    }
    assert!(timeout(Duration::from_millis(50), sub.recv())
        .await
        .is_err());
    feed.push(TONE).await;
    let resumed = bounded(sub.recv()).await.unwrap();
    assert_eq!(resumed.capture_ts_us, BASE_TS_US + 1_030_000);
    assert!(resumed.after_gap);
    decode_frame(&mut decoder, &resumed);
    // A fresh decoder can also consume the reset encoder's first packet.
    decode_frame(&mut AudioDecoder::new().unwrap(), &resumed);
    feed.push(TONE).await;
    let following = bounded(sub.recv()).await.unwrap();
    assert!(!following.after_gap);
    assert_eq!(following.capture_ts_us, resumed.capture_ts_us + 10_000);
}

#[tokio::test]
async fn slow_subscription_skips_overwritten_packets_and_counts_lag() {
    let source = TestSource::default();
    let hub = AudioHub::new(EncoderConfig::default());
    let mut sub = hub
        .subscribe_with(|cfg, sink| source.start(cfg, sink))
        .unwrap();
    sleep(Duration::from_millis(300)).await;
    let frame = bounded(sub.recv()).await.unwrap();
    assert!(sub.lagged() > 0);
    assert_eq!(frame.capture_ts_us, BASE_TS_US + sub.lagged() * 10_000);
    decode_frame(&mut AudioDecoder::new().unwrap(), &frame);
}

struct Pair {
    client: Conn,
    server: Conn,
    _controls: (ControlStream, ControlStream),
    _listener: QuicListener,
    _endpoint: quinn::Endpoint,
}

async fn pair() -> Pair {
    // As in rsp-transport/tests/conn.rs: loopback port 0, mutual TLS, rsp/1,
    // and a control-stream bootstrap before exchanging media datagrams.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rcgen::generate_simple_self_signed(vec!["rs-host".into()]).unwrap();
    let client = rcgen::generate_simple_self_signed(vec!["rs-client".into()]).unwrap();
    let mut server_roots = RootCertStore::empty();
    server_roots.add(server.cert.der().clone()).unwrap();
    let mut client_roots = RootCertStore::empty();
    client_roots.add(client.cert.der().clone()).unwrap();
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(client_roots),
        Arc::clone(&provider),
    )
    .build()
    .unwrap();
    let mut server_cfg = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.cert.der().clone()],
            PrivatePkcs8KeyDer::from(server.signing_key.serialize_der()).into(),
        )
        .unwrap();
    let mut client_cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(server_roots)
        .with_client_auth_cert(
            vec![client.cert.der().clone()],
            PrivatePkcs8KeyDer::from(client.signing_key.serialize_der()).into(),
        )
        .unwrap();
    server_cfg.alpn_protocols = vec![b"rsp/1".to_vec()];
    client_cfg.alpn_protocols = vec![b"rsp/1".to_vec()];
    let listener = QuicListener::bind("127.0.0.1:0".parse().unwrap(), server_cfg)
        .await
        .unwrap();
    let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    let (client, server) = bounded(async {
        tokio::join!(
            connect_quic(
                &endpoint,
                listener.local_addr().unwrap(),
                Arc::new(client_cfg),
                false,
            ),
            async { listener.accept().await.unwrap().accept().await }
        )
    })
    .await;
    let client = client.unwrap();
    let server = server.unwrap();
    let client_control = client.open_control().await.unwrap();
    client_control.send(b"bootstrap").await.unwrap();
    let server_control = bounded(server.accept_control()).await.unwrap();
    assert_eq!(
        bounded(server_control.recv()).await.unwrap().unwrap(),
        b"bootstrap"[..]
    );
    Pair {
        client,
        server,
        _controls: (client_control, server_control),
        _listener: listener,
        _endpoint: endpoint,
    }
}

async fn receive_audio(conn: &Conn) -> AudioHeader {
    let (kind, payload) = bounded(conn.recv_datagram()).await.unwrap();
    assert_eq!(kind, DatagramKind::Audio);
    let mut datagram = vec![audio::AUDIO_TYPE];
    datagram.extend_from_slice(&payload);
    let (header, opus) = audio::decode(&datagram).unwrap();
    assert_eq!(
        AudioDecoder::new()
            .unwrap()
            .decode(opus, &mut [0.0; FRAME_LEN])
            .unwrap(),
        480
    );
    header
}

#[tokio::test]
async fn sender_wraps_sequences_and_drop_stops_datagrams() {
    let pair = pair().await;
    let hub = AudioHub::new(EncoderConfig::default());
    let (sub, mut feed) = manual_subscription(&hub);
    let sender = AudioSender::spawn(pair.client.clone(), sub, 65534);
    for (index, seq) in [65534, 65535, 0, 1, 2].into_iter().enumerate() {
        feed.push(TONE).await;
        let header = receive_audio(&pair.server).await;
        assert_eq!(header.seq, seq);
        assert_eq!(header.capture_ts_us, BASE_TS_US + index as u64 * 10_000);
    }
    assert_eq!(sender.next_seq(), 3);
    assert_eq!(sender.stats().sent, 5);
    assert_eq!(sender.stats().dropped_full, 0);
    assert_eq!(sender.stats().dropped_too_large, 0);
    assert_eq!(sender.stats().lagged, 0);
    let next_seq = sender.next_seq();
    drop(sender);
    bounded(async {
        while hub.running() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert!(feed.dropped.load(Ordering::SeqCst));
    // Continue feeding the detached encoder: its cancelled sender must not
    // send even when the capture channel has not closed yet.
    for _ in 0..5 {
        feed.push(TONE).await;
    }
    assert!(
        timeout(Duration::from_millis(100), pair.server.recv_datagram())
            .await
            .is_err()
    );
    drop(feed);

    let (sub, mut feed) = manual_subscription(&hub);
    let restarted = AudioSender::spawn(pair.client.clone(), sub, next_seq);
    feed.push(TONE).await;
    assert_eq!(receive_audio(&pair.server).await.seq, 3);
    assert_eq!(restarted.next_seq(), 4);
}

#[tokio::test]
async fn sender_preserves_sequence_during_silence() {
    let pair = pair().await;
    let hub = AudioHub::new(EncoderConfig::default());
    let (sub, mut feed) = manual_subscription(&hub);
    let sender = AudioSender::spawn(pair.client.clone(), sub, 100);
    feed.push(TONE).await;
    assert_eq!(receive_audio(&pair.server).await.seq, 100);
    for seq in 101..121 {
        feed.push(Signal::Silence).await;
        assert_eq!(receive_audio(&pair.server).await.seq, seq);
    }
    for _ in 20..100 {
        feed.push(Signal::Silence).await;
    }
    assert!(
        timeout(Duration::from_millis(50), pair.server.recv_datagram())
            .await
            .is_err()
    );
    assert_eq!(sender.next_seq(), 121);
    feed.push(TONE).await;
    let resumed = receive_audio(&pair.server).await;
    assert_eq!(resumed.seq, 121);
    assert_eq!(resumed.capture_ts_us, BASE_TS_US + 1_010_000);
    assert_eq!(sender.next_seq(), 122);
}

#[tokio::test]
async fn closed_connection_ends_sender_after_advancing_sequence_for_attempt() {
    let pair = pair().await;
    let hub = AudioHub::new(EncoderConfig::default());
    let (sub, mut feed) = manual_subscription(&hub);
    let sender = AudioSender::spawn(pair.client.clone(), sub, 7);
    pair.client.close(rsp_transport::CloseCode::Normal, b"done");
    feed.push(TONE).await;
    bounded(async {
        while hub.running() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert_eq!(sender.next_seq(), 8);
    assert_eq!(sender.stats().sent, 0);
    assert!(feed.dropped.load(Ordering::SeqCst));
}

fn decision(target_kbps: u32, encoder_kbps: u32, fec_ratio: f32) -> Decision {
    Decision {
        target_kbps,
        encoder_kbps,
        fec_ratio,
        fps: 60,
        scale: 1.0,
        level: 0,
        config_changed: false,
        rate_update: None,
    }
}

#[test]
fn video_bitrate_reserves_audio_before_fec_and_respects_bounds() {
    assert_eq!(AUDIO_RESERVE_KBPS, 128);
    assert_eq!(AUDIO_CHUNK_QUEUE, 32);
    assert_eq!(AUDIO_FANOUT, 16);
    assert_eq!(AUDIO_MIN_VIDEO_KBPS, 500);
    let d = decision(10_000, 9_090, 0.1);
    assert!(video_encoder_kbps(&d, true).abs_diff(8_974) <= 1);
    assert_eq!(video_encoder_kbps(&d, false), 9_090);
    assert_eq!(video_encoder_kbps(&decision(600, 600, 0.0), true), 500);
    assert_eq!(video_encoder_kbps(&decision(100, 100, 0.0), true), 100);
    assert_eq!(video_encoder_kbps(&decision(128, 700, 0.0), true), 500);
    assert_eq!(video_encoder_kbps(&decision(0, 0, 0.0), true), 0);
    assert_eq!(video_encoder_kbps(&decision(10_000, 400, 0.1), true), 400);
    assert_eq!(
        video_encoder_kbps(&decision(10_000, 10_000, 0.0), true),
        9_872
    );
}
