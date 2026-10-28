//! Packet faults exercise the real QUIC, FEC and frame delivery paths (§5 / IT-07).

use std::time::Duration;

use host_fake::faultnet::{Dir, DirStats, FaultProxy, UdpRule};
use host_fake::harness::{app_identity, open, params, wait_frames, FrameReport, TestHost};
use rsp_client::session::{SessionEvent, SessionHandle, SessionStateKind, SessionStats};
use rsp_core::{clock::unix_micros, pb, trust::Permission};
use tokio::time::{timeout_at, Instant};

const CASE_TIMEOUT: Duration = Duration::from_secs(15);
const CLEANUP_BUDGET: Duration = Duration::from_secs(1);
const IT07_DURATION: Duration = Duration::from_secs(60);
// Keep the §09 requirements, including actual FEC recovery. The production
// packetizer currently passes redundancy=0.0; recovered=0 must fail, even when
// a later keyframe happens to restore the picture.
const IT07_MIN_RECOVERY: f64 = 0.99;
const IT07_MAX_KEYFRAME_REQUESTS: u64 = 2;
const IT07_MAX_GAP: Duration = Duration::from_millis(500);
const FRAMES_AFTER_RECOVERY: u64 = 10;
const FEC_BITRATE_KBPS: u32 = 4_000;

fn injected(rule: &UdpRule, faults: &DirStats) -> u64 {
    if rule.duplicate_pct > 0.0 {
        faults.duplicated
    } else if rule.reorder_pct > 0.0 {
        faults.reordered
    } else if rule.corrupt_pct > 0.0 {
        faults.corrupted
    } else {
        faults.dropped
    }
}

fn stats_since(session: &SessionHandle, before: &SessionStats) -> SessionStats {
    let mut stats = session.stats();
    stats.frames_received -= before.frames_received;
    stats.frames_lost -= before.frames_lost;
    stats.frames_recovered -= before.frames_recovered;
    stats.keyframe_requests -= before.keyframe_requests;
    stats
}

fn epoch_newer(a: u8, b: u8) -> bool {
    let delta = a.wrapping_sub(b);
    delta != 0 && delta < 128
}

struct Observation {
    report: FrameReport,
    closed: bool,
    ordered: bool,
    last_id: Option<(u8, u32)>,
    last_received_us: u64,
    last_capture_us: u64,
    frame_ages_us: Vec<u64>,
    worst_gap: Option<(Duration, Duration, Duration, Duration)>,
    recovery_observed_us: Option<u64>,
    frames_after_recovery: u64,
}

impl Observation {
    fn record(&mut self, event: SessionEvent, stats: &SessionStats) {
        if stats.frames_recovered > 0 {
            self.recovery_observed_us.get_or_insert_with(unix_micros);
        }
        match event {
            SessionEvent::Frame(frame) => {
                self.frame_ages_us
                    .push(frame.received_us.saturating_sub(frame.frame.capture_ts_us));
                // Use the receiver's timestamp: a busy test consumer can drain
                // queued events much later, hiding or inventing frame gaps.
                let gap =
                    Duration::from_micros(frame.received_us.saturating_sub(self.last_received_us));
                if gap > IT07_MAX_GAP {
                    eprintln!(
                        "GAP receive_us={} capture_us={} gap_us={} age_us={}",
                        frame.received_us,
                        frame.frame.capture_ts_us,
                        gap.as_micros(),
                        frame.received_us.saturating_sub(frame.frame.capture_ts_us)
                    );
                }
                if gap > self.report.max_gap {
                    self.worst_gap = Some((
                        gap,
                        Duration::from_micros(if self.last_id.is_some() {
                            frame
                                .frame
                                .capture_ts_us
                                .saturating_sub(self.last_capture_us)
                        } else {
                            0
                        }),
                        Duration::from_micros(
                            frame.received_us.saturating_sub(frame.frame.capture_ts_us),
                        ),
                        Duration::from_micros(unix_micros().saturating_sub(frame.received_us)),
                    ));
                }
                self.report.max_gap = self.report.max_gap.max(gap);
                self.last_capture_us = frame.frame.capture_ts_us;
                self.last_received_us = self.last_received_us.max(frame.received_us);
                let id = (frame.frame.epoch, frame.frame.frame_id);
                if let Some((epoch, previous)) = self.last_id {
                    // StreamConfig changes bitrate/quality and starts a new
                    // epoch (03 §7.4). The first frame delivered from a new
                    // epoch must be a keyframe; ids remain strictly increasing
                    // within one epoch (03 §5.1).
                    self.ordered &= if id.0 == epoch {
                        id.1 > previous
                    } else {
                        epoch_newer(id.0, epoch) && frame.frame.keyframe
                    };
                }
                self.last_id = Some(id);
                self.report.frames += 1;
                self.report.keyframes += u64::from(frame.frame.keyframe);
                // Do not count queued frames from before recovery as proof
                // that the picture continued after the recovery was observed.
                self.frames_after_recovery += u64::from(
                    self.recovery_observed_us
                        .is_some_and(|at| frame.received_us > at),
                );
            }
            SessionEvent::Stats(stats) => self.report.last_stats = Some(stats),
            SessionEvent::StreamConfig(config) => {
                assert!(
                    config.bitrate_kbps <= FEC_BITRATE_KBPS && config.fps == 30,
                    "FEC workload exceeded its negotiated budget: {config:?}"
                );
                self.report.stream = Some(config);
            }
            SessionEvent::StateChanged(state) => self.report.states.push(state),
            SessionEvent::Error { code, message } => self.report.errors.push((code, message)),
            SessionEvent::Ended { code, kind, .. } => self.report.ended = Some((code, kind)),
            _ => {}
        }
    }
}

/// Short cases wait for a frame quota AND evidence that the fault was handled,
/// with a shared 15 s deadline for setup, observation and cleanup. IT-07 alone
/// retains its required full 60 s observation; it must not pass on a short burst.
async fn exercise(
    name: &str,
    seed: u64,
    dir: Dir,
    mut rule: UdpRule,
    seconds: u64,
) -> (FrameReport, SessionStats) {
    let long_run = seconds == IT07_DURATION.as_secs();
    if long_run {
        host_core::pipeline::diagnostics::enable();
    }
    let setup_deadline = Instant::now() + CASE_TIMEOUT - CLEANUP_BUDGET;
    let deadline = setup_deadline
        + CLEANUP_BUDGET
        + if long_run {
            IT07_DURATION
        } else {
            Duration::ZERO
        };
    let test = async {
        let host = TestHost::start().await;
        let id = app_identity();
        host.pair(&id, Permission::Control);
        let proxy = FaultProxy::start(host.quic_addr, host.tcp_addr, seed)
            .await
            .expect("start loopback fault proxy");
        let mut options = params(&host, id, &[proxy.addr()]);
        // LAN clamps a 2 Mbps request to 4 Mbps anyway. Auto can then grow to
        // 40 Mbps and rebuild epochs based on scheduler-dependent feedback.
        // Keep this FEC workload at its original starting budget; adaptive
        // quality is exercised by netem. FEC feedback remains enabled.
        options.start.max_width = 1280;
        options.start.fps = 30;
        options.start.bitrate_kbps = FEC_BITRATE_KBPS;
        options.quality = pb::Quality::Eco;
        options.dial_options.tcp_after = Duration::from_secs(10);
        let (session, mut events) = timeout_at(setup_deadline, open(options))
            .await
            .expect("open session before setup deadline")
            .expect("open paired session through fault proxy");

        // Packet-count warmup alone does not establish that the client has a
        // StreamConfig and a decodable picture. Install faults only after both.
        let warmup = wait_frames(
            &mut events,
            3,
            setup_deadline.saturating_duration_since(Instant::now()),
        )
        .await;
        assert!(
            warmup.ended.is_none() && warmup.errors.is_empty(),
            "{name}: {warmup:?}"
        );
        assert_eq!(warmup.frames, 3, "{name}: video did not start: {warmup:?}");
        let config = warmup.stream.expect("warmup includes StreamConfig");
        assert!(
            config.fps > 0,
            "{name}: negotiated frame rate must be positive"
        );
        assert!(
            config.bitrate_kbps <= FEC_BITRATE_KBPS && config.fps == 30,
            "{name}: unexpected FEC workload: {config:?}"
        );
        // A bottleneck comparison needs a measured unloaded RTT, not a
        // guessed loopback baseline. Drain video while waiting for Ping/Pong.
        if rule.rate_kbps.is_some() {
            timeout_at(setup_deadline, async {
                while session.stats().rtt_us.is_none() {
                    assert!(
                        events.recv().await.is_some(),
                        "stream closed during RTT warmup"
                    );
                }
            })
            .await
            .expect("measure unloaded RTT before enabling the bottleneck");
        }
        let before = session.stats();
        let faults_before = proxy.stats().direction(dir).clone();
        // Burst ordinals are relative to skip_first. Counting the actual
        // handshake prefix makes periodic loss independent of its length.
        rule.skip_first =
            u32::try_from(faults_before.received.max(30)).expect("warmup packet count fits u32");
        proxy.set_udp(dir, rule.clone());

        let needs_recovery = dir == Dir::ToClient
            && (rule.burst.is_some() || rule.loss_pct > 0.0 || rule.corrupt_pct > 0.0);
        let mut target_frames = u64::from(config.fps) * seconds;
        if name == "loss_10pct_survives" {
            target_frames = target_frames.div_ceil(2);
        } else if name == "burst_loss" {
            // An eight-packet burst every fifty packets removes enough
            // shards to make at least one P frame unrecoverable. Per 03 §5.3,
            // a lost frame restarts delivery at the next keyframe, so the
            // nominal-fps quota is not achievable under this fault pattern.
            target_frames = target_frames.div_ceil(4);
        }
        if long_run {
            eprintln!(
                "PIPELINE warmup={:?}",
                host_core::pipeline::diagnostics::snapshot()
            );
            host_core::pipeline::diagnostics::reset();
        }
        let started = Instant::now();
        let observe_deadline = if long_run {
            started + IT07_DURATION
        } else {
            deadline - CLEANUP_BUDGET
        };
        let mut observation = Observation {
            report: FrameReport {
                stream: Some(config),
                ..FrameReport::default()
            },
            closed: false,
            ordered: true,
            last_id: None,
            last_received_us: unix_micros(),
            last_capture_us: 0,
            frame_ages_us: Vec::new(),
            worst_gap: None,
            recovery_observed_us: None,
            frames_after_recovery: 0,
        };
        let mut completed = false;
        let mut samples = tokio::time::interval(Duration::from_secs(1));
        samples.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut rtts_us = Vec::new();
        let mut previous_bytes = faults_before.received_bytes;
        let mut previous_at = started;
        let mut rates_kbps = Vec::new();
        loop {
            let event = tokio::select! {
                event = timeout_at(observe_deadline, events.recv()) => event,
                _ = samples.tick() => {
                    if long_run {
                        eprintln!("CLIENT_TICK t_ms={} pings_sent={} kf_requests={}", started.elapsed().as_millis(), session.stats().pings_sent.saturating_sub(before.pings_sent), session.stats().keyframe_requests.saturating_sub(before.keyframe_requests));
                        eprintln!("PIPELINE t_ms={} counters={:?}", started.elapsed().as_millis(), host_core::pipeline::diagnostics::snapshot());
                    }
                    if let Some(rtt) = session.stats().rtt_us { rtts_us.push(rtt); }
                    let bytes = proxy.stats().direction(dir).received_bytes;
                    let now = Instant::now();
                    if now.duration_since(previous_at) >= Duration::from_millis(500) {
                        rates_kbps.push((bytes - previous_bytes) as f64 * 0.008 / now.duration_since(previous_at).as_secs_f64());
                        previous_bytes = bytes;
                        previous_at = now;
                    }
                    continue;
                }
            };
            match event {
                Ok(Some(event)) => {
                    observation.record(event, &stats_since(&session, &before));
                }
                Ok(None) => {
                    observation.closed = true;
                    break;
                }
                Err(_) => {
                    completed = long_run;
                    break;
                }
            }
            if observation.report.ended.is_some() || !observation.report.errors.is_empty() {
                break;
            }
            let stats = stats_since(&session, &before);
            let faults = proxy.stats().direction(dir).clone();
            let continued_after_recovery =
                observation.frames_after_recovery >= FRAMES_AFTER_RECOVERY;
            if !long_run
                && observation.report.frames >= target_frames
                && injected(&rule, &faults) > injected(&rule, &faults_before)
                && (!needs_recovery || continued_after_recovery)
                && (dir != Dir::ToHost || stats.rtt_us.is_some())
            {
                completed = true;
                break;
            }
        }
        // Drain already-delivered events before measuring the tail, so a
        // delayed test consumer cannot invent a freeze or hide a queued error.
        loop {
            match events.try_recv() {
                Ok(event) => observation.record(event, &stats_since(&session, &before)),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    observation.closed = true;
                    break;
                }
            }
        }
        // Include the silent tail, even when the timeout expires before the
        // next frame. A frozen picture must not have a reported gap of zero.
        observation.report.max_gap = observation.report.max_gap.max(Duration::from_micros(
            unix_micros().saturating_sub(observation.last_received_us),
        ));
        let stats = stats_since(&session, &before);
        let state = session.state();
        let mut proxy_stats = proxy.stats();
        let faults = proxy_stats.direction(dir).clone();
        eprintln!(
            "fec {name}: frames={} keyframes={} lost={} recovered={} kf_req={} max_gap_ms={}",
            stats.frames_received,
            observation.report.keyframes,
            stats.frames_lost,
            stats.frames_recovered,
            stats.keyframe_requests,
            observation.report.max_gap.as_millis(),
        );
        eprintln!(
            "fec {name}: elapsed_ms={} target_frames={target_frames} completed={completed} \
             frames_after_recovery={} injected={} proxy={faults:?}",
            started.elapsed().as_millis(),
            observation.frames_after_recovery,
            injected(&rule, &faults) - injected(&rule, &faults_before),
        );
        if long_run {
            let affected = stats.frames_recovered + stats.frames_lost;
            let recovery = stats.frames_recovered as f64 / affected.max(1) as f64;
            eprintln!(
                "fec IT-07: recovery={recovery:.6} meets_09={}",
                stats.frames_recovered > 0
                    && recovery >= IT07_MIN_RECOVERY
                    && stats.keyframe_requests <= IT07_MAX_KEYFRAME_REQUESTS
                    && observation.report.max_gap <= IT07_MAX_GAP
            );
        }
        if long_run {
            let elapsed = started.elapsed().as_secs_f64();
            let input_kbps =
                (faults.received_bytes - faults_before.received_bytes) as f64 * 0.008 / elapsed;
            let output_kbps =
                (faults.forwarded_bytes - faults_before.forwarded_bytes) as f64 * 0.008 / elapsed;
            let mut histogram = [0u64; 21];
            for (i, count) in histogram.iter_mut().enumerate() {
                *count = faults.rate_queue_histogram[i] - faults_before.rate_queue_histogram[i];
            }
            let queue_near_full = histogram[15..].iter().sum::<u64>() as f64
                / histogram.iter().sum::<u64>().max(1) as f64;
            eprintln!("BENCH name={name} input_kbps={input_kbps:.1} output_kbps={output_kbps:.1} baseline_rtt_us={:?} rtt_p50_us={} rtt_p95_us={} age_p95_us={} queue_ge150ms_fraction={queue_near_full:.6} queue_dropped={} queue_hist={histogram:?} rates_kbps={rates_kbps:?}", before.rtt_us, percentile(&mut rtts_us, 50), percentile(&mut rtts_us, 95), percentile(&mut observation.frame_ages_us, 95), faults.rate_queue_dropped - faults_before.rate_queue_dropped);
        }
        if long_run {
            eprintln!(
                "CLIENT_TICK final_pings_sent={} warmup_pings_sent={}",
                session.stats().pings_sent.saturating_sub(before.pings_sent),
                before.pings_sent
            );
            eprintln!(
                "PIPELINE final={:?}",
                host_core::pipeline::diagnostics::snapshot()
            );
        }
        session.close();
        proxy.shutdown().await;
        host.stop().await;

        eprintln!(
            "fec {name}: worst gap (receive, capture, frame age, consumer lag)={:?}",
            observation.worst_gap
        );
        let report = observation.report;
        assert!(report.ended.is_none(), "{name}: ended: {:?}", report.ended);
        assert!(!observation.closed, "{name}: event channel closed");
        assert_eq!(state, SessionStateKind::Streaming, "{name}: {state:?}");
        assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
        assert!(report.frames > 0, "{name}: no video frames arrived");
        assert!(
            observation.ordered,
            "{name}: duplicate, reordered or unexpected epoch frames delivered"
        );
        assert_eq!(proxy_stats.tcp_accepted, 0, "{name}: used TCP fallback");
        assert!(
            injected(&rule, &faults) > injected(&rule, &faults_before),
            "{name}: fault was not injected: {faults:?}"
        );
        if needs_recovery {
            assert!(stats.frames_recovered > 0,
                "{name}: FEC did not recover a dropped video shard before the deadline: {stats:?}; proxy={faults:?}");
            assert!(
                observation.frames_after_recovery >= FRAMES_AFTER_RECOVERY,
                "{name}: video did not continue after FEC recovery: {report:?}"
            );
        }
        assert!(
            completed,
            "{name}: conditions not met before deadline: {report:?}; {stats:?}"
        );
        (report, stats)
    };
    timeout_at(deadline, test)
        .await
        .unwrap_or_else(|_| panic!("{name}: exceeded total budget (15 s, plus 60 s for IT-07)"))
}

fn expected_frames(report: &FrameReport, seconds: u64) -> u64 {
    u64::from(
        report
            .stream
            .as_ref()
            .expect("StreamConfig was checked")
            .fps,
    ) * seconds
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it07_lossy_5pct_60s() {
    // The follow-up task requires deterministic loss: every twentieth UDP
    // packet, rather than load-dependent sampling of random loss.
    let (report, stats) = exercise(
        "it07_lossy_5pct_60s",
        0xa17d_0001,
        Dir::ToClient,
        UdpRule {
            burst: Some((20, 1)),
            ..UdpRule::default()
        },
        60,
    )
    .await;
    let affected = stats.frames_recovered + stats.frames_lost;
    assert!(affected > 0, "IT-07 must exercise FEC recovery");
    let recovery = stats.frames_recovered as f64 / affected as f64;
    assert!(
        recovery >= IT07_MIN_RECOVERY,
        "IT-07 recovery={recovery:.6}, minimum={IT07_MIN_RECOVERY}"
    );
    assert!(
        stats.keyframe_requests <= IT07_MAX_KEYFRAME_REQUESTS,
        "IT-07 {stats:?}"
    );
    assert!(report.max_gap <= IT07_MAX_GAP, "IT-07 {report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loss_1pct() {
    let (report, stats) = exercise(
        "loss_1pct",
        0xa17d_0002,
        Dir::ToClient,
        UdpRule {
            burst: Some((100, 1)),
            ..UdpRule::default()
        },
        5,
    )
    .await;
    assert!(stats.frames_lost <= 2, "{stats:?}");
    assert!(report.max_gap <= Duration::from_millis(300), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loss_10pct_survives() {
    let (report, stats) = exercise(
        "loss_10pct_survives",
        0xa17d_0003,
        Dir::ToClient,
        UdpRule {
            burst: Some((10, 1)),
            ..UdpRule::default()
        },
        8,
    )
    .await;
    let expected = expected_frames(&report, 8);
    assert!(
        stats.frames_received * 2 >= expected,
        "received {} frames, expected at least half of {expected}",
        stats.frames_received
    );
    assert!(report.max_gap <= Duration::from_secs(1), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_loss() {
    let (report, stats) = exercise(
        "burst_loss",
        0xa17d_0004,
        Dir::ToClient,
        UdpRule {
            burst: Some((50, 8)),
            ..UdpRule::default()
        },
        8,
    )
    .await;
    assert!(
        stats.frames_recovered + stats.keyframe_requests > 0,
        "{stats:?}"
    );
    assert!(report.max_gap <= Duration::from_secs(1), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_10pct() {
    let (report, stats) = exercise(
        "duplicate_10pct",
        0xa17d_0005,
        Dir::ToClient,
        UdpRule {
            duplicate_pct: 10.0,
            ..UdpRule::default()
        },
        5,
    )
    .await;
    let upper_bound = expected_frames(&report, 5) * 11 / 10;
    assert!(
        stats.frames_received <= upper_bound,
        "{stats:?}; upper={upper_bound}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reorder_10pct() {
    let (report, stats) = exercise(
        "reorder_10pct",
        0xa17d_0006,
        Dir::ToClient,
        UdpRule {
            reorder_pct: 10.0,
            ..UdpRule::default()
        },
        5,
    )
    .await;
    let total = stats.frames_received + stats.frames_lost;
    assert!(stats.frames_lost * 100 <= total, "{stats:?}; total={total}");
    assert!(report.max_gap <= Duration::from_millis(500), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_2pct() {
    let (report, stats) = exercise(
        "corrupt_2pct",
        0xa17d_0007,
        Dir::ToClient,
        UdpRule {
            corrupt_pct: 2.0,
            ..UdpRule::default()
        },
        5,
    )
    .await;
    assert!(stats.frames_lost + stats.frames_recovered > 0, "{stats:?}");
    assert!(report.max_gap <= Duration::from_millis(500), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uplink_loss_5pct() {
    let (report, stats) = exercise(
        "uplink_loss_5pct",
        0xa17d_0008,
        Dir::ToHost,
        UdpRule {
            burst: Some((20, 1)),
            ..UdpRule::default()
        },
        5,
    )
    .await;
    assert!(report.max_gap <= Duration::from_millis(500), "{report:?}");
    assert!(stats.rtt_us.is_some(), "{stats:?}");
}

// Keep the same 4 Mbps source and FEC overhead as IT-07, but constrain the
// downstream wire rate too. Report queue occupancy rather than inferring it
// from a throughput average or the queue's fixed maximum residence time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bottleneck_4mbps_lossy_5pct_60s() {
    exercise(
        "bottleneck_4mbps_lossy_5pct_60s",
        0xa17d_0035,
        Dir::ToClient,
        UdpRule {
            rate_kbps: Some(4_000),
            burst: Some((20, 1)),
            ..UdpRule::default()
        },
        60,
    )
    .await;
}

fn percentile(values: &mut [u64], percent: usize) -> u64 {
    values.sort_unstable();
    if values.is_empty() {
        0
    } else {
        values[(values.len() * percent).div_ceil(100).saturating_sub(1)]
    }
}

/// Isolate the transport from capture/encoding CPU limits: offer 4 Mbps of
/// prebuilt video plus 25% parity to a 4 Mbps wire bottleneck for a full minute.
/// Timestamp each scheduled frame so sender buffering remains in frame age.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saturated_4mbps_lossy_5pct_60s() {
    use rsp_core::video::{self, FrameMeta, Reassembler, ReassemblyEvent, FLAG_KEYFRAME};
    use rsp_transport::{
        identity::{CertKind, LocalIdentity},
        tls::{client_config, server_config, ServerPin},
        CloseCode, DatagramKind, QuicListener,
    };
    use std::sync::Arc;
    let local = || "127.0.0.1:0".parse().unwrap();
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let proxy = FaultProxy::start(listener.local_addr().unwrap(), None, 0xa17d_0036)
        .await
        .unwrap();
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let config = Arc::new(client_config(&app, ServerPin::Fingerprint(host.fingerprint())).unwrap());
    let dial = rsp_transport::connect_quic(&endpoint, proxy.addr(), config, false);
    let accept = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (client, server) = tokio::join!(dial, accept);
    let client = client.unwrap();
    let tx_control = server.open_control().await.unwrap();
    tx_control.send(b"ready").await.unwrap();
    let rx_control = client.accept_control().await.unwrap();
    assert_eq!(rx_control.recv().await.unwrap().unwrap(), b"ready"[..]);
    rx_control.send(b"ready").await.unwrap();
    assert_eq!(tx_control.recv().await.unwrap().unwrap(), b"ready"[..]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let quic = server.as_quic().unwrap();
    let baseline = quic.stats().path;
    let before = proxy.stats().to_client;
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            rate_kbps: Some(4_000),
            burst: Some((20, 1)),
            skip_first: before.received as u32,
            ..UdpRule::default()
        },
    );
    let started = Instant::now();
    let epoch_us = unix_micros();
    let deadline = started + Duration::from_secs(60);
    let send = async {
        let mut interval = tokio::time::interval(Duration::from_nanos(1_000_000_000 / 30));
        let frame = vec![0x35; 4_000_000 / 8 / 30];
        let mut id = 0;
        loop {
            interval.tick().await;
            let packets = video::packetize(
                &frame,
                FrameMeta {
                    flags: FLAG_KEYFRAME,
                    display_id: 0,
                    epoch: 1,
                    frame_id: id,
                    capture_ts_us: epoch_us + u64::from(id) * 1_000_000 / 30,
                },
                server.max_datagram_size().unwrap() + 1,
                0.25,
            )
            .unwrap();
            for packet in packets {
                server
                    .send_datagram_wait(DatagramKind::Video, packet.slice(1..))
                    .await
                    .unwrap();
            }
            id += 1;
        }
    };
    let receive = async {
        let mut reassembler = Reassembler::with_fps(30);
        let mut ages = Vec::new();
        let mut rtts = Vec::new();
        let mut sample = tokio::time::interval(Duration::from_secs(1));
        let mut previous = before.received_bytes;
        let mut previous_at = started;
        let mut rates = Vec::new();
        let mut previous_drops = before.rate_queue_dropped;
        let mut startup_drops = before.rate_queue_dropped;
        let mut queue_drops_per_second = Vec::new();
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                _ = sample.tick() => {
                    rtts.push(quic.stats().path.rtt.as_micros() as u64);
                    let proxy_stats = proxy.stats().to_client;
                    let bytes = proxy_stats.received_bytes;
                    if started.elapsed() < Duration::from_secs(10) {
                        startup_drops = proxy_stats.rate_queue_dropped;
                    }
                    let elapsed = previous_at.elapsed().as_secs_f64();
                    if elapsed >= 0.5 {
                        rates.push((bytes - previous) as f64 * 0.008 / elapsed);
                        queue_drops_per_second.push(proxy_stats.rate_queue_dropped - previous_drops);
                        previous_drops = proxy_stats.rate_queue_dropped;
                        previous = bytes;
                        previous_at = Instant::now();
                    }
                }
                packet = client.recv_datagram() => {
                    let (kind, payload) = packet.unwrap();
                    assert_eq!(kind, DatagramKind::Video);
                    let now = unix_micros();
                    let mut wire = vec![video::VIDEO_TYPE];
                    wire.extend_from_slice(&payload);
                    for event in reassembler.push(&wire, now).unwrap() {
                        if let ReassemblyEvent::Frame(frame) = event {
                            ages.push(now.saturating_sub(frame.capture_ts_us));
                        }
                    }
                }
            }
        }
        (ages, rtts, rates, queue_drops_per_second, startup_drops)
    };
    let (mut ages, mut rtts, rates, queue_drops_per_second, startup_drops) = tokio::select! {
        result = receive => result,
        _ = send => unreachable!("sender runs until the measurement ends"),
    };
    let after = proxy.stats().to_client;
    let elapsed = started.elapsed().as_secs_f64();
    let input = (after.received_bytes - before.received_bytes) as f64 * 0.008 / elapsed;
    let output = (after.forwarded_bytes - before.forwarded_bytes) as f64 * 0.008 / elapsed;
    let histogram = after.rate_queue_histogram;
    let near_full =
        histogram[15..].iter().sum::<u64>() as f64 / histogram.iter().sum::<u64>().max(1) as f64;
    eprintln!("SATURATED input_kbps={input:.1} output_kbps={output:.1} baseline_rtt_us={} rtt_p50_us={} rtt_p95_us={} age_p95_us={} frames={} queue_ge150ms_fraction={near_full:.6} queue_dropped={} queue_hist={histogram:?} rates_kbps={rates:?} queue_drops_per_second={queue_drops_per_second:?} before={baseline:?} after={:?}", baseline.rtt.as_micros(), percentile(&mut rtts, 50), percentile(&mut rtts, 95), percentile(&mut ages, 95), ages.len(), after.rate_queue_dropped, quic.stats().path);
    client.close(CloseCode::Normal, b"benchmark complete");
    server.close(CloseCode::Normal, b"benchmark complete");
    proxy.shutdown().await;
    assert!(!ages.is_empty(), "bottleneck must deliver complete frames");
    // The shared 240 KB startup window can briefly overshoot the bottleneck.
    // Once it settles, a media floor must not keep overflowing the 200 ms FIFO.
    assert_eq!(after.rate_queue_dropped - startup_drops, 0,
        "congestion control keeps overflowing the bottleneck after startup: {queue_drops_per_second:?}");
}
