//! Network adapters for file descriptors handed out by an external dialer.
//!
//! The descriptors used here are connected sockets.  A UDP descriptor can only
//! reach the peer it was connected to, so it is deliberately exposed as an
//! abstract socket for one QUIC endpoint rather than as a shared UDP endpoint.

#![cfg(unix)]

use std::{
    fmt,
    io::{self, IoSliceMut, Read},
    net::SocketAddr,
    os::unix::io::OwnedFd,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use quinn::{
    udp::{RecvMeta, Transmit},
    AsyncUdpSocket, UdpPoller,
};
use socket2::{Socket, Type};
use tokio::{
    io::{unix::AsyncFd, Interest},
    task::JoinHandle,
};

use crate::net::{NetDialer, NetFuture, NetStream};

const SOCKET_RECV_BUFFER: usize = 4 * 1024 * 1024;
const SOCKET_SEND_BUFFER: usize = 1024 * 1024;
const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// A source of connected sockets supplied by an external network provider.
pub trait FdSource: Send + Sync + fmt::Debug + 'static {
    /// Blocking call corresponding to the future tailscale dial operation.
    /// The returned descriptor is owned by Rust and is closed by Rust.
    fn dial_udp(&self, peer: SocketAddr) -> io::Result<OwnedFd>;

    /// Blocking call corresponding to a TCP dial through the external network.
    fn dial_tcp(&self, host: &str, port: u16) -> io::Result<OwnedFd>;
}

#[derive(Debug, Clone)]
pub struct FdNetDialer {
    source: Arc<dyn FdSource>,
}

impl FdNetDialer {
    pub fn new(source: Arc<dyn FdSource>) -> Self {
        Self { source }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressFamily {
    Ip,
    Unix,
}

fn family(addr: &socket2::SockAddr) -> AddressFamily {
    if addr.is_ipv4() || addr.is_ipv6() {
        AddressFamily::Ip
    } else {
        AddressFamily::Unix
    }
}

fn unspecified_for(peer: SocketAddr) -> SocketAddr {
    match peer {
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ErrorClass {
    Raw(i32),
    Kind(io::ErrorKind),
}

#[derive(Debug, Default)]
struct ErrorLogState {
    entries: Vec<(ErrorClass, Instant)>,
}

impl ErrorLogState {
    fn should_log(&mut self, error: &io::Error) -> bool {
        let class = error
            .raw_os_error()
            .map(ErrorClass::Raw)
            .unwrap_or_else(|| ErrorClass::Kind(error.kind()));
        let now = Instant::now();
        if let Some((_, last)) = self.entries.iter_mut().find(|(kind, _)| *kind == class) {
            if now.duration_since(*last) < ERROR_LOG_INTERVAL {
                return false;
            }
            *last = now;
            return true;
        }
        self.entries.push((class, now));
        true
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug)]
pub struct PeerUdpSocket {
    io: AsyncFd<Socket>,
    peer: SocketAddr,
    local: SocketAddr,
    family: AddressFamily,
    send_errors: Mutex<ErrorLogState>,
    destination_logs: Mutex<Instant>,
    initial_send_probe: AtomicBool,
    pending_recv_error: Mutex<Option<io::ErrorKind>>,
}

impl PeerUdpSocket {
    pub fn new(fd: OwnedFd, peer: SocketAddr) -> io::Result<Self> {
        let socket = Socket::from(fd);
        if socket.r#type()? != Type::DGRAM {
            return Err(invalid_input("fd must refer to a SOCK_DGRAM socket"));
        }

        let socket_local = socket.local_addr()?;
        let family = family(&socket_local);
        if family == AddressFamily::Ip {
            let connected_peer = socket.peer_addr().ok().and_then(|addr| addr.as_socket());
            if connected_peer != Some(peer) {
                return Err(invalid_input(
                    "SOCK_DGRAM fd must be connected to the requested peer",
                ));
            }
        }

        socket.set_nonblocking(true)?;
        if let Err(error) = socket.set_recv_buffer_size(SOCKET_RECV_BUFFER) {
            tracing::debug!(?error, "unable to increase fd UDP receive buffer");
        }
        if let Err(error) = socket.set_send_buffer_size(SOCKET_SEND_BUFFER) {
            tracing::debug!(?error, "unable to increase fd UDP send buffer");
        }

        let local = match family {
            AddressFamily::Ip => socket_local
                .as_socket()
                .ok_or_else(|| invalid_input("IP socket has no IP local address"))?,
            AddressFamily::Unix => unspecified_for(peer),
        };
        // SAFETY: `socket` owns a valid descriptor and remains owned by the
        // `AsyncFd` for its entire lifetime; no code closes or replaces it.
        let io = unsafe { AsyncFd::register(socket)? };
        Ok(Self {
            io,
            peer,
            local,
            family,
            send_errors: Mutex::new(ErrorLogState::default()),
            destination_logs: Mutex::new(
                Instant::now()
                    .checked_sub(ERROR_LOG_INTERVAL)
                    .unwrap_or_else(Instant::now),
            ),
            initial_send_probe: AtomicBool::new(true),
            pending_recv_error: Mutex::new(None),
        })
    }

    fn log_send_error(&self, error: &io::Error) {
        if lock_unpoisoned(&self.send_errors).should_log(error) {
            tracing::debug!(peer = ?self.peer, ?error, "fd UDP send failed");
        }
    }

    fn log_destination_mismatch(&self, destination: SocketAddr) {
        let mut last = lock_unpoisoned(&self.destination_logs);
        let now = Instant::now();
        if now.duration_since(*last) >= ERROR_LOG_INTERVAL {
            *last = now;
            tracing::debug!(
                peer = ?self.peer,
                destination = ?destination,
                "discarding fd UDP packet for a different peer"
            );
        }
    }

    fn remember_recv_error(&self, error: &io::Error) {
        let kind = match (self.family, error.kind()) {
            (AddressFamily::Ip, io::ErrorKind::ConnectionRefused) => {
                Some(io::ErrorKind::ConnectionReset)
            }
            (
                AddressFamily::Unix,
                io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::NotConnected,
            ) => Some(io::ErrorKind::BrokenPipe),
            _ => None,
        };
        if let Some(kind) = kind {
            *lock_unpoisoned(&self.pending_recv_error) = Some(kind);
        }
    }

    fn send_one(&self, bytes: &[u8]) -> io::Result<()> {
        let result = self.io.try_io(Interest::WRITABLE, |_| {
            let sent = self.io.get_ref().send(bytes)?;
            if sent == bytes.len() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "connected datagram sent only part of a packet",
                ))
            }
        });
        match result {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // `AsyncFd::try_io` has no cached readiness event immediately
                // after registration. Probe the nonblocking socket once so a
                // first packet does not spuriously report WouldBlock. Every
                // later attempt follows the readiness contract exactly.
                if self.initial_send_probe.swap(false, Ordering::Relaxed) {
                    match self.io.get_ref().send(bytes) {
                        Ok(sent) if sent == bytes.len() => Ok(()),
                        Ok(_) => {
                            let error = io::Error::new(
                                io::ErrorKind::WriteZero,
                                "connected datagram sent only part of a packet",
                            );
                            self.log_send_error(&error);
                            Ok(())
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(error),
                        Err(error) => {
                            self.remember_recv_error(&error);
                            self.log_send_error(&error);
                            Ok(())
                        }
                    }
                } else {
                    Err(error)
                }
            }
            Err(error) => {
                self.remember_recv_error(&error);
                self.log_send_error(&error);
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}

impl AsyncUdpSocket for PeerUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(PeerUdpPoller { socket: self })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if transmit.destination != self.peer {
            self.log_destination_mismatch(transmit.destination);
            return Ok(());
        }

        let segment_size = transmit.segment_size.unwrap_or(transmit.contents.len());
        if segment_size == 0 {
            return self.send_one(transmit.contents);
        }
        for segment in transmit.contents.chunks(segment_size) {
            self.send_one(segment)?;
        }
        if transmit.contents.is_empty() {
            self.send_one(&[])?;
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(invalid_input(
                "quinn supplied no receive buffer or metadata slot",
            )));
        }

        loop {
            if let Some(kind) = lock_unpoisoned(&self.pending_recv_error).take() {
                if kind == io::ErrorKind::ConnectionReset {
                    self.initial_send_probe.store(true, Ordering::Relaxed);
                }
                return Poll::Ready(Err(io::Error::new(kind, "fd peer reported an error")));
            }
            if let Ok(Some(error)) = self.io.get_ref().take_error() {
                if error.kind() == io::ErrorKind::ConnectionRefused
                    && self.family == AddressFamily::Ip
                {
                    self.initial_send_probe.store(true, Ordering::Relaxed);
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionReset, error)));
                }
                if self.family == AddressFamily::Unix
                    && matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::NotConnected
                    )
                {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "tailnet fd closed",
                    )));
                }
                return Poll::Ready(Err(error));
            }

            // On Linux an asynchronous ICMP error wakes the writable side of
            // epoll. Consume that readiness and inspect SO_ERROR before
            // waiting for readable data.
            if let Poll::Ready(Ok(_guard)) = self.io.poll_write_ready(cx) {
                let error = self.io.get_ref().take_error();
                // Keep writable readiness set when SO_ERROR is clear. The
                // QUIC send poller may be waiting on the same event.
                if let Ok(Some(error)) = error {
                    if error.kind() == io::ErrorKind::ConnectionRefused
                        && self.family == AddressFamily::Ip
                    {
                        self.initial_send_probe.store(true, Ordering::Relaxed);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            error,
                        )));
                    }
                    if self.family == AddressFamily::Unix
                        && matches!(
                            error.kind(),
                            io::ErrorKind::BrokenPipe
                                | io::ErrorKind::ConnectionRefused
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::NotConnected
                        )
                    {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "tailnet fd closed",
                        )));
                    }
                    return Poll::Ready(Err(error));
                }
            }
            match self.io.poll_read_ready(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(_)) => {}
            }

            let result = self.io.try_io(Interest::READABLE, |socket| {
                let mut socket = socket;
                socket.read(&mut bufs[0])
            });
            match result {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    if self.family == AddressFamily::Ip {
                        self.initial_send_probe.store(true, Ordering::Relaxed);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            error,
                        )));
                    }
                    if matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::NotConnected
                    ) {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "tailnet fd closed",
                        )));
                    }
                    return Poll::Ready(Err(error));
                }
                Err(error) => return Poll::Ready(Err(error)),
                Ok(0) if self.family == AddressFamily::Unix => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "tailnet fd closed",
                    )));
                }
                Ok(0) => continue,
                Ok(length) => {
                    meta[0] = RecvMeta {
                        addr: self.peer,
                        len: length,
                        stride: length,
                        ..RecvMeta::default()
                    };
                    return Poll::Ready(Ok(1));
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }

    fn max_transmit_segments(&self) -> usize {
        1
    }

    fn max_receive_segments(&self) -> usize {
        1
    }
}

#[derive(Debug)]
struct PeerUdpPoller {
    socket: Arc<PeerUdpSocket>,
}

impl UdpPoller for PeerUdpPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.socket.io.poll_write_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
        }
    }
}

async fn await_fd(task: JoinHandle<io::Result<OwnedFd>>) -> io::Result<OwnedFd> {
    task.await.map_err(io::Error::other)?
}

fn dial_tcp_fd(fd: OwnedFd, nodelay: bool) -> io::Result<NetStream> {
    let socket = Socket::from(fd);
    if socket.r#type()? != Type::STREAM {
        return Err(invalid_input("fd must refer to a SOCK_STREAM socket"));
    }
    let socket_family = family(&socket.local_addr()?);
    socket.set_nonblocking(true)?;
    match socket_family {
        AddressFamily::Unix => {
            let stream: std::os::unix::net::UnixStream = socket.into();
            let stream = tokio::net::UnixStream::from_std(stream)?;
            Ok(Box::new(stream) as NetStream)
        }
        AddressFamily::Ip => {
            let stream: std::net::TcpStream = socket.into();
            let stream = tokio::net::TcpStream::from_std(stream)?;
            if nodelay {
                stream.set_nodelay(true)?;
            }
            Ok(Box::new(stream) as NetStream)
        }
    }
}

impl NetDialer for FdNetDialer {
    fn shared_quic_endpoint(&self, _bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn AsyncUdpSocket>> {
        let source = self.source.clone();
        Box::pin(async move {
            let task = tokio::task::spawn_blocking(move || source.dial_udp(peer));
            let fd = await_fd(task).await?;
            Ok(Arc::new(PeerUdpSocket::new(fd, peer)?) as Arc<dyn AsyncUdpSocket>)
        })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        let source = self.source.clone();
        Box::pin(async move {
            let host = peer.ip().to_string();
            let port = peer.port();
            let task = tokio::task::spawn_blocking(move || source.dial_tcp(&host, port));
            dial_tcp_fd(await_fd(task).await?, true)
        })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        let source = self.source.clone();
        Box::pin(async move {
            let host = host.to_owned();
            let task = tokio::task::spawn_blocking(move || source.dial_tcp(&host, port));
            dial_tcp_fd(await_fd(task).await?, false)
        })
    }
}
