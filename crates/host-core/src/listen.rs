//! Host listeners with source filtering before TLS.

use std::collections::{hash_map::Entry, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};

use crate::platform::Nic;
use crate::RuntimeResult;
use ipnet::IpNet;
use rsp_transport::{Conn, QuicListener, TcpListener};
use tokio::sync::{mpsc, watch, Mutex, RwLock};
use tracing::warn;

/// Compatibility test binder. Production uses HostListener with a persistent identity.
pub fn bind(addr: SocketAddr) -> RuntimeResult<(quinn::Endpoint, String)> {
    let identity = rsp_transport::identity::LocalIdentity::generate_software(
        rsp_transport::identity::CertKind::Host,
    )
    .map_err(crate::identity::IdentityError::from)?;
    let fp = identity
        .fingerprint()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let tls = rsp_transport::tls::server_config(&identity)?;
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)?;
    let config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    Ok((quinn::Endpoint::server(config, addr)?, fp))
}

/// The three network paths used by the authorization layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathClass {
    Tailnet,
    Intranet,
    /// A connection whose source is in a user-marked home network.
    Lan,
}

/// Host listener policy. `lan_cidrs` is the subset of `allowed_cidrs` marked
/// "home" in the network settings and is used only to label an admitted path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenPolicy {
    pub tailnet: bool,
    pub intranet_nics: Vec<String>,
    pub allowed_cidrs: Vec<IpNet>,
    pub lan_cidrs: Vec<IpNet>,
    pub port: u16,
}

impl Default for ListenPolicy {
    fn default() -> Self {
        Self {
            tailnet: true,
            intranet_nics: Vec::new(),
            allowed_cidrs: Vec::new(),
            lan_cidrs: Vec::new(),
            port: 7400,
        }
    }
}

/// Return all local addresses that should be exposed, with their bound path.
/// `Nic::tailnet` is honored so platform adapters and tests can inject a
/// tailnet address without needing to allocate a real 100.64/10 address.
pub fn bind_addrs(policy: &ListenPolicy, nics: &[Nic]) -> Vec<(SocketAddr, PathClass)> {
    let mut result: Vec<(SocketAddr, PathClass)> = Vec::new();
    let mut positions: HashMap<SocketAddr, usize> = HashMap::new();
    for nic in nics {
        let selected_intranet = policy.intranet_nics.iter().any(|name| name == &nic.name);
        for ip in &nic.addresses {
            let tailnet_address = is_tailnet_ip(*ip);
            if nic.tailnet && !policy.tailnet {
                continue;
            }
            if policy.tailnet && (tailnet_address || nic.tailnet) {
                let addr = SocketAddr::new(*ip, policy.port);
                if let Some(index) = positions.get(&addr).copied() {
                    result[index].1 = PathClass::Tailnet;
                } else {
                    positions.insert(addr, result.len());
                    result.push((addr, PathClass::Tailnet));
                }
                continue;
            }
            if selected_intranet && !tailnet_address {
                let path = if contains_ip(&policy.lan_cidrs, *ip) {
                    PathClass::Lan
                } else {
                    PathClass::Intranet
                };
                let addr = SocketAddr::new(*ip, policy.port);
                if let Entry::Vacant(entry) = positions.entry(addr) {
                    entry.insert(result.len());
                    result.push((addr, path));
                }
            }
        }
    }
    result
}

/// Check whether a connection may reach the TLS layer. IPv4-mapped IPv6
/// addresses are normalized before testing the fixed tailnet and CIDR ranges.
pub fn admit(local: SocketAddr, remote: SocketAddr, policy: &ListenPolicy) -> Option<PathClass> {
    let local_ip = normalize_ip(local.ip());
    let remote_ip = normalize_ip(remote.ip());
    if is_tailnet_ip(local_ip) {
        return Some(PathClass::Tailnet);
    }
    if policy.intranet_nics.is_empty() {
        return None;
    }
    if !contains_ip(&policy.allowed_cidrs, remote_ip) {
        return None;
    }
    if contains_ip(&policy.lan_cidrs, remote_ip) {
        Some(PathClass::Lan)
    } else {
        Some(PathClass::Intranet)
    }
}

fn contains_ip(cidrs: &[IpNet], ip: IpAddr) -> bool {
    cidrs.iter().any(|cidr| cidr.contains(&ip))
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(value) => value.to_ipv4().map(IpAddr::V4).unwrap_or(IpAddr::V6(value)),
        value => value,
    }
}

fn is_tailnet_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(value) => {
            let octets = value.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(value) => {
            let octets = value.octets();
            octets[..6] == [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0]
        }
    }
}

/// Transport selected for a bound socket.
pub use rsp_transport::TransportKind as Transport;

#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[error("listener channel closed")]
    Closed,
}

/// A connection which passed source filtering and completed transport TLS.
pub struct Accepted {
    pub conn: Conn,
    pub tcp_tokens: Option<rsp_transport::tcp::TcpTokenRegistry>,
    pub path: PathClass,
    pub local: SocketAddr,
    pub remote: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerStats {
    pub local: SocketAddr,
    pub path: PathClass,
    pub accepted: u64,
    pub ignored: u64,
    pub tls_failed: u64,
    pub input_attachments: u64,
}

#[derive(Default)]
struct AtomicStats {
    accepted: AtomicU64,
    ignored: AtomicU64,
    tls_failed: AtomicU64,
    input_attachments: AtomicU64,
}

impl AtomicStats {
    fn snapshot(&self, local: SocketAddr, path: PathClass) -> ListenerStats {
        ListenerStats {
            local,
            path,
            accepted: self.accepted.load(Ordering::Relaxed),
            ignored: self.ignored.load(Ordering::Relaxed),
            tls_failed: self.tls_failed.load(Ordering::Relaxed),
            input_attachments: self.input_attachments.load(Ordering::Relaxed),
        }
    }
}

struct Binding {
    key: SocketAddr,
    path: PathClass,
    quic_local: Option<SocketAddr>,
    tcp_local: Option<SocketAddr>,
    stats: Arc<AtomicStats>,
    stop: watch::Sender<bool>,
}

struct ListenerState {
    bindings: HashMap<SocketAddr, Binding>,
}

/// The H11 listener owns one QUIC and one TCP accept loop for every selected
/// local address. Updating the policy only stops loops whose requested address
/// or path changed; existing `Conn`s are independent of the listener.
pub struct HostListener {
    shared: Arc<Shared>,
    receiver: Mutex<mpsc::Receiver<Accepted>>,
}

/// Updates a live listener without owning its accept receiver or sessions.
#[derive(Clone)]
pub struct ListenerUpdater {
    shared: Arc<Shared>,
}

struct Shared {
    state: StdRwLock<ListenerState>,
    policy: Arc<RwLock<ListenPolicy>>,
    tls: Arc<rustls::ServerConfig>,
    events: mpsc::Sender<Accepted>,
    update_lock: Mutex<()>,
}

impl Drop for HostListener {
    fn drop(&mut self) {
        // Close before stopping bindings so an updater racing with Drop sees it.
        self.receiver.get_mut().close();
        self.shared.stop();
    }
}

impl ListenerUpdater {
    /// Returns `Closed` after the listener has gone away, stopping any bindings
    /// created by an update racing with the listener's destruction.
    pub async fn update(&self, policy: ListenPolicy, nics: Vec<Nic>) -> Result<(), ListenError> {
        let _update = self.shared.update_lock.lock().await;
        if self.shared.events.is_closed() {
            return Err(ListenError::Closed);
        }
        self.shared.update_inner(policy, nics).await;
        if self.shared.events.is_closed() {
            self.shared.stop();
            return Err(ListenError::Closed);
        }
        Ok(())
    }
}

impl HostListener {
    pub async fn start(
        policy: ListenPolicy,
        nics: Vec<Nic>,
        tls: Arc<rustls::ServerConfig>,
    ) -> Result<Self, ListenError> {
        let (events, receiver) = mpsc::channel(64);
        let listener = Self {
            shared: Arc::new(Shared {
                state: StdRwLock::new(ListenerState {
                    bindings: HashMap::new(),
                }),
                policy: Arc::new(RwLock::new(ListenPolicy::default())),
                tls,
                events,
                update_lock: Mutex::new(()),
            }),
            receiver: Mutex::new(receiver),
        };
        listener.shared.update_inner(policy, nics).await;
        Ok(listener)
    }

    pub async fn accept(&self) -> Result<Accepted, ListenError> {
        self.receiver
            .lock()
            .await
            .recv()
            .await
            .ok_or(ListenError::Closed)
    }

    pub async fn update(&self, policy: ListenPolicy, nics: Vec<Nic>) -> Result<(), ListenError> {
        self.updater().update(policy, nics).await
    }

    pub fn updater(&self) -> ListenerUpdater {
        ListenerUpdater {
            shared: self.shared.clone(),
        }
    }

    pub fn bound(&self) -> Vec<(SocketAddr, PathClass, Transport)> {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let mut result = Vec::new();
        for binding in state.bindings.values() {
            if let Some(local) = binding.quic_local {
                result.push((local, binding.path, Transport::Quic));
            }
            if let Some(local) = binding.tcp_local {
                result.push((local, binding.path, Transport::Tcp));
            }
        }
        result
    }

    pub fn stats(&self) -> Vec<ListenerStats> {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(|error| error.into_inner());
        state
            .bindings
            .values()
            .map(|binding| {
                let local = binding
                    .quic_local
                    .or(binding.tcp_local)
                    .unwrap_or(binding.key);
                binding.stats.snapshot(local, binding.path)
            })
            .collect()
    }
}

impl Shared {
    fn stop(&self) {
        let state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        for binding in state.bindings.values() {
            binding.stop.send_replace(true);
        }
    }

    async fn update_inner(&self, policy: ListenPolicy, nics: Vec<Nic>) {
        let desired = bind_addrs(&policy, &nics);
        *self.policy.write().await = policy.clone();

        let desired_paths = desired.iter().copied().collect::<HashMap<_, _>>();
        let mut additions = Vec::new();
        let mut retries = Vec::new();
        {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(|error| error.into_inner());
            let old_keys = state.bindings.keys().copied().collect::<Vec<_>>();
            for key in old_keys {
                let keep = state
                    .bindings
                    .get(&key)
                    .is_some_and(|binding| desired_paths.get(&key) == Some(&binding.path));
                if !keep {
                    if let Some(binding) = state.bindings.remove(&key) {
                        let _ = binding.stop.send(true);
                    }
                }
            }
            for (key, path) in desired {
                if let Some(binding) = state.bindings.get(&key) {
                    if binding.quic_local.is_none() || binding.tcp_local.is_none() {
                        retries.push((
                            key,
                            path,
                            binding.quic_local.is_none(),
                            binding.tcp_local.is_none(),
                        ));
                    }
                } else {
                    additions.push((key, path));
                }
            }
        }

        for (key, path) in additions {
            if let Some((binding, listeners)) = bind_one(key, path, self.tls.clone()).await {
                let mut state = self
                    .state
                    .write()
                    .unwrap_or_else(|error| error.into_inner());
                // Publish and spawn under the same lock as Drop. If an update
                // is cancelled before its final closed check, no new binding
                // can escape a concurrent listener shutdown.
                if !self.events.is_closed() {
                    spawn_binding(
                        &binding,
                        self.events.clone(),
                        self.policy.clone(),
                        listeners,
                    );
                    state.bindings.insert(key, binding);
                }
            }
        }
        for (key, path, need_quic, need_tcp) in retries {
            self.retry_binding(key, path, need_quic, need_tcp).await;
        }
    }

    async fn retry_binding(
        &self,
        key: SocketAddr,
        path: PathClass,
        need_quic: bool,
        need_tcp: bool,
    ) {
        if need_quic {
            match QuicListener::bind(key, self.tls.as_ref().clone()).await {
                Ok(listener) => match listener.local_addr() {
                    Ok(local) => {
                        let should_spawn = {
                            let mut state = self
                                .state
                                .write()
                                .unwrap_or_else(|error| error.into_inner());
                            state.bindings.get_mut(&key).is_some_and(|binding| {
                                if binding.quic_local.is_none() && binding.path == path {
                                    binding.quic_local = Some(local);
                                    true
                                } else {
                                    false
                                }
                            })
                        };
                        if should_spawn {
                            if let Some(binding) = self.binding_snapshot(key) {
                                spawn_quic(
                                    &binding,
                                    self.events.clone(),
                                    self.policy.clone(),
                                    listener,
                                );
                            }
                        }
                    }
                    Err(error) => {
                        warn!(%key, %error, "failed to query retried QUIC listener address")
                    }
                },
                Err(error) => warn!(%key, %error, "failed to retry QUIC listener"),
            }
        }
        if need_tcp {
            let tcp_addr = self
                .binding_snapshot(key)
                .and_then(|binding| binding.quic_local)
                .unwrap_or(key);
            match TcpListener::bind(tcp_addr, self.tls.as_ref().clone()).await {
                Ok(listener) => match listener.local_addr() {
                    Ok(local) => {
                        let should_spawn = {
                            let mut state = self
                                .state
                                .write()
                                .unwrap_or_else(|error| error.into_inner());
                            state.bindings.get_mut(&key).is_some_and(|binding| {
                                if binding.tcp_local.is_none() && binding.path == path {
                                    binding.tcp_local = Some(local);
                                    true
                                } else {
                                    false
                                }
                            })
                        };
                        if should_spawn {
                            if let Some(binding) = self.binding_snapshot(key) {
                                spawn_tcp(
                                    &binding,
                                    self.events.clone(),
                                    self.policy.clone(),
                                    listener,
                                );
                            }
                        }
                    }
                    Err(error) => {
                        warn!(%tcp_addr, %error, "failed to query retried TCP listener address")
                    }
                },
                Err(error) => warn!(%tcp_addr, %error, "failed to retry TCP listener"),
            }
        }
    }

    fn binding_snapshot(&self, key: SocketAddr) -> Option<Binding> {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        state.bindings.get(&key).map(|binding| Binding {
            key: binding.key,
            path: binding.path,
            quic_local: binding.quic_local,
            tcp_local: binding.tcp_local,
            stats: binding.stats.clone(),
            stop: binding.stop.clone(),
        })
    }
}

struct BoundListeners {
    quic: Option<QuicListener>,
    tcp: Option<TcpListener>,
}

async fn bind_one(
    key: SocketAddr,
    path: PathClass,
    tls: Arc<rustls::ServerConfig>,
) -> Option<(Binding, BoundListeners)> {
    for attempt in 0..8 {
        let result = bind_pair(key, path, tls.clone()).await;
        // UDP and TCP have separate ephemeral-port namespaces. A UDP port
        // selected by the OS can already belong to another TCP listener.
        // Retry only ephemeral binds; fixed-port partial availability retains
        // the existing policy and update/retry behavior.
        let retry = key.port() == 0
            && result
                .as_ref()
                .is_some_and(|(_, listeners)| listeners.quic.is_some() && listeners.tcp.is_none());
        if !retry || attempt == 7 {
            return result;
        }
    }
    None
}

async fn bind_pair(
    key: SocketAddr,
    path: PathClass,
    tls: Arc<rustls::ServerConfig>,
) -> Option<(Binding, BoundListeners)> {
    let quic = match QuicListener::bind(key, tls.as_ref().clone()).await {
        Ok(listener) => match listener.local_addr() {
            Ok(local) => Some((listener, local)),
            Err(error) => {
                warn!(%key, %error, "failed to query QUIC listener address");
                None
            }
        },
        Err(error) => {
            warn!(%key, %error, "failed to bind QUIC listener");
            None
        }
    };
    let tcp_addr = quic.as_ref().map_or(key, |(_, local)| *local);
    let tcp = match TcpListener::bind(tcp_addr, tls.as_ref().clone()).await {
        Ok(listener) => match listener.local_addr() {
            Ok(local) => Some((listener, local)),
            Err(error) => {
                warn!(%tcp_addr, %error, "failed to query TCP listener address");
                None
            }
        },
        Err(error) => {
            warn!(%tcp_addr, %error, "failed to bind TCP listener");
            None
        }
    };
    if quic.is_none() && tcp.is_none() {
        return None;
    }
    let (stop, _) = watch::channel(false);
    let stats = Arc::new(AtomicStats::default());
    let (quic_listener, quic_local) = quic.map_or((None, None), |(listener, local)| {
        (Some(listener), Some(local))
    });
    let (tcp_listener, tcp_local) = tcp.map_or((None, None), |(listener, local)| {
        (Some(listener), Some(local))
    });
    Some((
        Binding {
            key,
            path,
            quic_local,
            tcp_local,
            stats,
            stop,
        },
        BoundListeners {
            quic: quic_listener,
            tcp: tcp_listener,
        },
    ))
}

fn spawn_binding(
    binding: &Binding,
    events: mpsc::Sender<Accepted>,
    policy: Arc<RwLock<ListenPolicy>>,
    listeners: BoundListeners,
) {
    if let Some(listener) = listeners.quic {
        spawn_quic(binding, events.clone(), policy.clone(), listener);
    }
    if let Some(listener) = listeners.tcp {
        spawn_tcp(binding, events, policy, listener);
    }
}

fn spawn_quic(
    binding: &Binding,
    events: mpsc::Sender<Accepted>,
    policy: Arc<RwLock<ListenPolicy>>,
    listener: QuicListener,
) {
    let local = binding.quic_local.unwrap_or(binding.key);
    let path = binding.path;
    let stats = binding.stats.clone();
    let mut stop = binding.stop.subscribe();
    tokio::spawn(async move {
        run_quic(listener, local, path, policy, stats, events, &mut stop).await;
    });
}

fn spawn_tcp(
    binding: &Binding,
    events: mpsc::Sender<Accepted>,
    policy: Arc<RwLock<ListenPolicy>>,
    listener: TcpListener,
) {
    let local = binding.tcp_local.unwrap_or(binding.key);
    let path = binding.path;
    let stats = binding.stats.clone();
    let mut stop = binding.stop.subscribe();
    tokio::spawn(async move {
        run_tcp(listener, local, path, policy, stats, events, &mut stop).await;
    });
}

async fn run_quic(
    listener: QuicListener,
    local: SocketAddr,
    bound_path: PathClass,
    policy: Arc<RwLock<ListenPolicy>>,
    stats: Arc<AtomicStats>,
    events: mpsc::Sender<Accepted>,
    stop: &mut watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            return;
        }
        let incoming = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return; }
                continue;
            }
            result = listener.accept() => match result {
                Ok(incoming) => incoming,
                Err(error) => {
                    warn!(%local, %error, "QUIC listener stopped");
                    return;
                }
            }
        };
        if *stop.borrow() {
            incoming.ignore();
            return;
        }
        let remote = incoming.remote_addr();
        let admitted = {
            let current = policy.read().await;
            admit(local, remote, &current).or_else(|| {
                // `Nic::tailnet` permits injected tailnet addresses used by
                // platform tests; their local IP is not in the fixed ranges.
                (bound_path == PathClass::Tailnet && current.tailnet).then_some(PathClass::Tailnet)
            })
        };
        let Some(path) = admitted else {
            incoming.ignore();
            stats.ignored.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let stats = stats.clone();
        let events = events.clone();
        tokio::spawn(async move {
            match incoming.accept().await {
                Ok(conn) => {
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                    let _ = events
                        .send(Accepted {
                            conn,
                            tcp_tokens: None,
                            path,
                            local,
                            remote,
                        })
                        .await;
                }
                Err(error) => {
                    stats.tls_failed.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(%local, %error, "QUIC TLS handshake failed");
                }
            }
        });
    }
}

async fn run_tcp(
    listener: TcpListener,
    local: SocketAddr,
    bound_path: PathClass,
    policy: Arc<RwLock<ListenPolicy>>,
    stats: Arc<AtomicStats>,
    events: mpsc::Sender<Accepted>,
    stop: &mut watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            return;
        }
        let incoming = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return; }
                continue;
            }
            result = listener.accept() => match result {
                Ok(incoming) => incoming,
                Err(error) => {
                    warn!(%local, %error, "TCP listener stopped");
                    return;
                }
            }
        };
        if *stop.borrow() {
            incoming.ignore();
            return;
        }
        let remote = incoming.remote_addr();
        let admitted = {
            let current = policy.read().await;
            admit(local, remote, &current).or_else(|| {
                (bound_path == PathClass::Tailnet && current.tailnet).then_some(PathClass::Tailnet)
            })
        };
        if admitted.is_none() {
            incoming.ignore();
            stats.ignored.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let Some(path) = admitted else {
            continue;
        };
        let stats = stats.clone();
        let events = events.clone();
        let tcp_tokens = listener.token_registry();
        tokio::spawn(async move {
            match incoming.accept().await {
                Ok(Some(conn)) => {
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                    let _ = events
                        .send(Accepted {
                            conn,
                            tcp_tokens: Some(tcp_tokens),
                            path,
                            local,
                            remote,
                        })
                        .await;
                }
                Ok(None) => {
                    stats.input_attachments.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    stats.tls_failed.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(%local, %error, "TCP TLS handshake failed");
                }
            }
        });
    }
}
