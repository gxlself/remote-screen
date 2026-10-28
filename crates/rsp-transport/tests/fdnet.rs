#![cfg(unix)]

use std::{
    collections::VecDeque,
    io,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    os::unix::io::OwnedFd,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};

use quinn::AsyncUdpSocket;
use rsp_transport::{
    dial::DefaultDialer,
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    FdNetDialer, FdSource, NetDialer, PeerUdpSocket, QuicListener, TransportKind,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixDatagram, UnixStream},
    time::timeout,
};

#[derive(Debug, Default)]
struct TestSource {
    udp: Mutex<VecDeque<io::Result<OwnedFd>>>,
    tcp: Mutex<VecDeque<io::Result<OwnedFd>>>,
    calls: Mutex<Vec<(String, u16)>>,
    udp_delay: Option<Duration>,
}

impl TestSource {
    fn udp_fd(fd: OwnedFd) -> Arc<Self> {
        Arc::new(Self {
            udp: Mutex::new(VecDeque::from([Ok(fd)])),
            ..Self::default()
        })
    }

    fn tcp_fd(fd: OwnedFd) -> Arc<Self> {
        Arc::new(Self {
            tcp: Mutex::new(VecDeque::from([Ok(fd)])),
            ..Self::default()
        })
    }

    fn calls(&self) -> Vec<(String, u16)> {
        self.calls.lock().unwrap().clone()
    }
}

impl FdSource for TestSource {
    fn dial_udp(&self, _peer: SocketAddr) -> io::Result<OwnedFd> {
        if let Some(delay) = self.udp_delay {
            std::thread::sleep(delay);
        }
        self.udp
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(io::Error::new(io::ErrorKind::NotFound, "no UDP fd")))
    }

    fn dial_tcp(&self, host: &str, port: u16) -> io::Result<OwnedFd> {
        self.calls.lock().unwrap().push((host.to_owned(), port));
        self.tcp
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(io::Error::new(io::ErrorKind::NotFound, "no TCP fd")))
    }
}

fn owned_fd<T: Into<OwnedFd>>(socket: T) -> OwnedFd {
    socket.into()
}

fn local() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

fn identities() -> (LocalIdentity, LocalIdentity) {
    (
        LocalIdentity::generate_software(CertKind::Host).unwrap(),
        LocalIdentity::generate_software(CertKind::App).unwrap(),
    )
}

#[tokio::test]
async fn connected_udp_fd_carries_quic() {
    let (host, app) = identities();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let peer = listener.local_addr().unwrap();
    let socket = StdUdpSocket::bind(local()).unwrap();
    socket.connect(peer).unwrap();
    let source = TestSource::udp_fd(owned_fd(socket));
    let dialer = DefaultDialer::with_net(Arc::new(FdNetDialer::new(source)), None, &app).unwrap();

    let accept = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (client, server) = tokio::join!(dialer.dial(peer, TransportKind::Quic), accept);
    let client = client.unwrap();
    let client_control = client.open_control().await.unwrap();
    client_control.send(b"hello").await.unwrap();
    let server_control = server.accept_control().await.unwrap();
    assert_eq!(server_control.recv().await.unwrap().unwrap(), b"hello"[..]);
    server_control.send(b"world").await.unwrap();
    assert_eq!(client_control.recv().await.unwrap().unwrap(), b"world"[..]);
    let payload = vec![0x5a; 4 * 1024 * 1024];
    let server_for_accept = server.clone();
    let receive_task = tokio::spawn(async move {
        let (_, mut recv) = server_for_accept.accept_uni().await.unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        while let Some(length) = recv.read_raw(&mut chunk).await.unwrap() {
            got.extend_from_slice(&chunk[..length]);
        }
        got
    });
    let mut send = client
        .open_uni(rsp_transport::StreamKind::File)
        .await
        .unwrap();
    send.write_raw(&payload).await.unwrap();
    send.finish().await.unwrap();
    let got = receive_task.await.unwrap();
    assert_eq!(got, payload);
    client.close(rsp_transport::CloseCode::Normal, b"done");
}

#[tokio::test]
async fn connected_udp_fd_carries_packets() {
    let receiver = StdUdpSocket::bind(local()).unwrap();
    let receiver_addr = receiver.local_addr().unwrap();
    let sender = StdUdpSocket::bind(local()).unwrap();
    sender.connect(receiver_addr).unwrap();
    let peer = PeerUdpSocket::new(owned_fd(sender), receiver_addr).unwrap();

    let transmit = quinn::udp::Transmit {
        destination: receiver_addr,
        ecn: None,
        contents: b"fdnet",
        segment_size: None,
        src_ip: None,
    };
    peer.try_send(&transmit).unwrap();

    let mut bytes = [0; 32];
    receiver
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (length, _) = receiver.recv_from(&mut bytes).unwrap();
    assert_eq!(&bytes[..length], b"fdnet");
}

#[tokio::test]
async fn unix_dgram_pair_relay_carries_packets() {
    let (left, right) = UnixDatagram::pair().unwrap();
    let peer = SocketAddr::from(([127, 0, 0, 1], 4567));
    let socket = PeerUdpSocket::new(owned_fd(left.into_std().unwrap()), peer).unwrap();
    socket
        .try_send(&quinn::udp::Transmit {
            destination: peer,
            ecn: None,
            contents: b"relay",
            segment_size: None,
            src_ip: None,
        })
        .unwrap();
    let mut received = [0; 32];
    let length = right.recv(&mut received).await.unwrap();
    assert_eq!(&received[..length], b"relay");
}

#[tokio::test]
async fn bad_udp_fds_are_rejected() {
    let peer = SocketAddr::from(([127, 0, 0, 1], 9));

    let (stream, _) = UnixStream::pair().unwrap();
    let source = TestSource::udp_fd(owned_fd(stream.into_std().unwrap()));
    let dialer = FdNetDialer::new(source);
    let error = dialer.dial_udp(peer).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("SOCK_DGRAM"));

    let unconnected = StdUdpSocket::bind(local()).unwrap();
    let source = TestSource::udp_fd(owned_fd(unconnected));
    let error = FdNetDialer::new(source).dial_udp(peer).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    let other = StdUdpSocket::bind(local()).unwrap();
    let other_addr = other.local_addr().unwrap();
    let connected = StdUdpSocket::bind(local()).unwrap();
    connected.connect(other_addr).unwrap();
    let source = TestSource::udp_fd(owned_fd(connected));
    let error = FdNetDialer::new(source).dial_udp(peer).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn icmp_unreachable_is_connection_reset() {
    let unused = StdUdpSocket::bind(local()).unwrap();
    let peer = unused.local_addr().unwrap();
    drop(unused);
    let sender = StdUdpSocket::bind(local()).unwrap();
    sender.connect(peer).unwrap();
    let socket = Arc::new(PeerUdpSocket::new(owned_fd(sender), peer).unwrap());
    socket
        .try_send(&quinn::udp::Transmit {
            destination: peer,
            ecn: None,
            contents: b"probe",
            segment_size: None,
            src_ip: None,
        })
        .unwrap();

    let mut bytes = [0; 64];
    let mut bufs = [io::IoSliceMut::new(&mut bytes)];
    let mut meta = [quinn::udp::RecvMeta::default()];
    let error = timeout(
        Duration::from_secs(2),
        std::future::poll_fn(|cx| match socket.poll_recv(cx, &mut bufs, &mut meta) {
            Poll::Ready(result) => Poll::Ready(result.unwrap_err()),
            Poll::Pending => Poll::Pending,
        }),
    )
    .await
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    socket
        .try_send(&quinn::udp::Transmit {
            destination: peer,
            ecn: None,
            contents: b"again",
            segment_size: None,
            src_ip: None,
        })
        .unwrap();
}

#[tokio::test]
async fn tcp_fd_carries_stream_and_records_arguments() {
    let listener = TcpListener::bind(local()).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let std_stream = std::net::TcpStream::connect(addr).unwrap();
    std_stream.set_nonblocking(false).unwrap();
    let source = TestSource::tcp_fd(owned_fd(std_stream));
    let dialer = FdNetDialer::new(source.clone());
    let (stream, accepted) = tokio::join!(dialer.dial_tcp(addr), listener.accept(),);
    let mut stream = stream.unwrap();
    let (mut accepted, _) = accepted.unwrap();
    stream.write_all(b"tcp").await.unwrap();
    let mut bytes = [0; 3];
    accepted.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"tcp");
    assert_eq!(source.calls(), vec![(addr.ip().to_string(), addr.port())]);
}

#[tokio::test]
async fn unix_stream_fd_carries_stream() {
    let (left, mut right) = UnixStream::pair().unwrap();
    let source = TestSource::tcp_fd(owned_fd(left.into_std().unwrap()));
    let dialer = FdNetDialer::new(source);
    let mut stream = dialer
        .dial_tcp(SocketAddr::from(([127, 0, 0, 1], 1)))
        .await
        .unwrap();
    stream.write_all(b"unix").await.unwrap();
    let mut bytes = [0; 4];
    right.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"unix");
}

#[tokio::test]
async fn cancelled_dial_closes_fd() {
    let (left, right) = UnixDatagram::pair().unwrap();
    let source = Arc::new(TestSource {
        udp: Mutex::new(VecDeque::from([Ok(owned_fd(left.into_std().unwrap()))])),
        udp_delay: Some(Duration::from_millis(100)),
        ..TestSource::default()
    });
    let dialer = FdNetDialer::new(source);
    let peer = SocketAddr::from(([127, 0, 0, 1], 9));
    assert!(timeout(Duration::from_millis(10), dialer.dial_udp(peer))
        .await
        .is_err());
    tokio::time::sleep(Duration::from_millis(250)).await;
    let error = right.send(b"closed").await.unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotConnected
    ));
}
