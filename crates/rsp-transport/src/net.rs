//! Socket routing beneath the authenticated session dialer.

use std::{fmt, future::Future, io, net::SocketAddr, pin::Pin, sync::Arc};

use quinn::Runtime;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub trait NetIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> NetIo for T {}

pub type NetStream = Box<dyn NetIo>;
pub type NetFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// Selects the network used for outbound sockets.
///
/// Implementations must be cancellation safe: dropping a future must not leave
/// background tasks running or leak file descriptors.
/// Return I/O errors unchanged, without wrapping them or changing their kind;
/// the session dial race distinguishes connection refusal from timeout this way.
pub trait NetDialer: Send + Sync + fmt::Debug + 'static {
    /// The QUIC endpoint shared by all candidates in one dial race.
    /// System networking returns `Some(Endpoint::client(bind))`. `None` requests
    /// one endpoint per peer, using the socket returned by `dial_udp` (for
    /// networks such as tailnet whose socket only reaches a single peer).
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>>;

    /// A UDP socket for QUIC traffic to `peer`.
    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>>;

    /// TCP to an IP address, used by RSP fallback and its input connection.
    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream>;

    /// TCP to a host resolved within this network, used by SSH and jump hosts.
    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemNetDialer;

impl NetDialer for SystemNetDialer {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(Some(quinn::Endpoint::client(bind)?))
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async move {
            let bind = match peer {
                SocketAddr::V4(addr) if addr.ip().is_loopback() => {
                    SocketAddr::from(([127, 0, 0, 1], 0))
                }
                SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
                SocketAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
            };
            let socket = socket2::Socket::new(
                socket2::Domain::for_address(bind),
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )?;
            if bind.is_ipv6() {
                socket.set_only_v6(false)?;
            }
            socket.bind(&bind.into())?;
            // Do not connect the socket: quinn-udp supplies the destination and
            // retains its GSO and ECN support.
            quinn::TokioRuntime.wrap_udp_socket(socket.into())
        })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        Box::pin(async move {
            let socket = TcpStream::connect(peer).await?;
            socket.set_nodelay(true)?;
            Ok(Box::new(socket) as NetStream)
        })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        Box::pin(async move { Ok(Box::new(TcpStream::connect((host, port)).await?) as NetStream) })
    }
}

pub fn system_net() -> Arc<dyn NetDialer> {
    Arc::new(SystemNetDialer)
}
