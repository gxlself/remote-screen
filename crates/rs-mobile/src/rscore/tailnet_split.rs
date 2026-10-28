//! Network selection for mode B's embedded tailnet.

use std::{
    fmt, io,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use rsp_transport::{net::NetFuture, net::NetStream, NetDialer};
use tokio::sync::watch;

use super::netinfo::is_tailnet_ip;

/// App 内节点当前能不能拨。由控制器经接线更新。
#[derive(Clone)]
pub enum Readiness {
    /// 等也没用（本构建不可用 / needsLogin / 启动失败）：tailnet 目标立即失败
    Down,
    /// 因进后台被停，可以叫醒：先调 wake 钩子，再按 Starting 等
    Idle,
    /// 正在启动或重绑：等到 Ready
    Starting,
    /// 节点已经可以拨号。
    Ready {
        dialer: Arc<dyn NetDialer>,
        magic_dns_suffix: String,
        peer_names: Vec<String>,
    },
}

impl fmt::Debug for Readiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Down => formatter.write_str("Down"),
            Self::Idle => formatter.write_str("Idle"),
            Self::Starting => formatter.write_str("Starting"),
            Self::Ready {
                magic_dns_suffix,
                peer_names,
                ..
            } => formatter
                .debug_struct("Ready")
                .field("magic_dns_suffix", magic_dns_suffix)
                .field("peer_names_len", &peer_names.len())
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Embedded,
    System,
}

/// The default amount of time to wait for the embedded node to become ready.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(8);

/// Classify an IP address according to the tailnet address pools.
pub fn classify_ip(ip: IpAddr) -> Target {
    if is_tailnet_ip(ip) {
        Target::Embedded
    } else {
        Target::System
    }
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_owned()
}

fn normalize_suffix(suffix: &str) -> String {
    suffix
        .trim()
        .to_ascii_lowercase()
        .trim_matches('.')
        .to_owned()
}

/// Classify a host using the last Ready node's MagicDNS suffix and peer names.
pub fn classify_host(host: &str, magic_dns_suffix: &str, peer_names: &[String]) -> Target {
    let host = normalize_host(host);
    if let Ok(ip) = IpAddr::from_str(&host) {
        return classify_ip(ip);
    }

    let suffix = normalize_suffix(magic_dns_suffix);
    if !suffix.is_empty() && host.ends_with(&format!(".{suffix}")) {
        return Target::Embedded;
    }

    if !host.contains('.')
        && peer_names
            .iter()
            .map(|name| normalize_host(name))
            .any(|name| name == host)
    {
        Target::Embedded
    } else {
        Target::System
    }
}

#[derive(Debug, Default, Clone)]
struct LastReady {
    magic_dns_suffix: String,
    peer_names: Vec<String>,
}

/// A dialer that sends tailnet destinations through the embedded node and all
/// other destinations through the system network.
pub struct SplitNetDialer {
    system: Arc<dyn NetDialer>,
    readiness: watch::Sender<Readiness>,
    wait: Duration,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    last_ready: Mutex<LastReady>,
}

impl fmt::Debug for SplitNetDialer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let readiness = self.readiness();
        let variant = match readiness {
            Readiness::Down => "Down",
            Readiness::Idle => "Idle",
            Readiness::Starting => "Starting",
            Readiness::Ready { .. } => "Ready",
        };
        formatter
            .debug_struct("SplitNetDialer")
            .field("readiness", &variant)
            .field("wait", &self.wait)
            .finish()
    }
}

impl SplitNetDialer {
    pub fn new(system: Arc<dyn NetDialer>, wait: Duration) -> Arc<Self> {
        let (readiness, _) = watch::channel(Readiness::Down);
        Arc::new(Self {
            system,
            readiness,
            wait,
            wake: Mutex::new(None),
            last_ready: Mutex::new(LastReady::default()),
        })
    }

    pub fn set_readiness(&self, readiness: Readiness) {
        if let Readiness::Ready {
            magic_dns_suffix,
            peer_names,
            ..
        } = &readiness
        {
            let mut last_ready = match self.last_ready.lock() {
                Ok(last_ready) => last_ready,
                Err(error) => error.into_inner(),
            };
            last_ready.magic_dns_suffix = magic_dns_suffix.clone();
            last_ready.peer_names = peer_names.clone();
        }
        self.readiness.send_replace(readiness);
    }

    pub fn readiness(&self) -> Readiness {
        self.readiness.borrow().clone()
    }

    pub fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        let mut stored_wake = match self.wake.lock() {
            Ok(stored_wake) => stored_wake,
            Err(error) => error.into_inner(),
        };
        *stored_wake = Some(wake);
    }

    fn last_ready(&self) -> LastReady {
        match self.last_ready.lock() {
            Ok(last_ready) => last_ready.clone(),
            Err(error) => error.into_inner().clone(),
        }
    }

    async fn embedded(&self) -> io::Result<Arc<dyn NetDialer>> {
        let mut readiness = self.readiness.subscribe();
        let wait = async {
            let mut wake_called = false;
            loop {
                let current = readiness.borrow().clone();
                match current {
                    Readiness::Ready { dialer, .. } => return Ok(dialer),
                    Readiness::Down => {
                        tracing::debug!("embedded tailnet is not running");
                        return Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            "embedded tailnet is not running",
                        ));
                    }
                    Readiness::Idle => {
                        if !wake_called {
                            wake_called = true;
                            let wake = match self.wake.lock() {
                                Ok(wake) => wake.clone(),
                                Err(error) => error.into_inner().clone(),
                            };
                            if let Some(wake) = wake {
                                wake();
                            }
                        }
                    }
                    Readiness::Starting => {}
                }
                readiness.changed().await.map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "embedded tailnet readiness is unavailable",
                    )
                })?;
            }
        };

        match tokio::time::timeout(self.wait, wait).await {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "embedded tailnet did not become ready",
            )),
        }
    }
}

impl NetDialer for SplitNetDialer {
    fn shared_quic_endpoint(&self, _bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        if classify_ip(peer.ip()) == Target::System {
            return self.system.dial_udp(peer);
        }

        Box::pin(async move {
            let dialer = self.embedded().await?;
            dialer.dial_udp(peer).await
        })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        if classify_ip(peer.ip()) == Target::System {
            return self.system.dial_tcp(peer);
        }

        Box::pin(async move {
            let dialer = self.embedded().await?;
            dialer.dial_tcp(peer).await
        })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        let LastReady {
            magic_dns_suffix,
            peer_names,
        } = self.last_ready();
        if classify_host(host, &magic_dns_suffix, &peer_names) == Target::System {
            return self.system.dial_tcp_host(host, port);
        }

        Box::pin(async move {
            let dialer = self.embedded().await?;
            dialer.dial_tcp_host(host, port).await
        })
    }
}
