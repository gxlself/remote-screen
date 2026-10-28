//! A keyframe burst must fit the §5.3 deadline even across a 200 ms RTT.
//!
//! Run the real loopback QUIC endpoints with a manually advanced Tokio clock.
//! A loaded runner can deschedule all endpoints for 80--100 ms at a time;
//! counting those pauses as network RTT/pacing makes a 50 ms flight take whole
//! ACK rounds in wall time. Advancing only after draining runnable work keeps
//! the 133 ms assertion about the transport, including its real Quinn pacer.
//! The loss test and the deliberately delayed application flight still advance this
//! same clock: network delay and congestion are never subtracted from results.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use rsp_core::{
    clock::unix_micros,
    video::{self, FrameMeta, Reassembler, ReassemblyEvent, FLAG_KEYFRAME},
};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, DatagramKind, QuicListener,
};
use tokio::{net::UdpSocket, task::JoinHandle, time::Instant};

fn local() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}

#[derive(Debug)]
struct WireEvent {
    to_host: bool,
    forwarded: bool,
    at: Instant,
    bytes: usize,
}

/// Delay both directions independently. Receive and release stay in one select
/// loop, so delaying a packet never blocks the next packet's arrival.
struct DelayProxy {
    addr: SocketAddr,
    task: JoinHandle<()>,
    loss: Arc<AtomicBool>,
    spike_delay_ms: Arc<AtomicU64>,
    trace: Arc<Mutex<Vec<WireEvent>>>,
}

impl DelayProxy {
    async fn start(host: SocketAddr) -> Self {
        // The fixture must absorb a burst without accidental kernel-queue loss.
        let main = UdpSocket::bind(local()).await.unwrap();
        socket2::SockRef::from(&main)
            .set_recv_buffer_size(4 << 20)
            .unwrap();
        let addr = main.local_addr().unwrap();
        let upstream = UdpSocket::bind(local()).await.unwrap();
        socket2::SockRef::from(&upstream)
            .set_recv_buffer_size(4 << 20)
            .unwrap();
        upstream.connect(host).await.unwrap();
        let loss = Arc::new(AtomicBool::new(false));
        let dropping = Arc::clone(&loss);
        let spike_delay_ms = Arc::new(AtomicU64::new(0));
        let spike = Arc::clone(&spike_delay_ms);
        let trace = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&trace);
        let task = tokio::spawn(async move {
            let mut client = None;
            let mut requests = vec![0; 65_536];
            let mut replies = vec![0; 65_536];
            let mut queue = BTreeMap::<(Instant, u64), (bool, Vec<u8>)>::new();
            let mut sequence = 0u64;
            let mut rng = 0xa17f_2301u64;
            loop {
                let deadline = queue.first_key_value().map(|(key, _)| key.0);
                let incoming = tokio::select! {
                    _ = async {
                        match deadline {
                            Some(at) => tokio::time::sleep_until(at).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let (_, (to_host, data)) = queue.pop_first().unwrap();
                        recording.lock().unwrap().push(WireEvent { to_host, forwarded: true, at: Instant::now(), bytes: data.len() });
                        if to_host {
                            upstream.send(&data).await.unwrap();
                        } else {
                            main.send_to(&data, client.unwrap()).await.unwrap();
                        }
                        tokio::task::yield_now().await;
                        continue;
                    }
                    packet = main.recv_from(&mut requests) => {
                        let (len, from) = packet.unwrap();
                        client = Some(from);
                        (true, requests[..len].to_vec())
                    }
                    packet = upstream.recv(&mut replies) => {
                        (false, replies[..packet.unwrap()].to_vec())
                    }
                };
                recording.lock().unwrap().push(WireEvent {
                    to_host: incoming.0,
                    forwarded: false,
                    at: Instant::now(),
                    bytes: incoming.1.len(),
                });
                if !incoming.0 && dropping.load(Ordering::Relaxed) && sequence.is_multiple_of(5) {
                    sequence += 1;
                    continue;
                }
                // Uniform deterministic 80..=120 ms delay, with no packet loss.
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let extra = Duration::from_millis(spike.swap(0, Ordering::Relaxed));
                let due = Instant::now() + Duration::from_millis(80 + rng % 41) + extra;
                queue.insert((due, sequence), incoming);
                sequence += 1;
            }
        });
        Self {
            addr,
            task,
            loss,
            spike_delay_ms,
            trace,
        }
    }
}

impl DelayProxy {
    // At this stage video shards are MTU-sized. Include the occasional MTU
    // probe as well: these are bounds observed at the proxy, not a timestamp
    // taken when send_datagram_wait merely accepted data into Quinn's queue.
    fn wire_spans(&self, to_host: bool) -> (Duration, Duration) {
        let trace = self.trace.lock().unwrap();
        let span = |forwarded| {
            let mut times = trace
                .iter()
                .filter(|event| {
                    event.to_host == to_host && event.forwarded == forwarded && event.bytes > 1000
                })
                .map(|event| event.at);
            let Some(first) = times.next() else {
                return Duration::ZERO;
            };
            times
                .next_back()
                .unwrap_or(first)
                .saturating_duration_since(first)
        };
        (span(false), span(true))
    }
}

impl Drop for DelayProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn burst(host_sends: bool, congest: bool, rtt_spike: Duration) {
    let host_id = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app_id = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = QuicListener::bind(local(), server_config(&host_id).unwrap())
        .await
        .unwrap();
    let proxy = DelayProxy::start(listener.local_addr().unwrap()).await;
    let socket = std::net::UdpSocket::bind(local()).unwrap();
    socket.set_nonblocking(true).unwrap();
    socket2::SockRef::from(&socket)
        .set_recv_buffer_size(4 << 20)
        .unwrap();
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        None,
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let connect = rsp_transport::connect_quic(
        &endpoint,
        proxy.addr,
        Arc::new(client_config(&app_id, ServerPin::Fingerprint(host_id.fingerprint())).unwrap()),
        false,
    );
    let accept = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (client, server) = tokio::join!(connect, accept);
    let client = client.unwrap();
    // As in a media session, finish an application exchange before the burst.
    // Otherwise jitter can deliver media before reordered handshake packets.
    let client_control = client.open_control().await.unwrap();
    client_control.send(b"ready").await.unwrap();
    let server_control = server.accept_control().await.unwrap();
    assert_eq!(server_control.recv().await.unwrap().unwrap(), b"ready"[..]);
    server_control.send(b"ready").await.unwrap();
    assert_eq!(client_control.recv().await.unwrap().unwrap(), b"ready"[..]);
    let (sender, receiver): (&Conn, &Conn) = if host_sends {
        (&server, &client)
    } else {
        (&client, &server)
    };
    if !rtt_spike.is_zero() {
        // Let MTU probes and handshake ACKs settle, then delay exactly one
        // application flight. Its reply acknowledges the delayed packet and
        // supplies a measured RTT sample before the keyframe is queued.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let before = sender.as_quic().unwrap().stats().path.rtt;
        let (out, incoming) = if host_sends {
            (&server_control, &client_control)
        } else {
            (&client_control, &server_control)
        };
        proxy
            .spike_delay_ms
            .store(rtt_spike.as_millis() as u64, Ordering::Relaxed);
        out.send(b"spike").await.unwrap();
        assert_eq!(incoming.recv().await.unwrap().unwrap(), b"spike"[..]);
        incoming.send(b"sample").await.unwrap();
        assert_eq!(out.recv().await.unwrap().unwrap(), b"sample"[..]);
        // The stream reply can beat its ACK because of independent jitter.
        // Queue media only once the sender has actually consumed that sample.
        let after = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let after = sender.as_quic().unwrap().stats().path.rtt;
                if after > before + Duration::from_millis(10) {
                    return after;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "RTT spike must reach the estimator: before={before:?}, after={:?}",
                sender.as_quic().unwrap().stats().path.rtt
            )
        });
        eprintln!("single RTT spike: {before:?} -> {after:?}");
    }
    let frame = vec![0x23; 64 * 1024];
    let packets = video::packetize(
        &frame,
        FrameMeta {
            flags: FLAG_KEYFRAME,
            display_id: 0,
            epoch: 1,
            frame_id: 0,
            capture_ts_us: unix_micros(),
        },
        sender.max_datagram_size().unwrap() + 1,
        0.0,
    )
    .unwrap();
    let quic = sender.as_quic().unwrap();
    let before = quic.stats().path;
    let started = Instant::now();
    let wall_started = std::time::Instant::now();
    let epoch_us = unix_micros();
    proxy.trace.lock().unwrap().clear();
    let send = async {
        for packet in packets {
            sender
                .send_datagram_wait(DatagramKind::Video, packet.slice(1..))
                .await
                .unwrap();
        }
        eprintln!("media enqueue host={host_sends}: {:?}", started.elapsed());
    };
    let receive = async {
        let mut first = None;
        let mut shards = 0;
        let mut reassembler = Reassembler::with_fps(15);
        loop {
            let (kind, payload) =
                tokio::time::timeout(Duration::from_secs(2), receiver.recv_datagram())
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "media stopped after {shards} shards: sender={:?}, receiver={:?}",
                            sender.as_quic().unwrap().stats(),
                            receiver.as_quic().unwrap().stats(),
                        )
                    })
                    .unwrap();
            shards += 1;
            assert_eq!(kind, DatagramKind::Video);
            let first = *first.get_or_insert_with(Instant::now);
            assert!(
                first.elapsed() < Duration::from_micros(2_000_000 / 15),
                "a 64 KiB frame spans an ACK round: {:?}; host={host_sends}; before={before:?}; after={:?}; proxy_spans={:?}; trace={:?}",
                first.elapsed(), quic.stats().path, proxy.wire_spans(!host_sends), proxy.trace.lock().unwrap().iter().map(|event| (event.to_host, event.forwarded, event.at.duration_since(started).as_micros(), event.bytes)).collect::<Vec<_>>()
            );
            let mut wire = vec![video::VIDEO_TYPE];
            wire.extend_from_slice(&payload);
            match reassembler
                .push(&wire, epoch_us + started.elapsed().as_micros() as u64)
                .unwrap()
                .as_slice()
            {
                [] => {}
                [ReassemblyEvent::Frame(received)] => {
                    assert_eq!(received.data.as_slice(), frame.as_slice());
                    let (ingress, egress) = proxy.wire_spans(!host_sends);
                    eprintln!(
                        "media complete host={host_sends} proxy_ingress_span={ingress:?} proxy_egress_span={egress:?} recv_span={:?} before={before:?} after={:?}",
                        first.elapsed(), quic.stats().path,
                    );
                    return;
                }
                events => panic!("keyframe should reassemble within §5.3: {events:?}"),
            }
        }
    };
    tokio::join!(send, receive);
    eprintln!(
        "media clock: transport={:?} wall={:?} injected_rtt_spike={rtt_spike:?}",
        started.elapsed(),
        wall_started.elapsed()
    );
    let quic = sender.as_quic().unwrap();
    assert!(quic.stats().path.rtt >= Duration::from_millis(100));
    if congest {
        let before = quic.stats().path;
        proxy.loss.store(true, Ordering::Relaxed);
        // Periodic packet loss must still reduce the window. Raising the
        // reordering tolerance must not turn congestion control off.
        for _ in 0..512 {
            sender
                .send_datagram_wait(DatagramKind::Video, vec![0x23; 1024].into())
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let path = quic.stats().path;
                if path.congestion_events > before.congestion_events && path.cwnd < before.cwnd {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "real loss reduces the congestion window: before={before:?}, after={:?}",
                quic.stats().path
            )
        });
    }
    client.close(CloseCode::Normal, b"test complete");
    server.close(CloseCode::Normal, b"test complete");
}

/// Do not let Tokio auto-advance past UDP readiness: a runnable pump services
/// the reactor and endpoint tasks before each 1 ms step. Loopback sends enter
/// the kernel synchronously. Several scheduler/reactor turns drain each batch
/// before time progresses, even if the OS suspends this process meanwhile.
async fn with_network_clock(host_sends: bool, congest: bool, rtt_spike: Duration) {
    let test = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(if congest { 8 } else { 5 }),
            burst(host_sends, congest, rtt_spike),
        )
        .await
        .expect("media and bounded loss detection complete");
    });
    while !test.is_finished() {
        for _ in 0..128 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
    }
    test.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn host_keyframe_with_jitter_and_high_rtt() {
    with_network_clock(true, false, Duration::ZERO).await;
}

#[tokio::test(start_paused = true)]
async fn client_burst_with_jitter_and_high_rtt() {
    with_network_clock(false, false, Duration::ZERO).await;
}

#[tokio::test(start_paused = true)]
async fn real_loss_still_reduces_the_media_window() {
    with_network_clock(true, true, Duration::ZERO).await;
}

#[tokio::test(start_paused = true)]
async fn keyframe_after_one_rtt_spike() {
    // Later packets retain the 80..120 ms path; no pacing/window override.
    // Stay below the PTO: otherwise a probe can overtake the delayed packet
    // and correctly supply a fresh, uninflated RTT sample instead.
    with_network_clock(true, false, Duration::from_millis(150)).await;
}
