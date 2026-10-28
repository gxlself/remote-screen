use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, TcpMode, TcpRule, UdpRule};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, timeout, Instant};

pub const WAIT: Duration = Duration::from_millis(800);

pub struct Seen {
    pub source: SocketAddr,
    pub data: Vec<u8>,
    pub at: Instant,
}

pub struct Echo {
    pub udp: SocketAddr,
    pub tcp: SocketAddr,
    pub seen: mpsc::UnboundedReceiver<Seen>,
    pub tasks: Vec<JoinHandle<()>>,
}

impl Echo {
    pub async fn start() -> Self {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let udp_addr = udp.local_addr().unwrap();
        let tcp_addr = tcp.local_addr().unwrap();
        let (tx, seen) = mpsc::unbounded_channel();
        let udp_task = tokio::spawn(async move {
            let mut buffer = vec![0; 65_536];
            while let Ok((size, source)) = udp.recv_from(&mut buffer).await {
                tx.send(Seen {
                    source,
                    data: buffer[..size].to_vec(),
                    at: Instant::now(),
                })
                .unwrap();
                udp.send_to(&buffer[..size], source).await.unwrap();
            }
        });
        let tcp_task = tokio::spawn(async move {
            let mut streams = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = tcp.accept() => {
                        let Ok((mut stream, _)) = accepted else { break };
                        streams.spawn(async move {
                            let mut buffer = vec![0; 16 * 1024];
                            loop {
                                let size = stream.read(&mut buffer).await.unwrap();
                                if size == 0 { break; }
                                if stream.write_all(&buffer[..size]).await.is_err() { break; }
                            }
                        });
                    }
                    Some(_) = streams.join_next(), if !streams.is_empty() => {}
                }
            }
        });
        Self {
            udp: udp_addr,
            tcp: tcp_addr,
            seen,
            tasks: vec![udp_task, tcp_task],
        }
    }

    pub async fn proxy(&self, seed: u64) -> FaultProxy {
        FaultProxy::start(self.udp, Some(self.tcp), seed)
            .await
            .unwrap()
    }

    pub async fn next(&mut self) -> Seen {
        timeout(WAIT, self.seen.recv()).await.unwrap().unwrap()
    }
}

impl Drop for Echo {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub async fn client(proxy: &FaultProxy) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(proxy.addr()).await.unwrap();
    socket
}

pub async fn recv(socket: &UdpSocket) -> Vec<u8> {
    let mut data = vec![0; 65_536];
    let size = timeout(WAIT, socket.recv(&mut data))
        .await
        .unwrap()
        .unwrap();
    data.truncate(size);
    data
}

pub async fn wait_for(mut predicate: impl FnMut() -> bool) {
    timeout(WAIT, async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

pub async fn send_number(socket: &UdpSocket, number: u32) {
    socket.send(&number.to_be_bytes()).await.unwrap();
}

pub fn number(data: &[u8]) -> u32 {
    u32::from_be_bytes(data.try_into().unwrap())
}

#[tokio::test]
pub async fn passthrough_same_port_and_per_client_nat() {
    let mut echo = Echo::start().await;
    assert_ne!(echo.udp.port(), echo.tcp.port());
    let proxy = echo.proxy(1).await;
    let first = client(&proxy).await;
    for id in 0..100 {
        send_number(&first, id).await;
        assert_eq!(number(&recv(&first).await), id);
    }
    assert_eq!(proxy.stats().to_host.forwarded, 100);
    assert_eq!(proxy.stats().udp_clients, 1);
    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
    stream.write_all(b"same port").await.unwrap();
    let mut bytes = [0; 9];
    timeout(WAIT, stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"same port");
    assert_eq!(proxy.stats().tcp_accepted, 1);

    let second = client(&proxy).await;
    first.send(b"first").await.unwrap();
    second.send(b"second").await.unwrap();
    assert_eq!(recv(&first).await, b"first");
    assert_eq!(recv(&second).await, b"second");
    assert_eq!(proxy.stats().udp_clients, 2);
    let first_source = echo.next().await.source;
    for _ in 1..100 {
        echo.next().await;
    }
    let one = echo.next().await;
    let two = echo.next().await;
    assert_eq!(one.data, b"first");
    assert_eq!(two.data, b"second");
    assert_eq!(first_source, one.source);
    assert_ne!(one.source, two.source);
    wait_for(|| proxy.stats().tcp_bytes_to_client == 9).await;
    assert_eq!(proxy.stats().tcp_bytes_to_host, 9);
    proxy.shutdown().await;
}

pub async fn loss_run() -> BTreeSet<u32> {
    let mut echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            loss_pct: 20.0,
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    for id in 0..1000 {
        send_number(&socket, id).await;
        // Pace at the proxy's receive boundary; kernel queue overflow is not
        // the fault under test, and must not change the seeded input sequence.
        wait_for(|| proxy.stats().to_host.received > u64::from(id)).await;
    }
    wait_for(|| {
        let stats = proxy.stats().to_host;
        stats.forwarded + stats.dropped == 1000
    })
    .await;
    let stats = proxy.stats();
    assert!((150..=250).contains(&stats.to_host.dropped), "{stats:?}");
    let mut missing: BTreeSet<_> = (0..1000).collect();
    for _ in 0..stats.to_host.forwarded {
        assert!(missing.remove(&number(&echo.next().await.data)));
    }
    assert_eq!(missing.len() as u64, stats.to_host.dropped);
    proxy.shutdown().await;
    missing
}

#[tokio::test]
pub async fn seeded_loss_is_reproducible() {
    let first = loss_run().await;
    let second = loss_run().await;
    assert_eq!(first, second);
}

#[tokio::test]
pub async fn burst_drops_three_consecutive_packets_in_each_ten() {
    let mut echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            burst: Some((10, 3)),
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    for id in 0..100 {
        send_number(&socket, id).await;
        wait_for(|| proxy.stats().to_host.received > u64::from(id)).await;
    }
    wait_for(|| proxy.stats().to_host.forwarded == 70).await;
    assert_eq!(proxy.stats().to_host.dropped, 30);
    let mut got = BTreeSet::new();
    for _ in 0..70 {
        got.insert(number(&echo.next().await.data));
    }
    assert_eq!(got, (0..100).filter(|id| id % 10 >= 3).collect());
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn blackhole_and_restore() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(1).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            blackhole: true,
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    socket.send(b"lost").await.unwrap();
    let mut buffer = [0; 32];
    assert!(
        timeout(Duration::from_millis(300), socket.recv(&mut buffer))
            .await
            .is_err()
    );
    assert_eq!(proxy.stats().to_host.dropped, 1);
    proxy.restore();
    socket.send(b"restored").await.unwrap();
    assert_eq!(recv(&socket).await, b"restored");
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn fixed_delay_and_parallel_jitter() {
    let mut echo = Echo::start().await;
    let proxy = echo.proxy(9).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 80,
            ..UdpRule::default()
        },
    );
    let sent = Instant::now();
    socket.send(b"delay").await.unwrap();
    assert_eq!(recv(&socket).await, b"delay");
    assert!(sent.elapsed() >= Duration::from_millis(80));
    echo.next().await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 50,
            jitter_ms: 30,
            ..UdpRule::default()
        },
    );
    let mut sent = Vec::new();
    for id in 0..50 {
        sent.push(Instant::now());
        send_number(&socket, id).await;
    }
    let mut delays = Vec::new();
    for _ in 0..50 {
        let packet = echo.next().await;
        let delay = packet.at - sent[number(&packet.data) as usize];
        assert!(
            (Duration::from_millis(20)..=Duration::from_millis(200)).contains(&delay),
            "{delay:?}"
        );
        delays.push(delay);
    }
    assert!(
        *delays.iter().max().unwrap() - *delays.iter().min().unwrap() >= Duration::from_millis(20)
    );
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn duplicate_reorder_and_corrupt() {
    let mut echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            duplicate_pct: 100.0,
            ..UdpRule::default()
        },
    );
    for id in 0..10 {
        send_number(&socket, id).await;
        assert_eq!(number(&recv(&socket).await), id);
        assert_eq!(number(&recv(&socket).await), id);
    }
    assert_eq!(proxy.stats().to_host.duplicated, 10);
    assert_eq!(proxy.stats().to_host.forwarded, 20);
    for _ in 0..20 {
        echo.next().await;
    }

    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            reorder_pct: 100.0,
            ..UdpRule::default()
        },
    );
    send_number(&socket, 1).await;
    send_number(&socket, 2).await;
    assert_eq!(number(&echo.next().await.data), 2);
    assert_eq!(number(&echo.next().await.data), 1);
    assert_eq!(number(&recv(&socket).await), 2);
    assert_eq!(number(&recv(&socket).await), 1);
    assert_eq!(proxy.stats().to_host.reordered, 1);

    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            corrupt_pct: 100.0,
            ..UdpRule::default()
        },
    );
    let original = b"corrupt precisely one byte";
    socket.send(original).await.unwrap();
    let response = recv(&socket).await;
    assert_eq!(response.len(), original.len());
    assert_eq!(
        response
            .iter()
            .zip(original)
            .filter(|(a, b)| a != b)
            .count(),
        1
    );
    assert_eq!(proxy.stats().to_host.corrupted, 1);
    // Empty datagrams have no byte to flip and must remain valid datagrams.
    socket.send(b"").await.unwrap();
    assert!(recv(&socket).await.is_empty());
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn mtu_blackhole_boundary() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            max_datagram: Some(1200),
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    socket.send(&[1; 1201]).await.unwrap();
    socket.send(&[2; 1200]).await.unwrap();
    assert_eq!(recv(&socket).await, vec![2; 1200]);
    assert_eq!(proxy.stats().to_host.oversize_dropped, 1);
    assert_eq!(proxy.stats().to_host.dropped, 1);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn bandwidth_limits_queue_to_two_hundred_ms() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            rate_kbps: Some(800),
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    let started = Instant::now();
    for id in 0..200 {
        socket.send(&[0x55; 1000]).await.unwrap();
        wait_for(|| proxy.stats().to_host.received > id).await;
    }
    wait_for(|| {
        let stats = proxy.stats().to_host;
        stats.dropped + stats.forwarded == 200
    })
    .await;
    let stats = proxy.stats().to_host;
    println!(
        "800 kbps / 200000 bytes: elapsed={:?}, stats={stats:?}",
        started.elapsed()
    );
    assert!(started.elapsed() >= Duration::from_millis(1500) || stats.dropped > 0);
    assert!(
        stats.forwarded >= 10,
        "initial token credit should be usable"
    );
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn skip_prefix_and_direction_counters_are_independent() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            skip_first: 5,
            blackhole: true,
            ..UdpRule::default()
        },
    );
    let socket = client(&proxy).await;
    for id in 0..5 {
        send_number(&socket, id).await;
        assert_eq!(number(&recv(&socket).await), id);
    }
    for id in 5..10 {
        send_number(&socket, id).await;
    }
    wait_for(|| proxy.stats().to_host.dropped == 5).await;
    assert_eq!(proxy.stats().to_client.received, 5);
    let mut buffer = [0; 8];
    assert!(timeout(Duration::from_millis(40), socket.recv(&mut buffer))
        .await
        .is_err());

    proxy.restore();
    // The reverse direction has received only 5 packets, so its own sixth
    // packet is still within this newly configured skip prefix.
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            skip_first: 6,
            blackhole: true,
            ..UdpRule::default()
        },
    );
    send_number(&socket, 10).await;
    assert_eq!(number(&recv(&socket).await), 10);
    send_number(&socket, 11).await;
    wait_for(|| proxy.stats().to_client.dropped == 1).await;
    assert_eq!(proxy.stats().to_host.dropped, 5);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn rules_change_for_delayed_and_reordered_packets() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 500,
            ..UdpRule::default()
        },
    );
    socket.send(b"queued").await.unwrap();
    wait_for(|| proxy.stats().to_host.received == 1).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            blackhole: true,
            ..UdpRule::default()
        },
    );
    wait_for(|| proxy.stats().to_host.dropped == 1).await;
    proxy.restore();
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            reorder_pct: 100.0,
            ..UdpRule::default()
        },
    );
    socket.send(b"held").await.unwrap();
    wait_for(|| proxy.stats().to_host.received == 2).await;
    proxy.restore();
    assert_eq!(recv(&socket).await, b"held");
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 500,
            ..UdpRule::default()
        },
    );
    socket.send(b"release delay").await.unwrap();
    wait_for(|| proxy.stats().to_host.received == 3).await;
    proxy.restore();
    assert_eq!(recv(&socket).await, b"release delay");
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn tcp_refuse_rebind_and_blackhole() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_tcp(TcpRule {
        mode: TcpMode::Refuse,
        ..TcpRule::default()
    });
    let error = TcpStream::connect(proxy.addr()).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
    proxy.set_tcp(TcpRule::default());
    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
    stream.write_all(b"forward").await.unwrap();
    let mut buffer = [0; 7];
    timeout(WAIT, stream.read_exact(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer, b"forward");
    assert_eq!(proxy.stats().tcp_accepted, 1);
    proxy.set_tcp(TcpRule {
        mode: TcpMode::Blackhole,
        ..TcpRule::default()
    });
    let mut blackhole = TcpStream::connect(proxy.addr()).await.unwrap();
    blackhole.write_all(b"held").await.unwrap();
    assert!(
        timeout(Duration::from_millis(300), blackhole.read(&mut buffer))
            .await
            .is_err()
    );
    assert_eq!(proxy.stats().tcp_accepted, 2);
    assert_eq!(proxy.stats().tcp_bytes_to_host, 7);
    proxy.restore();
    let mut restored = [0; 4];
    timeout(WAIT, blackhole.read_exact(&mut restored))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&restored, b"held");
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn cut_preserves_established_tcp_and_queued_bytes() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
    stream.write_all(b"warmup").await.unwrap();
    let mut warmup = [0; 6];
    timeout(WAIT, stream.read_exact(&mut warmup))
        .await
        .unwrap()
        .unwrap();
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            skip_first: 100,
            ..UdpRule::default()
        },
    );
    proxy.cut();
    stream.write_all(b"during cut").await.unwrap();
    socket.send(b"lost").await.unwrap();
    let mut buffer = [0; 10];
    assert!(
        timeout(Duration::from_millis(300), stream.read(&mut buffer))
            .await
            .is_err()
    );
    assert_eq!(proxy.stats().tcp_bytes_to_host, 6);
    assert_eq!(proxy.stats().to_host.dropped, 1);
    proxy.restore();
    timeout(WAIT, stream.read_exact(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer, b"during cut");
    stream.write_all(b"after").await.unwrap();
    let mut after = [0; 5];
    timeout(WAIT, stream.read_exact(&mut after))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&after, b"after");
    socket.send(b"restored").await.unwrap();
    assert_eq!(recv(&socket).await, b"restored");
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn tcp_delay_rate_and_half_close() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    proxy.set_tcp(TcpRule {
        delay_ms: 30,
        rate_kbps: Some(800),
        ..TcpRule::default()
    });
    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
    let started = Instant::now();
    let data = vec![0x51; 30_000];
    stream.write_all(&data).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response, data);
    assert!(started.elapsed() >= Duration::from_millis(200));
    wait_for(|| proxy.stats().tcp_bytes_to_client == 30_000).await;
    assert_eq!(proxy.stats().tcp_bytes_to_host, 30_000);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn shutdown_and_drop_release_ports_and_connections() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let addr = proxy.addr();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    wait_for(|| proxy.stats().tcp_accepted == 1).await;
    let socket = client(&proxy).await;
    socket.send(b"NAT").await.unwrap();
    assert_eq!(recv(&socket).await, b"NAT");
    proxy.shutdown().await;
    assert_eq!(
        timeout(WAIT, stream.read(&mut [0])).await.unwrap().unwrap(),
        0
    );
    let udp = UdpSocket::bind(addr).await.unwrap();
    let tcp = FaultProxy::listen(addr).unwrap();
    drop((udp, tcp));

    let proxy = echo.proxy(42).await;
    let addr = proxy.addr();
    proxy.set_tcp(TcpRule {
        mode: TcpMode::Blackhole,
        ..TcpRule::default()
    });
    let mut stream = TcpStream::connect(addr).await.unwrap();
    wait_for(|| proxy.stats().tcp_accepted == 1).await;
    drop(proxy);
    timeout(WAIT, async {
        loop {
            if let Ok(socket) = UdpSocket::bind(addr).await {
                break socket;
            }
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let _listener = FaultProxy::listen(addr).unwrap();
    assert_eq!(
        timeout(WAIT, stream.read(&mut [0])).await.unwrap().unwrap(),
        0
    );
}

#[tokio::test]
pub async fn explicit_bind_address_keeps_the_proxy_in_range() {
    let echo = Echo::start().await;
    let mut attempts = 0;
    let (addr, proxy) = loop {
        attempts += 1;
        let reserved = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = reserved.local_addr().unwrap();
        drop(reserved);
        // UDP allocation does not reserve TCP's port, and another test may
        // claim either socket before start_on binds. Retry fixture allocation
        // only; the explicit-bind API must still use exactly the chosen address.
        match FaultProxy::start_on(addr, echo.udp, Some(echo.tcp), 42).await {
            Ok(proxy) => break (addr, proxy),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && attempts < 8 => {}
            Err(error) => panic!("bind explicit proxy address {addr}: {error}"),
        }
    };
    assert_eq!(proxy.addr(), addr);
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(addr).await.unwrap();
    send_number(&socket, 7).await;
    assert_eq!(number(&recv(&socket).await), 7);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn reverse_rule_changes_do_not_reroll_queued_forward_packets() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 200,
            loss_pct: 20.0,
            ..UdpRule::default()
        },
    );
    for id in 0..100 {
        send_number(&socket, id).await;
        wait_for(|| proxy.stats().to_host.received > u64::from(id)).await;
    }
    let dropped = proxy.stats().to_host.dropped;
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            blackhole: true,
            ..UdpRule::default()
        },
    );
    wait_for(|| {
        let stats = proxy.stats().to_host;
        stats.dropped + stats.forwarded == 100
    })
    .await;
    assert_eq!(proxy.stats().to_host.dropped, dropped);
    wait_for(|| proxy.stats().to_client.received == 100 - dropped).await;
    assert_eq!(proxy.stats().to_client.dropped, 100 - dropped);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn reverse_duplication_corruption_reordering_and_mtu() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            duplicate_pct: 100.0,
            ..UdpRule::default()
        },
    );
    socket.send(b"reverse").await.unwrap();
    assert_eq!(recv(&socket).await, b"reverse");
    assert_eq!(recv(&socket).await, b"reverse");
    assert_eq!(proxy.stats().to_client.duplicated, 1);
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            reorder_pct: 100.0,
            ..UdpRule::default()
        },
    );
    send_number(&socket, 1).await;
    send_number(&socket, 2).await;
    assert_eq!(number(&recv(&socket).await), 2);
    assert_eq!(number(&recv(&socket).await), 1);
    assert_eq!(proxy.stats().to_client.reordered, 1);
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            corrupt_pct: 100.0,
            ..UdpRule::default()
        },
    );
    socket.send(b"reverse").await.unwrap();
    assert_eq!(
        recv(&socket)
            .await
            .iter()
            .zip(b"reverse")
            .filter(|(a, b)| a != b)
            .count(),
        1
    );
    assert_eq!(proxy.stats().to_client.corrupted, 1);
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            max_datagram: Some(1200),
            ..UdpRule::default()
        },
    );
    socket.send(&[1; 1201]).await.unwrap();
    socket.send(&[2; 1200]).await.unwrap();
    assert_eq!(recv(&socket).await, vec![2; 1200]);
    assert_eq!(proxy.stats().to_client.oversize_dropped, 1);
    proxy.shutdown().await;
}

#[tokio::test]
pub async fn zero_bandwidth_can_be_restored_without_closing_tcp() {
    let echo = Echo::start().await;
    let proxy = echo.proxy(42).await;
    let socket = client(&proxy).await;
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            rate_kbps: Some(0),
            ..UdpRule::default()
        },
    );
    socket.send(b"no tokens").await.unwrap();
    wait_for(|| proxy.stats().to_host.dropped == 1).await;
    proxy.set_tcp(TcpRule {
        rate_kbps: Some(0),
        ..TcpRule::default()
    });
    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
    stream.write_all(b"queued").await.unwrap();
    assert!(timeout(Duration::from_millis(40), stream.read(&mut [0; 6]))
        .await
        .is_err());
    proxy.restore();
    let mut bytes = [0; 6];
    timeout(WAIT, stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"queued");
    proxy.shutdown().await;
}
