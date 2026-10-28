use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::Poll,
    time::Duration,
};

use rsp_core::errors::ErrorCode;
use rsp_transport::{
    connect_quic, connect_tcp,
    dial::{
        classify_failure, plan, race, AttemptLog, AttemptResult, Candidate, CandidateSource,
        DefaultDialer, DialFailure, DialOpts, DialPlan, IfaceInfo, IpNet, LocalNet, Observations,
        PathKind, Skipped,
    },
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, ConnError, QuicListener, TransportKind,
};
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn candidate(port: u16, kind: PathKind) -> Candidate {
    Candidate {
        addr: SocketAddr::from(([127, 0, 0, 1], port)),
        kind,
        source: CandidateSource::Cache,
    }
}

fn iface(net: &str, vpn: bool) -> LocalNet {
    LocalNet {
        interfaces: vec![IfaceInfo {
            name: if vpn { "vpn0" } else { "eth0" }.into(),
            addrs: vec![net.parse().unwrap()],
            is_vpn: vpn,
        }],
    }
}

#[test]
fn planning_merges_sources_sorts_and_preserves_original_cached_path() {
    let local = iface("127.0.0.1/8", false);
    let mut lan = candidate(1, PathKind::Lan);
    lan.source = CandidateSource::Mdns;
    let cached = Candidate {
        kind: PathKind::LastGood(Box::new(PathKind::Tailnet)),
        source: CandidateSource::Cache,
        ..candidate(2, PathKind::Tailnet)
    };
    let p = plan(
        vec![
            candidate(1, PathKind::Manual),
            candidate(5, PathKind::Manual),
            candidate(4, PathKind::Tailnet),
            candidate(3, PathKind::Intranet),
            lan.clone(),
            cached.clone(),
            candidate(2, PathKind::Tailnet),
        ],
        &local,
        &[],
    );
    assert_eq!(p.candidates.len(), 5);
    assert_eq!(p.candidates[0], cached);
    assert_eq!(p.candidates[1], lan);
    assert_eq!(p.candidates[2].kind, PathKind::Intranet);
    assert_eq!(p.candidates[3].kind, PathKind::Tailnet);
    assert_eq!(p.candidates[4].kind, PathKind::Manual);
    assert!(p.skipped.is_empty());
}

#[test]
fn planning_checks_vpn_and_direct_subnets_even_for_cached_addresses() {
    let intranet = Candidate {
        addr: "10.30.2.8:7400".parse().unwrap(),
        kind: PathKind::LastGood(Box::new(PathKind::Lan)),
        source: CandidateSource::Cache,
    };
    let lan = Candidate {
        addr: "192.168.31.9:7400".parse().unwrap(),
        kind: PathKind::Lan,
        source: CandidateSource::Mdns,
    };
    let cidrs = ["10.30.0.0/16".parse().unwrap()];
    let p = plan(
        vec![intranet.clone(), lan.clone()],
        &LocalNet::default(),
        &cidrs,
    );
    assert!(p.candidates.is_empty());
    assert_eq!(p.skipped[0].1, Skipped::NeedsVpn);
    assert_eq!(p.skipped[0].0.kind.original(), &PathKind::Intranet);
    assert_eq!(p.skipped[1].1, Skipped::NotOnLan);
    let vpn = iface("10.99.0.2/24", true);
    let p = plan(vec![intranet.clone(), lan], &vpn, &cidrs);
    assert_eq!(p.candidates.len(), 1);
    assert_eq!(p.skipped[0].1, Skipped::NotOnLan);
    let p = plan(vec![intranet], &iface("10.30.2.3/24", false), &cidrs);
    assert_eq!(p.candidates.len(), 1);
}

#[test]
fn network_prefixes_cover_ipv4_ipv6_and_reject_invalid_input() {
    for (net, inside, outside) in [
        ("10.30.2.3/24", "10.30.2.255", "10.30.3.0"),
        ("2001:db8::1/64", "2001:db8::ffff", "2001:db8:0:1::1"),
        ("10.30.2.3/32", "10.30.2.3", "10.30.2.4"),
        ("::1/128", "::1", "::2"),
    ] {
        let net: IpNet = net.parse().unwrap();
        assert!(net.contains(inside.parse().unwrap()));
        assert!(!net.contains(outside.parse().unwrap()));
    }
    assert!("0.0.0.0/0"
        .parse::<IpNet>()
        .unwrap()
        .contains("10.30.2.3".parse().unwrap()));
    assert!("::/0"
        .parse::<IpNet>()
        .unwrap()
        .contains("::1".parse().unwrap()));
    assert!(!"::/0"
        .parse::<IpNet>()
        .unwrap()
        .contains("127.0.0.1".parse().unwrap()));
    for invalid in ["10.30.0.1/33", "::1/129", "10.30.0.1", "bad/2"] {
        assert!(invalid.parse::<IpNet>().is_err());
    }
}

// A fake Dialer still returns genuine Conns, prepared before pausing time. The
// certificate cannot be faked: race must read the authenticated Conn's peer key.
struct HeldConn(Option<Conn>);

impl Drop for HeldConn {
    fn drop(&mut self) {
        if let Some(conn) = &self.0 {
            conn.close(CloseCode::Normal, b"fake handshake cancelled");
        }
    }
}

enum Reply {
    Connected(HeldConn),
    Error(ConnError),
    Hang,
}

struct Script {
    delay: Duration,
    reply: Reply,
}

type Starts = Vec<(SocketAddr, TransportKind, Instant)>;

#[derive(Default)]
struct FakeDialer {
    scripts: Mutex<HashMap<(SocketAddr, u8), Script>>,
    starts: Mutex<Starts>,
    active: Arc<AtomicUsize>,
}

impl FakeDialer {
    fn add(&self, candidate: &Candidate, transport: TransportKind, delay_ms: u64, reply: Reply) {
        self.scripts.lock().unwrap().insert(
            (candidate.addr, transport as u8),
            Script {
                delay: Duration::from_millis(delay_ms),
                reply,
            },
        );
    }

    fn success(&self, c: &Candidate, transport: TransportKind, delay: u64, conn: Conn) {
        self.add(c, transport, delay, Reply::Connected(HeldConn(Some(conn))));
    }

    fn count(&self) -> usize {
        self.starts.lock().unwrap().len()
    }
}

struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Dialer for FakeDialer {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        self.starts
            .lock()
            .unwrap()
            .push((addr, kind, Instant::now()));
        let script = self
            .scripts
            .lock()
            .unwrap()
            .remove(&(addr, kind as u8))
            .unwrap_or(Script {
                delay: Duration::ZERO,
                reply: Reply::Hang,
            });
        self.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(self.active.clone());
        let delay = tokio::time::sleep(script.delay);
        Box::pin(async move {
            let _active = active;
            delay.await;
            match script.reply {
                Reply::Connected(mut held) => Ok(held.0.take().unwrap()),
                Reply::Error(error) => Err(error),
                Reply::Hang => std::future::pending().await,
            }
        })
    }
}

struct Pool {
    _listeners: Vec<QuicListener>,
    _endpoint: quinn::Endpoint,
    conns: Vec<Conn>,
    peers: Vec<Conn>,
    expected: [u8; 32],
}

impl Drop for Pool {
    fn drop(&mut self) {
        for conn in self.conns.iter().chain(&self.peers) {
            conn.close(CloseCode::Normal, b"fixture done");
        }
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("test timed out")
}

async fn pool(correct: &[bool]) -> Pool {
    // Keep real I/O from auto-advancing the paused clock. Resuming and pausing
    // here would shift the timer origin by a fractional tick and spoil exact
    // millisecond assertions in the race tests.
    let (unblock, wait) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || wait.recv().unwrap());
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let other = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let config = Arc::new(client_config(&app, ServerPin::Any).unwrap());
    let mut result = Pool {
        _listeners: vec![],
        _endpoint: endpoint.clone(),
        conns: vec![],
        peers: vec![],
        expected: host.fingerprint(),
    };
    for good in correct {
        let listener = QuicListener::bind(
            local(),
            server_config(if *good { &host } else { &other }).unwrap(),
        )
        .await
        .unwrap();
        let accept = async { listener.accept().await.unwrap().accept().await.unwrap() };
        let (conn, peer) = bounded(async {
            tokio::join!(
                connect_quic(
                    &endpoint,
                    listener.local_addr().unwrap(),
                    config.clone(),
                    true
                ),
                accept,
            )
        })
        .await;
        result.conns.push(conn.unwrap());
        result.peers.push(peer);
        result._listeners.push(listener);
    }
    unblock.send(()).unwrap();
    blocker.await.unwrap();
    result
}

async fn step(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
}

// A step is complete when the race has actually been polled at the new time.
// This also handles negative assertions, where no dial or completion is due.
struct TestTask<T> {
    task: tokio::task::JoinHandle<T>,
    poll: tokio::sync::mpsc::UnboundedSender<tokio::sync::oneshot::Sender<()>>,
}

impl<T: Send + 'static> TestTask<T> {
    fn spawn(future: impl Future<Output = T> + Send + 'static) -> Self {
        let (poll, mut requests) =
            tokio::sync::mpsc::unbounded_channel::<tokio::sync::oneshot::Sender<()>>();
        let task = tokio::spawn(async move {
            tokio::pin!(future);
            loop {
                tokio::select! {
                    biased;
                    Some(done) = requests.recv() => {
                        let result = poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
                        let _ = done.send(());
                        if let Poll::Ready(result) = result {
                            return result;
                        }
                    }
                    result = &mut future => return result,
                }
            }
        });
        Self { task, poll }
    }

    async fn step(&self, ms: u64) {
        step(ms).await;
        let (done, polled) = tokio::sync::oneshot::channel();
        if self.poll.send(done).is_err() {
            return;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        tokio::pin!(polled);
        loop {
            tokio::select! {
                biased;
                _ = &mut polled => return,
                _ = tokio::task::yield_now() => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "race was not polled at {:?}; task finished: {}",
                        Instant::now(), self.is_finished()
                    );
                }
            }
        }
    }

    fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl<T> Future for TestTask<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().task).poll(cx)
    }
}

async fn is_closed(conn: &Conn) -> bool {
    let mut closed = Box::pin(conn.closed());
    poll_fn(|cx| Poll::Ready(closed.as_mut().poll(cx).is_ready())).await
}

fn close_outcome(outcome: &rsp_transport::dial::DialOutcome) {
    outcome.primary.close(CloseCode::Normal, b"test done");
    if let Some((conn, _)) = &outcome.backup {
        conn.close(CloseCode::Normal, b"test done");
    }
}

#[tokio::test(start_paused = true)]
async fn exact_stagger_tcp_fallback_and_three_second_timeout() {
    let fake = Arc::new(FakeDialer::default());
    let dialer = fake.clone();
    let start = Instant::now();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates: (1..=3).map(|p| candidate(p, PathKind::Manual)).collect(),
                ..DialPlan::default()
            },
            &*dialer,
            [0; 32],
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    assert_eq!(fake.count(), 1);
    task.step(24).await;
    assert_eq!(fake.count(), 1);
    task.step(1).await;
    assert_eq!(fake.count(), 2);
    task.step(25).await;
    assert_eq!(fake.count(), 3);
    task.step(199).await;
    assert_eq!(fake.count(), 3);
    task.step(1).await;
    assert_eq!(fake.count(), 4);
    task.step(25).await;
    task.step(25).await;
    let starts = fake.starts.lock().unwrap().clone();
    let times: Vec<_> = starts
        .iter()
        .map(|(_, _, at)| (*at - start).as_millis())
        .collect();
    assert_eq!(times, [0, 25, 50, 250, 275, 300]);
    assert!(starts[..3]
        .iter()
        .all(|(_, t, _)| *t == TransportKind::Quic));
    assert!(starts[3..].iter().all(|(_, t, _)| *t == TransportKind::Tcp));
    task.step(2699).await;
    assert!(!task.is_finished());
    task.step(1).await;
    let failure = task.await.unwrap().err().unwrap();
    assert_eq!(failure.attempts.len(), 6);
    assert!(failure
        .attempts
        .iter()
        .all(|a| a.result == AttemptResult::TimedOut));
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(Instant::now() - start, Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn only_explicit_derp_knowledge_extends_timeout_to_five_seconds() {
    let fake = Arc::new(FakeDialer::default());
    let dialer = fake.clone();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates: vec![candidate(1, PathKind::Tailnet)],
                derp_only: true,
                ..DialPlan::default()
            },
            &*dialer,
            [0; 32],
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(250).await;
    task.step(4749).await;
    assert!(!task.is_finished());
    task.step(1).await;
    assert!(task.await.unwrap().is_err());
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn total_timeout_records_candidates_that_have_not_reached_their_launch_slot() {
    let fake = FakeDialer::default();
    let opts = DialOpts {
        timeout: Duration::from_millis(40),
        ..DialOpts::default()
    };
    let failure = race(
        &DialPlan {
            candidates: (1..=4)
                .map(|port| candidate(port, PathKind::Manual))
                .collect(),
            ..DialPlan::default()
        },
        &fake,
        [0; 32],
        &opts,
    )
    .await
    .unwrap_err();
    assert_eq!(fake.count(), 2);
    assert!(failure
        .attempts
        .iter()
        .all(|attempt| attempt.result == AttemptResult::TimedOut));
    assert!(failure.attempts[4].started_at_us.is_none());
}

#[tokio::test(start_paused = true)]
async fn delayed_executor_poll_cannot_accept_a_handshake_after_total_timeout() {
    let p = pool(&[true]).await;
    let c = candidate(1, PathKind::Manual);
    let fake = Arc::new(FakeDialer::default());
    fake.success(&c, TransportKind::Quic, 3500, p.conns[0].clone());
    let expected = p.expected;
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates: vec![c],
                ..DialPlan::default()
            },
            &*fake,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(3500).await;
    let failure = task.await.unwrap().unwrap_err();
    assert_eq!(failure.attempts[0].result, AttemptResult::TimedOut);
    assert!(is_closed(&p.conns[0]).await);
}

#[tokio::test(start_paused = true)]
async fn grace_selects_higher_priority_keeps_one_different_type_and_closes_rest() {
    let p = pool(&[true, true, true, true]).await;
    let candidates = vec![
        candidate(1, PathKind::LastGood(Box::new(PathKind::Lan))),
        candidate(2, PathKind::Lan),
        candidate(3, PathKind::Tailnet),
        candidate(4, PathKind::Manual),
    ];
    let fake = Arc::new(FakeDialer::default());
    for (i, delay) in [90, 60, 30, 20].into_iter().enumerate() {
        fake.success(
            &candidates[i],
            TransportKind::Quic,
            delay,
            p.conns[i].clone(),
        );
    }
    let expected = p.expected;
    let dialer = fake.clone();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates,
                ..DialPlan::default()
            },
            &*dialer,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    for _ in 0..21 {
        task.step(5).await;
    }
    task.step(4).await;
    assert!(!task.is_finished());
    task.step(1).await;
    let outcome = task.await.unwrap().ok().unwrap();
    assert_eq!(outcome.primary_path.addr.port(), 1);
    assert_eq!(outcome.backup.as_ref().unwrap().1.addr.port(), 3);
    assert_eq!(fake.count(), 4); // Authenticated QUIC suppresses TCP entirely.
    assert!(!is_closed(&p.conns[0]).await);
    assert!(is_closed(&p.conns[1]).await);
    assert!(!is_closed(&p.conns[2]).await);
    assert!(is_closed(&p.conns[3]).await);
    // Primary and backup are still prewarmed: neither opened a control stream.
    assert!(
        tokio::time::timeout(Duration::from_millis(1), p.peers[0].accept_control())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(1), p.peers[2].accept_control())
            .await
            .is_err()
    );
    close_outcome(&outcome);
}

#[tokio::test(start_paused = true)]
async fn a_later_high_priority_connection_outside_grace_is_cancelled() {
    let p = pool(&[true, true]).await;
    let candidates = vec![candidate(1, PathKind::Lan), candidate(2, PathKind::Tailnet)];
    let fake = Arc::new(FakeDialer::default());
    fake.success(&candidates[0], TransportKind::Quic, 100, p.conns[0].clone());
    fake.success(&candidates[1], TransportKind::Quic, 5, p.conns[1].clone());
    let expected = p.expected;
    let dialer = fake.clone();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates,
                ..DialPlan::default()
            },
            &*dialer,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(25).await;
    task.step(5).await;
    task.step(29).await;
    assert!(!task.is_finished());
    task.step(1).await;
    let outcome = task.await.unwrap().ok().unwrap();
    assert_eq!(outcome.primary_path.addr.port(), 2);
    assert!(outcome.backup.is_none());
    assert!(is_closed(&p.conns[0]).await);
    assert_eq!(outcome.attempts[0].result, AttemptResult::Cancelled);
    close_outcome(&outcome);
}

#[tokio::test(start_paused = true)]
async fn equal_priority_preserves_first_completion_without_same_type_backup() {
    let p = pool(&[true, true]).await;
    let candidates = vec![candidate(1, PathKind::Lan), candidate(2, PathKind::Lan)];
    let fake = Arc::new(FakeDialer::default());
    fake.success(&candidates[0], TransportKind::Quic, 50, p.conns[0].clone());
    fake.success(&candidates[1], TransportKind::Quic, 5, p.conns[1].clone());
    let expected = p.expected;
    let dialer = fake.clone();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates,
                ..DialPlan::default()
            },
            &*dialer,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(25).await;
    task.step(5).await;
    task.step(20).await;
    task.step(10).await;
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome.primary_path.addr.port(), 2);
    assert!(outcome.backup.is_none());
    assert!(is_closed(&p.conns[0]).await);
    close_outcome(&outcome);
}

#[tokio::test(start_paused = true)]
async fn higher_priority_completion_on_grace_boundary_wins() {
    let p = pool(&[true, true]).await;
    let candidates = vec![candidate(1, PathKind::Lan), candidate(2, PathKind::Tailnet)];
    let fake = Arc::new(FakeDialer::default());
    fake.success(&candidates[0], TransportKind::Quic, 60, p.conns[0].clone());
    fake.success(&candidates[1], TransportKind::Quic, 5, p.conns[1].clone());
    let expected = p.expected;
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates,
                ..DialPlan::default()
            },
            &*fake,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(25).await;
    task.step(5).await;
    task.step(30).await;
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome.primary_path.addr.port(), 1);
    assert_eq!(outcome.backup.as_ref().unwrap().1.addr.port(), 2);
    close_outcome(&outcome);
}

#[tokio::test(start_paused = true)]
async fn dropping_race_closes_ready_connections_and_cancels_every_handshake() {
    let p = pool(&[true, true]).await;
    let candidates = vec![candidate(1, PathKind::Lan), candidate(2, PathKind::Tailnet)];
    let fake = FakeDialer::default();
    fake.success(&candidates[0], TransportKind::Quic, 40, p.conns[0].clone());
    fake.success(
        &candidates[1],
        TransportKind::Quic,
        1000,
        p.conns[1].clone(),
    );
    let dial_plan = DialPlan {
        candidates,
        ..DialPlan::default()
    };
    let opts = DialOpts::default();
    let mut future = Box::pin(race(&dial_plan, &fake, p.expected, &opts));
    // Poll directly, so this checks dropping the actual race future.
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
    step(25).await;
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
    step(15).await;
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
    assert_eq!(fake.active.load(Ordering::SeqCst), 1);
    assert!(!is_closed(&p.conns[0]).await);
    drop(future);
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert!(is_closed(&p.conns[0]).await);
    assert!(is_closed(&p.conns[1]).await);
}

#[tokio::test(start_paused = true)]
async fn failures_keep_tls_refusal_timeout_and_skipped_results_separate() {
    let fake = FakeDialer::default();
    let c = candidate(1, PathKind::Manual);
    fake.add(
        &c,
        TransportKind::Quic,
        0,
        Reply::Error(ConnError::Tls("bad certificate".into())),
    );
    fake.add(
        &c,
        TransportKind::Tcp,
        0,
        Reply::Error(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into()),
    );
    let failure = race(
        &DialPlan {
            candidates: vec![c],
            skipped: vec![(candidate(2, PathKind::Intranet), Skipped::NeedsVpn)],
            ..DialPlan::default()
        },
        &fake,
        [0; 32],
        &DialOpts::default(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(failure.attempts[0].result, AttemptResult::TlsFailed);
    assert_eq!(failure.attempts[1].result, AttemptResult::Refused);
    assert_eq!(
        failure.attempts[2].result,
        AttemptResult::Skipped(Skipped::NeedsVpn)
    );
    assert!(failure.attempts[0].started_at_us.unwrap() > 0);
    assert!(failure.attempts[0].finished_at_us >= failure.attempts[0].started_at_us);
    assert!(failure.attempts[2].started_at_us.is_none());
}

#[tokio::test(start_paused = true)]
async fn skipped_only_plan_never_calls_dialer() {
    let fake = FakeDialer::default();
    let failure = race(
        &DialPlan {
            skipped: vec![(candidate(1, PathKind::Intranet), Skipped::NeedsVpn)],
            ..DialPlan::default()
        },
        &fake,
        [0; 32],
        &DialOpts::default(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(failure.attempts.len(), 1);
    assert_eq!(fake.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn fingerprint_mismatch_never_enters_grace_or_suppresses_tcp() {
    let p = pool(&[false]).await;
    let c = candidate(1, PathKind::Manual);
    let fake = Arc::new(FakeDialer::default());
    fake.success(&c, TransportKind::Quic, 0, p.conns[0].clone());
    fake.add(
        &c,
        TransportKind::Tcp,
        0,
        Reply::Error(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into()),
    );
    let dialer = fake.clone();
    let expected = p.expected;
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates: vec![c],
                ..DialPlan::default()
            },
            &*dialer,
            expected,
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(249).await;
    assert!(!task.is_finished());
    assert_eq!(fake.count(), 1);
    assert!(is_closed(&p.conns[0]).await);
    task.step(1).await;
    assert_eq!(fake.count(), 2);
    let failure = task.await.unwrap().err().unwrap();
    assert!(matches!(
        failure.attempts[0].result,
        AttemptResult::FingerprintMismatch { .. }
    ));
    assert_eq!(
        classify_failure(&Observations {
            failure: Some(failure),
            ..Observations::default()
        }),
        ErrorCode::FingerprintMismatch
    );
}

#[tokio::test]
async fn real_same_address_replaced_host_reports_e150() {
    let host_a = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let host_b = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = QuicListener::bind(local(), server_config(&host_b).unwrap())
        .await
        .unwrap();
    let c = Candidate {
        addr: listener.local_addr().unwrap(),
        ..candidate(0, PathKind::Manual)
    };
    let dialer = DefaultDialer::new(quinn::Endpoint::client(local()).unwrap(), &app).unwrap();
    let accept = async {
        let peer = listener.accept().await.unwrap().accept().await.unwrap();
        bounded(peer.closed()).await;
    };
    let (result, ()) = bounded(async {
        tokio::join!(
            async {
                race(
                    &DialPlan {
                        candidates: vec![c],
                        ..DialPlan::default()
                    },
                    &dialer,
                    host_a.fingerprint(),
                    &DialOpts::default(),
                )
                .await
            },
            accept
        )
    })
    .await;
    let failure = result.err().unwrap();
    assert!(
        matches!(failure.attempts[0].result, AttemptResult::FingerprintMismatch { got } if got == host_b.fingerprint())
    );
    assert_eq!(
        classify_failure(&Observations {
            failure: Some(failure),
            ..Observations::default()
        }),
        ErrorCode::FingerprintMismatch
    );
}

#[tokio::test]
async fn real_two_quic_hosts_selects_only_the_matching_identity() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let wrong = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let a = QuicListener::bind(local(), server_config(&wrong).unwrap())
        .await
        .unwrap();
    let b = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let wrong_addr = a.local_addr().unwrap();
    let correct_addr = b.local_addr().unwrap();
    let dialer = DefaultDialer::new(quinn::Endpoint::client(local()).unwrap(), &app).unwrap();
    let dial_plan = DialPlan {
        candidates: vec![
            Candidate {
                addr: wrong_addr,
                ..candidate(0, PathKind::Lan)
            },
            Candidate {
                addr: correct_addr,
                ..candidate(0, PathKind::Tailnet)
            },
        ],
        ..DialPlan::default()
    };
    let accept_a = async { a.accept().await.unwrap().accept().await.unwrap() };
    let accept_b = async { b.accept().await.unwrap().accept().await.unwrap() };
    let (outcome, peer_a, peer_b) = bounded(async {
        tokio::join!(
            async {
                race(
                    &dial_plan,
                    &dialer,
                    host.fingerprint(),
                    &DialOpts::default(),
                )
                .await
            },
            accept_a,
            accept_b
        )
    })
    .await;
    let outcome = outcome.ok().unwrap();
    assert_eq!(outcome.primary_path.addr, correct_addr);
    assert!(outcome.backup.is_none());
    bounded(peer_a.closed()).await;
    close_outcome(&outcome);
    bounded(peer_b.closed()).await;
}

#[tokio::test]
async fn real_tcp_succeeds_after_250ms_when_udp_is_unreachable() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    // Hold an unread UDP socket at the TCP address: UDP deterministically
    // blackholes without involving any external network or ICMP timing.
    let socket = tokio::net::TcpListener::bind(local()).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let _udp = tokio::net::UdpSocket::bind(addr).await.unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&host).unwrap()));
    let dialer = DefaultDialer::new(quinn::Endpoint::client(local()).unwrap(), &app).unwrap();
    let start = Instant::now();
    let accept = async {
        let (stream, _) = socket.accept().await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(250));
        acceptor.accept(stream).await.unwrap()
    };
    let (outcome, mut peer) = bounded(async {
        tokio::join!(
            async {
                race(
                    &DialPlan {
                        candidates: vec![Candidate {
                            addr,
                            ..candidate(0, PathKind::Manual)
                        }],
                        ..DialPlan::default()
                    },
                    &dialer,
                    host.fingerprint(),
                    &DialOpts::default(),
                )
                .await
            },
            accept
        )
    })
    .await;
    let outcome = outcome.ok().unwrap();
    assert_eq!(outcome.transport, TransportKind::Tcp);
    assert_eq!(outcome.attempts[0].result, AttemptResult::Cancelled);
    assert!(start.elapsed() >= Duration::from_millis(280));
    use tokio::io::AsyncReadExt;
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(5), peer.read(&mut byte))
            .await
            .is_err()
    );
    close_outcome(&outcome);
    assert_eq!(bounded(peer.read(&mut byte)).await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn same_path_quic_and_tcp_are_valid_distinct_backups() {
    let (unblock, wait) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || wait.recv().unwrap());
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let socket = tokio::net::TcpListener::bind(local()).await.unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&host).unwrap()));
    let (tcp, _peer) = bounded(async {
        tokio::join!(
            connect_tcp(
                socket.local_addr().unwrap(),
                Arc::new(client_config(&app, ServerPin::Any).unwrap())
            ),
            async {
                acceptor
                    .accept(socket.accept().await.unwrap().0)
                    .await
                    .unwrap()
            }
        )
    })
    .await;
    let tcp = tcp.unwrap();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let (quic, peer) = bounded(async {
        tokio::join!(
            connect_quic(
                &endpoint,
                listener.local_addr().unwrap(),
                Arc::new(client_config(&app, ServerPin::Any).unwrap()),
                true
            ),
            async { listener.accept().await.unwrap().accept().await.unwrap() }
        )
    })
    .await;
    let quic = quic.unwrap();
    unblock.send(()).unwrap();
    blocker.await.unwrap();
    let fake = Arc::new(FakeDialer::default());
    let c = candidate(1, PathKind::Manual);
    fake.success(&c, TransportKind::Quic, 260, quic.clone());
    fake.success(&c, TransportKind::Tcp, 20, tcp.clone());
    let dialer = fake.clone();
    let task = TestTask::spawn(async move {
        race(
            &DialPlan {
                candidates: vec![c],
                ..DialPlan::default()
            },
            &*dialer,
            host.fingerprint(),
            &DialOpts::default(),
        )
        .await
    });
    task.step(0).await;
    task.step(250).await;
    task.step(10).await;
    task.step(10).await;
    task.step(20).await;
    let outcome = task.await.unwrap().ok().unwrap();
    assert_eq!(
        outcome.transport,
        TransportKind::Quic,
        "{:#?}",
        outcome.attempts
    );
    assert_eq!(
        outcome.backup.as_ref().unwrap().0.kind(),
        TransportKind::Tcp
    );
    assert!(!is_closed(&tcp).await);
    close_outcome(&outcome);
    peer.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn real_drop_cancels_quic_and_tcp_handshakes_without_leaking_sockets() {
    use tokio::io::AsyncReadExt;

    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = tokio::net::TcpListener::bind(local()).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _udp = tokio::net::UdpSocket::bind(addr).await.unwrap();
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let dialer = DefaultDialer::new(endpoint.clone(), &app).unwrap();
    let dial_plan = DialPlan {
        candidates: vec![Candidate {
            addr,
            ..candidate(0, PathKind::Manual)
        }],
        ..DialPlan::default()
    };
    let opts = DialOpts::default();
    let mut future = Box::pin(race(&dial_plan, &dialer, [0; 32], &opts));
    // The TCP peer deliberately never answers ClientHello. Both handshakes are
    // in flight when the owner drops the race future.
    let (mut peer, _) = bounded(async {
        tokio::select! {
            result = &mut future => panic!("race finished before cancellation: {:?}", result.err()),
            accepted = listener.accept() => accepted.unwrap(),
        }
    })
    .await;
    assert!(endpoint.open_connections() > 0);
    drop(future);
    let mut bytes = [0; 4096];
    bounded(async {
        loop {
            match peer.read(&mut bytes).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(error) => panic!("unexpected socket error: {error}"),
            }
        }
    })
    .await;
    bounded(endpoint.wait_idle()).await;
    assert_eq!(endpoint.open_connections(), 0);
}

#[tokio::test]
async fn real_tcp_host_fingerprint_is_checked_too() {
    let host_a = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let host_b = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = tokio::net::TcpListener::bind(local()).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _udp = tokio::net::UdpSocket::bind(addr).await.unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&host_b).unwrap()));
    let dialer = DefaultDialer::new(quinn::Endpoint::client(local()).unwrap(), &app).unwrap();
    let accept = async {
        acceptor
            .accept(listener.accept().await.unwrap().0)
            .await
            .unwrap()
    };
    let (result, mut peer) = bounded(async {
        tokio::join!(
            async {
                race(
                    &DialPlan {
                        candidates: vec![Candidate {
                            addr,
                            ..candidate(0, PathKind::Manual)
                        }],
                        ..DialPlan::default()
                    },
                    &dialer,
                    host_a.fingerprint(),
                    &DialOpts::default(),
                )
                .await
            },
            accept
        )
    })
    .await;
    let failure = result.err().unwrap();
    assert_eq!(failure.attempts[0].result, AttemptResult::TimedOut);
    assert!(
        matches!(failure.attempts[1].result, AttemptResult::FingerprintMismatch { got } if got == host_b.fingerprint())
    );
    assert_eq!(
        classify_failure(&Observations {
            failure: Some(failure),
            ..Observations::default()
        }),
        ErrorCode::FingerprintMismatch
    );
    use tokio::io::AsyncReadExt;
    assert_eq!(bounded(peer.read(&mut [0])).await.unwrap(), 0);
}

fn observation(kind: PathKind, quic: AttemptResult, tcp: AttemptResult) -> Observations {
    let c = candidate(1, kind);
    Observations {
        failure: Some(DialFailure {
            attempts: [(TransportKind::Quic, quic), (TransportKind::Tcp, tcp)]
                .into_iter()
                .map(|(transport, result)| AttemptLog {
                    candidate: c.clone(),
                    transport: Some(transport),
                    result,
                    started_at_us: Some(1),
                    finished_at_us: Some(2),
                })
                .collect(),
        }),
        plan: DialPlan {
            candidates: vec![c],
            ..DialPlan::default()
        },
        ..Observations::default()
    }
}

#[test]
fn classification_only_intranet_without_vpn_is_e160() {
    let obs = observation(
        PathKind::Intranet,
        AttemptResult::Skipped(Skipped::NeedsVpn),
        AttemptResult::NotStarted,
    );
    assert_eq!(classify_failure(&obs), ErrorCode::VpnMissing);
}

#[test]
fn classification_routed_intranet_dual_timeout_is_e161_with_a_local_route() {
    let mut obs = observation(
        PathKind::Intranet,
        AttemptResult::TimedOut,
        AttemptResult::TimedOut,
    );
    obs.local = iface("127.0.0.1/8", false);
    assert_eq!(classify_failure(&obs), ErrorCode::PortBlocked);
    obs.local = iface("10.99.0.2/24", true);
    assert_eq!(classify_failure(&obs), ErrorCode::PortBlocked);
}

#[test]
fn classification_every_completed_handshake_wrong_is_e150_over_timeouts() {
    let obs = observation(
        PathKind::Tailnet,
        AttemptResult::FingerprintMismatch { got: [1; 32] },
        AttemptResult::TimedOut,
    );
    assert_eq!(classify_failure(&obs), ErrorCode::FingerprintMismatch);
}

#[test]
fn classification_version_is_e170_before_any_dial_and_overrides_other_failures() {
    let mut obs = Observations {
        host_version_ok: Some(false),
        ..Observations::default()
    };
    assert_eq!(classify_failure(&obs), ErrorCode::HostTooOld);
    obs.failure = observation(
        PathKind::Tailnet,
        AttemptResult::FingerprintMismatch { got: [1; 32] },
        AttemptResult::TimedOut,
    )
    .failure;
    assert_eq!(classify_failure(&obs), ErrorCode::HostTooOld);
}

#[test]
fn classification_does_not_invent_missing_evidence() {
    assert_eq!(classify_failure(&Observations::default()), ErrorCode::Other);
    let obs = observation(
        PathKind::Manual,
        AttemptResult::TlsFailed,
        AttemptResult::TimedOut,
    );
    assert_eq!(classify_failure(&obs), ErrorCode::Other);
    let obs = observation(
        PathKind::Manual,
        AttemptResult::FingerprintMismatch { got: [1; 32] },
        AttemptResult::Connected,
    );
    assert_ne!(classify_failure(&obs), ErrorCode::FingerprintMismatch);
}
