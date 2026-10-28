//! Blocking file-descriptor source for `rsp_transport::fdnet`.

use crate::node::Tailnet;
use rsp_transport::fdnet::FdSource;
use std::{fmt, io, net::SocketAddr, os::fd::OwnedFd, sync::Arc, time::Duration};

pub struct TailnetFdSource {
    pub(crate) tailnet: Arc<Tailnet>,
    pub(crate) udp_timeout: Duration,
    pub(crate) tcp_timeout: Duration,
}

impl fmt::Debug for TailnetFdSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailnetFdSource")
            .field("udp_timeout", &self.udp_timeout)
            .field("tcp_timeout", &self.tcp_timeout)
            .finish()
    }
}

impl FdSource for TailnetFdSource {
    fn dial_udp(&self, peer: SocketAddr) -> io::Result<OwnedFd> {
        self.tailnet.dial_udp(peer, self.udp_timeout)
    }

    fn dial_tcp(&self, host: &str, port: u16) -> io::Result<OwnedFd> {
        self.tailnet.dial_tcp(host, port, self.tcp_timeout)
    }
}
