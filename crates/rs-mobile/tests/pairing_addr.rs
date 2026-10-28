mod support;

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::TestHost;
use rs_mobile::rscore::api_pairing::{cancel, paired, pairing_error, start, NOT_A_HOST};
use rs_mobile::rscore::dto_core::{PairOutcome, PairStartArg};
use rs_mobile::rscore::runtime::Core;
use rsp_core::ids::fp_b64url;
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::tls::{client_config, server_config, ServerPin};
use rsp_transport::{connect_quic, CloseCode, ControlStream, QuicListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, timeout, Instant};

use support::{test_core_with, with_identity, EventCollector};

async fn within(future: impl Future<Output = ()>) {
    timeout(Duration::from_secs(9), future)
        .await
        .expect("address pairing test completes within nine seconds");
}

fn configured_core(probe: Duration) -> Arc<Core> {
    let core = test_core_with(|_, opts| {
        opts.timeouts.pairing = Duration::from_secs(6);
        opts.timeouts.probe = probe;
        opts.clock = Some(Arc::new(|| 1_792_465_134_000_000));
    });
    with_identity(&core, "Address pairing phone");
    core
}

fn collect(core: &Arc<Core>) -> Arc<EventCollector> {
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    events
}

fn assert_done(core: &Arc<Core>, events: &EventCollector, outcome: &PairOutcome) {
    assert!(core.events().flush(Duration::from_secs(1)));
    let done = events.of_type("pairing.done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["outcome"], serde_json::to_value(outcome).unwrap());
    assert_eq!(events.events().last().unwrap().1["type"], "pairing.done");
}

fn steps(events: &EventCollector) -> Vec<String> {
    events
        .of_type("pairing.progress")
        .iter()
        .map(|event| event["step"].as_str().unwrap().to_owned())
        .collect()
}

fn by_addr(addr: SocketAddr, code: impl Into<String>) -> PairStartArg {
    PairStartArg::CodeAddr {
        addr: addr.ip().to_string(),
        port: Some(addr.port()),
        code: code.into(),
    }
}

fn wrong_code(code: &str) -> &'static str {
    if code == "000000" {
        "000001"
    } else {
        "000000"
    }
}

/// Terminates the App's TLS with its own Host certificate and reaches the real
/// Host on a second connection with its own device certificate, relaying
/// control frames unchanged in both directions.
struct Mitm {
    addr: SocketAddr,
    fp: [u8; 32],
    relayed: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Mitm {
    async fn start(upstream: SocketAddr) -> Self {
        let front = LocalIdentity::generate_software(CertKind::Host).unwrap();
        let back = LocalIdentity::generate_software(CertKind::App).unwrap();
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let listener = QuicListener::bind(loopback, server_config(&front).unwrap())
            .await
            .unwrap();
        let endpoint = quinn::Endpoint::client(loopback).unwrap();
        let tls = Arc::new(client_config(&back, ServerPin::Any).unwrap());
        let relayed = Arc::new(AtomicUsize::new(0));
        let count = relayed.clone();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut relays = JoinSet::new();
            while let Ok(incoming) = listener.accept().await {
                let (endpoint, tls, count) = (endpoint.clone(), tls.clone(), count.clone());
                relays.spawn(async move {
                    let Ok(app) = incoming.accept().await else {
                        return;
                    };
                    // Fingerprint discovery closes right after TLS, opening no stream.
                    let Ok(app_control) = app.accept_control().await else {
                        return;
                    };
                    let Ok(host) = connect_quic(&endpoint, upstream, tls, false).await else {
                        return;
                    };
                    let Ok(host_control) = host.open_control().await else {
                        return;
                    };
                    tokio::select! {
                        _ = pump(&app_control, &host_control, &count) => {}
                        _ = pump(&host_control, &app_control, &count) => {
                            let _ = app_control.finish().await;
                        }
                    }
                    let _ = timeout(Duration::from_secs(2), app.closed()).await;
                    host.close(CloseCode::Normal, b"relay done");
                    app.close(CloseCode::Normal, b"relay done");
                });
            }
        });
        Self {
            addr,
            fp: front.fingerprint(),
            relayed,
            task,
        }
    }
}

impl Drop for Mitm {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn pump(from: &ControlStream, to: &ControlStream, count: &AtomicUsize) {
    while let Ok(Some(frame)) = from.recv().await {
        count.fetch_add(1, Ordering::Relaxed);
        if to.send(&frame).await.is_err() {
            break;
        }
    }
}

/// A plain TCP echo service: it accepts connections but speaks no TLS.
async fn echo_server() -> (SocketAddr, Arc<AtomicUsize>, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let echoed = Arc::new(AtomicUsize::new(0));
    let count = echoed.clone();
    let task = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        while let Ok((mut stream, _)) = listener.accept().await {
            let count = count.clone();
            clients.spawn(async move {
                let mut buf = [0; 4096];
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    count.fetch_add(n, Ordering::Relaxed);
                    if stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, echoed, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn correct_code_pairs_with_the_fingerprint_presented_at_the_address() {
    within(async {
        let host = TestHost::start().await;
        let core = configured_core(Duration::from_secs(1));
        let events = collect(&core);
        let outcome = start(
            &core,
            PairStartArg::CodeAddr {
                addr: " 127.0.0.1 ".to_owned(),
                port: Some(host.quic_addr.port()),
                code: host.new_pairing_code().code,
            },
        )
        .await
        .unwrap();
        let PairOutcome::Ok {
            host_id, host_name, ..
        } = &outcome
        else {
            panic!("address pairing failed: {outcome:?}");
        };
        assert_eq!(host_id, &host.host_id);
        assert_done(&core, &events, &outcome);
        assert_eq!(
            steps(&events),
            ["dialing", "verifying", "exchanging", "waitingLocal"]
        );
        assert_eq!(
            events.of_type("pairing.progress")[1]["fp"],
            fp_b64url(&host.fp)
        );
        let hosts = paired(&core).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host_id, host.host_id);
        assert_eq!(hosts[0].fp, fp_b64url(&host.fp));
        assert_eq!(&hosts[0].name, host_name);
        assert_eq!(hosts[0].paired_at, core.now_us());
        assert_eq!(hosts[0].addresses, vec![host.quic_addr.to_string()]);
        assert_eq!(host.security.store.paired().unwrap().len(), 1);
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_code_saves_nothing() {
    within(async {
        let host = TestHost::start().await;
        let core = configured_core(Duration::from_secs(1));
        let events = collect(&core);
        let ticket = host.new_pairing_code();
        let outcome = start(&core, by_addr(host.quic_addr, wrong_code(&ticket.code)))
            .await
            .unwrap();
        assert_eq!(outcome, PairOutcome::BadCode { attempts_left: 4 });
        assert_done(&core, &events, &outcome);
        assert!(paired(&core).unwrap().is_empty());
        assert!(host.security.store.paired().unwrap().is_empty());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relaying_proxy_with_its_own_certificate_fails_confirmation() {
    within(async {
        let host = TestHost::start().await;
        let mitm = Mitm::start(host.quic_addr).await;
        let core = configured_core(Duration::from_secs(1));
        let events = collect(&core);
        let ticket = host.new_pairing_code();
        let outcome = start(&core, by_addr(mitm.addr, ticket.code.clone()))
            .await
            .unwrap();
        // The App's transcript binds the proxy's certificate and the Host's
        // binds the proxy's device certificate, so the Host rejects the App's
        // key confirmation even though the code is right.
        assert_eq!(outcome, PairOutcome::BadCode { attempts_left: 4 });
        assert_done(&core, &events, &outcome);
        assert_eq!(
            events.of_type("pairing.progress")[1]["fp"],
            fp_b64url(&mitm.fp)
        );
        assert!(mitm.relayed.load(Ordering::Relaxed) > 0);
        assert!(paired(&core).unwrap().is_empty());
        assert!(host.security.store.paired().unwrap().is_empty());

        // The same code pairs directly, so only the relay broke confirmation.
        let outcome = start(&core, by_addr(host.quic_addr, ticket.code))
            .await
            .unwrap();
        assert!(matches!(outcome, PairOutcome::Ok { .. }), "{outcome:?}");
        assert_eq!(paired(&core).unwrap()[0].fp, fp_b64url(&host.fp));
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn address_without_an_rsp_host_is_not_a_remote_host() {
    within(async {
        let (echo, echoed, server) = echo_server().await;
        let core = configured_core(Duration::from_millis(800));
        let events = collect(&core);
        let outcome = start(&core, by_addr(echo, "123456")).await.unwrap();
        assert_eq!(outcome, pairing_error(NOT_A_HOST));
        assert_done(&core, &events, &outcome);
        assert_eq!(steps(&events), ["dialing"]);
        // TLS reached the echo service, which only reflected the ClientHello.
        assert!(echoed.load(Ordering::Relaxed) > 0);
        server.abort();

        for addr in ["a b", "   "] {
            let core = configured_core(Duration::from_millis(800));
            let outcome = start(
                &core,
                PairStartArg::CodeAddr {
                    addr: addr.to_owned(),
                    port: None,
                    code: "123456".to_owned(),
                },
            )
            .await
            .unwrap();
            assert_eq!(outcome, pairing_error(NOT_A_HOST));
            assert!(paired(&core).unwrap().is_empty());
        }

        let core = configured_core(Duration::from_millis(800));
        let events = collect(&core);
        let outcome = start(&core, by_addr(echo, "12345a")).await.unwrap();
        assert_eq!(
            outcome,
            pairing_error("pairing code must contain six ASCII digits")
        );
        assert_done(&core, &events, &outcome);
        assert!(steps(&events).is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_interrupts_fingerprint_discovery() {
    within(async {
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = silent.local_addr().unwrap();
        let core = configured_core(Duration::from_secs(5));
        let events = collect(&core);
        let other = core.clone();
        let task = tokio::spawn(async move { start(&other, by_addr(addr, "123456")).await });
        timeout(Duration::from_secs(1), async {
            while steps(&events).is_empty() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("address pairing reaches dialing");
        sleep(Duration::from_millis(200)).await;
        let cancelled_at = Instant::now();
        cancel(&core);
        let outcome = timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(cancelled_at.elapsed() < Duration::from_millis(500));
        assert_eq!(outcome, PairOutcome::Cancelled);
        assert_done(&core, &events, &outcome);
        assert_eq!(steps(&events), ["dialing"]);
        assert!(paired(&core).unwrap().is_empty());
    })
    .await;
}
