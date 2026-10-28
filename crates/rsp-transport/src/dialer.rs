//! Dialer abstraction used by the session layer.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use crate::conn::{Conn, ConnError, TransportKind};

pub trait Dialer: Send + Sync {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>>;
}
