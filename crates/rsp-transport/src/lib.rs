//! RSP transport: authenticated QUIC/TCP connections and candidate dialing.

pub mod conn;
pub mod dialer;
pub mod identity;
pub mod mux;
pub mod net;
pub mod quic;
pub mod tcp;
pub mod tls;

pub use conn::{
    CloseCode, CloseReason, Conn, ConnError, ControlStream, TransportKind, UniRecv, UniSend,
    MAX_CONTROL_FRAME,
};
pub use mux::{DatagramKind, StreamKind};
pub use net::{system_net, NetDialer, NetStream, SystemNetDialer};
pub use quic::{connect_quic, QuicIncoming, QuicListener, DATAGRAM_SEND_BUFFER_BYTES};
pub use tcp::{
    connect_tcp, connect_tcp_via, encode_tcp_record, TcpIncoming, TcpListener, TcpRecordDecoder,
    TcpRecordHeader,
};

pub mod dial;
#[cfg(unix)]
pub mod fdnet;
#[cfg(unix)]
pub use fdnet::{FdNetDialer, FdSource, PeerUdpSocket};
