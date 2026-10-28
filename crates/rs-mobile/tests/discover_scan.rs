mod support;

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rs_mobile::rscore::api_discover::discover_scan_lan;
use rs_mobile::rscore::discover::scan::{scan_targets, start};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::netinfo::RawIface;
use rs_mobile::rscore::runtime::Core;
use rsp_transport::dial::IpNet;
use rsp_transport::{net::NetFuture, NetDialer, NetStream, SystemNetDialer};
use support::EventCollector;
use tokio::net::TcpListener;

fn iface(name: &str, addrs: &[&str]) -> RawIface {
    RawIface {
        name: name.to_owned(),
        addrs: addrs.iter().map(|addr| addr.parse().unwrap()).collect(),
    }
}

fn core_with_net(net: Arc<dyn NetDialer>) -> Arc<Core> {
    support::test_core_with(|_, options| {
        options.net = Some(net);
    })
}

async fn wait_for_done(collector: &EventCollector, timeout: Duration) -> Vec<serde_json::Value> {
    let started = Instant::now();
    loop {
        let done = collector.of_type("discover.done");
        if !done.is_empty() {
            return done;
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for discover.done"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[test]
fn scan_targets_excludes_network_broadcast_and_local_address() {
    let targets = scan_targets(
        &[iface("en0", &["192.168.31.20/24"])],
        &[],
        Some(Ipv4Addr::new(192, 168, 31, 20)),
    );
    assert_eq!(targets.len(), 253);
    assert!(!targets.contains(&Ipv4Addr::new(192, 168, 31, 0)));
    assert!(!targets.contains(&Ipv4Addr::new(192, 168, 31, 255)));
    assert!(!targets.contains(&Ipv4Addr::new(192, 168, 31, 20)));
    assert_eq!(targets.first(), Some(&Ipv4Addr::new(192, 168, 31, 1)));
    assert_eq!(targets.last(), Some(&Ipv4Addr::new(192, 168, 31, 254)));
}

#[test]
fn scan_targets_narrows_wide_prefix_to_local_24() {
    let targets = scan_targets(
        &[iface("en0", &["10.30.0.1/16"])],
        &[],
        Some(Ipv4Addr::new(10, 30, 7, 20)),
    );
    assert_eq!(targets.len(), 253);
    assert_eq!(targets.first(), Some(&Ipv4Addr::new(10, 30, 7, 1)));
    assert_eq!(targets.last(), Some(&Ipv4Addr::new(10, 30, 7, 254)));
    assert!(targets.iter().all(|address| address.octets()[2] == 7));
}

#[test]
fn scan_targets_excludes_tailnet_vpn_and_deduplicates_interfaces() {
    let vpn_pool: IpNet = "10.99.0.0/16".parse().unwrap();
    let targets = scan_targets(
        &[
            iface("en0", &["192.168.31.20/24"]),
            iface("en1", &["192.168.31.30/24"]),
            iface("utun0", &["100.64.0.7/24"]),
            iface("en2", &["10.99.7.20/24"]),
        ],
        &[vpn_pool],
        None::<Ipv4Addr>,
    );
    assert_eq!(targets.len(), 252);
    assert!(!targets.contains(&Ipv4Addr::new(100, 64, 0, 1)));
    assert!(!targets.contains(&Ipv4Addr::new(10, 99, 7, 1)));
    assert!(!targets.contains(&Ipv4Addr::new(192, 168, 31, 20)));
    assert!(!targets.contains(&Ipv4Addr::new(192, 168, 31, 30)));
}

#[tokio::test]
async fn scan_emits_open_results_then_done() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let accept = tokio::spawn(async move {
        let _ = listener.accept().await;
    });
    let core = core_with_net(Arc::new(SystemNetDialer));
    let collector = EventCollector::new();
    core.events().subscribe(collector.clone());

    let scan_id = start(&core, vec![address.port()], vec![Ipv4Addr::LOCALHOST])
        .await
        .unwrap();
    let done = wait_for_done(&collector, Duration::from_secs(2)).await;
    assert_eq!(done[0]["scanId"], scan_id.to_string());
    assert_eq!(done[0]["scanned"], 1);
    assert_eq!(done[0]["cancelled"], false);
    let found = collector.of_type("discover.found");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["addr"], address.ip().to_string());
    assert_eq!(found[0]["port"], address.port());
    accept.await.unwrap();
}

#[derive(Debug, Default)]
struct PendingNet {
    calls: Mutex<Vec<(String, u16)>>,
}

impl NetDialer for PendingNet {
    fn shared_quic_endpoint(&self, _: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        panic!("scan must not create a QUIC endpoint")
    }

    fn dial_udp(&self, _: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        panic!("scan must not dial UDP")
    }

    fn dial_tcp(&self, _: SocketAddr) -> NetFuture<'_, NetStream> {
        panic!("scan must use host dialing")
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.calls.lock().unwrap().push((host.to_owned(), port));
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn cancel_finishes_quickly_and_replacing_scan_cancels_previous() {
    let net = Arc::new(PendingNet::default());
    let core = core_with_net(net);
    let collector = EventCollector::new();
    core.events().subscribe(collector.clone());

    let first = start(
        &core,
        vec![1234],
        vec![Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)],
    )
    .await
    .unwrap();
    let started = Instant::now();
    rs_mobile::rscore::discover::scan::cancel(&core, first);
    let done = wait_for_done(&collector, Duration::from_millis(500)).await;
    let first_id = first.to_string();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(done[0]["scanId"].as_str(), Some(first_id.as_str()));
    assert_eq!(done[0]["cancelled"], true);

    let second = start(&core, vec![1234], vec![]).await.unwrap();
    let second_id = second.to_string();
    let started = Instant::now();
    let done = loop {
        let done = collector.of_type("discover.done");
        if done
            .iter()
            .any(|event| event["scanId"].as_str() == Some(second_id.as_str()))
        {
            break done;
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(done.iter().any(|event| {
        event["scanId"].as_str() == Some(second_id.as_str()) && event["cancelled"] == false
    }));
}

#[tokio::test]
async fn starting_new_scan_cancels_active_scan() {
    let core = core_with_net(Arc::new(PendingNet::default()));
    let collector = EventCollector::new();
    core.events().subscribe(collector.clone());
    let first = start(&core, vec![1234], vec![Ipv4Addr::new(192, 0, 2, 1)])
        .await
        .unwrap();
    let second = start(&core, vec![1234], vec![]).await.unwrap();
    let first_id = first.to_string();
    let second_id = second.to_string();
    let done = wait_for_done(&collector, Duration::from_millis(500)).await;
    assert!(done.iter().any(|event| {
        event["scanId"].as_str() == Some(first_id.as_str()) && event["cancelled"] == true
    }));
    assert!(done.iter().any(|event| {
        event["scanId"].as_str() == Some(second_id.as_str()) && event["cancelled"] == false
    }));
}

#[tokio::test]
async fn bad_ports_are_rejected_before_core_lookup() {
    for ports in [Vec::new(), vec![1, 2, 3, 4, 5]] {
        assert_eq!(
            discover_scan_lan(ports).await,
            Err(CoreError::Internal {
                message: "bad ports".to_owned()
            })
        );
    }
}
