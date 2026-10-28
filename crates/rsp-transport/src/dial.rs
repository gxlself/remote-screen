//! Candidate planning and Happy Eyeballs (protocol §10.1–10.3).
//!
//! No control stream is opened here. Every returned connection has passed a
//! post-handshake SPKI check, including connections supplied by a custom Dialer.

use std::{
    collections::BTreeMap,
    fmt,
    future::{poll_fn, Future},
    net::{IpAddr, SocketAddr},
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::Poll,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rsp_core::errors::ErrorCode;
use thiserror::Error;
use tokio::time::{sleep_until, Instant};

use crate::{
    dialer::Dialer,
    identity::LocalIdentity,
    net::{system_net, NetDialer},
    tls::{client_config, peer_fingerprint, ServerPin, TlsError},
    CloseCode, Conn, ConnError, TransportKind,
};

/// An interface address and its prefix. Kept local to avoid adding a network
/// enumeration dependency; callers supply the OS's interface snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix_len: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid IP address or network prefix")]
pub struct InvalidNet;

impl IpNet {
    pub fn new(addr: IpAddr, prefix_len: u8) -> Result<Self, InvalidNet> {
        if prefix_len > if addr.is_ipv4() { 32 } else { 128 } {
            return Err(InvalidNet);
        }
        Ok(Self { addr, prefix_len })
    }

    pub fn addr(&self) -> IpAddr {
        self.addr
    }

    pub fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.addr, addr) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                self.prefix_len == 0
                    || (u32::from(net) ^ u32::from(ip)) >> (32 - self.prefix_len) == 0
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                self.prefix_len == 0
                    || (u128::from(net) ^ u128::from(ip)) >> (128 - self.prefix_len) == 0
            }
            _ => false,
        }
    }
}

impl FromStr for IpNet {
    type Err = InvalidNet;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (addr, prefix) = value.split_once('/').ok_or(InvalidNet)?;
        Self::new(
            addr.parse().map_err(|_| InvalidNet)?,
            prefix.parse().map_err(|_| InvalidNet)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathKind {
    LastGood(Box<PathKind>),
    Lan,
    Intranet,
    Tailnet,
    Manual,
}

impl PathKind {
    /// Cached paths retain their original type for reachability and backup choice.
    pub fn original(&self) -> &Self {
        let mut kind = self;
        while let Self::LastGood(inner) = kind {
            kind = inner;
        }
        kind
    }

    pub fn priority(&self) -> u8 {
        match self {
            Self::LastGood(_) => 0,
            Self::Lan => 1,
            Self::Intranet => 2,
            Self::Tailnet => 3,
            Self::Manual => 4,
        }
    }

    fn order(&self) -> (u8, u8) {
        (self.priority(), self.original().priority())
    }

    fn register_intranet(&mut self, ip: IpAddr, cidrs: &[IpNet]) {
        match self {
            Self::LastGood(inner) => inner.register_intranet(ip, cidrs),
            Self::Lan if cidrs.iter().any(|net| net.contains(ip)) => *self = Self::Intranet,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    Mdns,
    Manual,
    Cache,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub addr: SocketAddr,
    pub kind: PathKind,
    pub source: CandidateSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IfaceInfo {
    pub name: String,
    pub addrs: Vec<IpNet>,
    pub is_vpn: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalNet {
    pub interfaces: Vec<IfaceInfo>,
}

impl LocalNet {
    pub fn is_direct(&self, addr: IpAddr) -> bool {
        self.interfaces
            .iter()
            .flat_map(|iface| &iface.addrs)
            .any(|net| net.contains(addr))
    }

    pub fn has_vpn(&self) -> bool {
        self.interfaces.iter().any(|iface| iface.is_vpn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    NeedsVpn,
    NotOnLan,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialPlan {
    /// Deduplicated and sorted; skipped addresses never reach the Dialer.
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<(Candidate, Skipped)>,
    /// Explicit caller knowledge, not inferred from a tailnet IP. PeerInfo is
    /// unavailable before the handshake and tailnet may also connect directly.
    pub derp_only: bool,
}

pub fn plan(cands: Vec<Candidate>, local: &LocalNet, intranet_cidrs: &[IpNet]) -> DialPlan {
    let mut merged: BTreeMap<SocketAddr, Candidate> = BTreeMap::new();
    for mut candidate in cands {
        candidate
            .kind
            .register_intranet(candidate.addr.ip(), intranet_cidrs);
        match merged.get(&candidate.addr) {
            Some(old) if old.kind.order() <= candidate.kind.order() => {}
            _ => {
                merged.insert(candidate.addr, candidate);
            }
        }
    }
    let mut candidates: Vec<_> = merged.into_values().collect();
    candidates.sort_by_key(|candidate| (candidate.kind.order(), candidate.addr));
    let mut result = DialPlan::default();
    for candidate in candidates {
        let direct = local.is_direct(candidate.addr.ip());
        let skipped = match candidate.kind.original() {
            PathKind::Intranet if !direct && !local.has_vpn() => Some(Skipped::NeedsVpn),
            PathKind::Lan if !direct => Some(Skipped::NotOnLan),
            _ => None,
        };
        if let Some(reason) = skipped {
            result.skipped.push((candidate, reason));
        } else {
            result.candidates.push(candidate);
        }
    }
    result
}

#[derive(Debug, Clone)]
pub struct DialOpts {
    pub stagger: Duration,
    pub tcp_after: Duration,
    pub grace: Duration,
    pub timeout: Duration,
    pub derp_only_timeout: Duration,
}

impl Default for DialOpts {
    fn default() -> Self {
        Self {
            stagger: Duration::from_millis(25),
            tcp_after: Duration::from_millis(250),
            grace: Duration::from_millis(30),
            timeout: Duration::from_secs(3),
            derp_only_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptResult {
    NotStarted,
    Connected,
    TimedOut,
    Refused,
    TlsFailed,
    FingerprintMismatch { got: [u8; 32] },
    Failed,
    Cancelled,
    Skipped(Skipped),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptLog {
    pub candidate: Candidate,
    /// None for a candidate skipped before either transport is attempted.
    pub transport: Option<TransportKind>,
    pub result: AttemptResult,
    pub started_at_us: Option<u64>,
    pub finished_at_us: Option<u64>,
}

pub struct DialOutcome {
    pub primary: Conn,
    pub primary_path: Candidate,
    pub transport: TransportKind,
    /// Different original path type OR different transport. LastGood is a
    /// preference, not a separate path type. Neither connection opens control.
    pub backup: Option<(Conn, Candidate)>,
    pub attempts: Vec<AttemptLog>,
}

impl fmt::Debug for DialOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialOutcome")
            .field("primary", &self.primary.remote_addr())
            .field("primary_path", &self.primary_path)
            .field("transport", &self.transport)
            .field(
                "backup",
                &self
                    .backup
                    .as_ref()
                    .map(|(conn, candidate)| (conn.remote_addr(), conn.kind(), candidate)),
            )
            .field("attempts", &self.attempts)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("no candidate completed an authenticated RSP handshake")]
pub struct DialFailure {
    pub attempts: Vec<AttemptLog>,
}

/// Real mutual TLS dialer. The caller selects the network and whether QUIC
/// candidates share an endpoint or obtain a separate socket for each peer.
pub struct DefaultDialer {
    quic: Option<quinn::Endpoint>,
    net: Arc<dyn NetDialer>,
    config: Arc<rustls::ClientConfig>,
}

impl DefaultDialer {
    pub fn new(endpoint: quinn::Endpoint, local: &LocalIdentity) -> Result<Self, TlsError> {
        Self::with_net(system_net(), Some(endpoint), local)
    }

    pub fn with_net(
        net: Arc<dyn NetDialer>,
        shared: Option<quinn::Endpoint>,
        local: &LocalIdentity,
    ) -> Result<Self, TlsError> {
        // Verify certificate signatures during TLS, then enforce the expected
        // identity in race, before any connection is exposed or stream opened.
        // This also preserves E150: ConnError otherwise erases rustls's typed
        // pin mismatch into a String in connect_quic/connect_tcp.
        Ok(Self {
            quic: shared,
            net,
            config: Arc::new(client_config(local, ServerPin::Any)?),
        })
    }
}

impl Dialer for DefaultDialer {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async move {
            match kind {
                TransportKind::Quic => {
                    let endpoint;
                    let shared = match &self.quic {
                        Some(shared) => shared,
                        None => {
                            let socket = self.net.dial_udp(addr).await?;
                            endpoint = quinn::Endpoint::new_with_abstract_socket(
                                quinn::EndpointConfig::default(),
                                None,
                                socket,
                                Arc::new(quinn::TokioRuntime),
                            )?;
                            &endpoint
                        }
                    };
                    crate::connect_quic(shared, addr, self.config.clone(), true).await
                }
                TransportKind::Tcp => {
                    crate::connect_tcp_via(self.net.clone(), addr, self.config.clone()).await
                }
            }
        })
    }
}

// Conn has background tasks and cloning/dropping it alone does not close it.
// Keep a synchronous close guard across every await, including final cleanup.
struct OwnedConn(Option<Conn>);

impl OwnedConn {
    fn get(&self) -> &Conn {
        self.0
            .as_ref()
            .expect("owned connection has not been returned")
    }

    fn take(mut self) -> Conn {
        self.0.take().expect("owned connection is returned once")
    }
}

impl Drop for OwnedConn {
    fn drop(&mut self) {
        if let Some(conn) = &self.0 {
            conn.close(CloseCode::Normal, b"dial attempt discarded");
        }
    }
}

type Handshake<'a> = Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>>;

struct Pending<'a> {
    log: usize,
    future: Handshake<'a>,
}

async fn next_completed(pending: &mut Vec<Pending<'_>>) -> (usize, Result<OwnedConn, ConnError>) {
    poll_fn(|cx| {
        for index in 0..pending.len() {
            if let Poll::Ready(result) = pending[index].future.as_mut().poll(cx) {
                let attempt = pending.swap_remove(index);
                return Poll::Ready((attempt.log, result.map(|conn| OwnedConn(Some(conn)))));
            }
        }
        Poll::Pending
    })
    .await
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

fn failed_attempt(error: ConnError) -> AttemptResult {
    match error {
        ConnError::Io(error) => match error.kind() {
            std::io::ErrorKind::TimedOut => AttemptResult::TimedOut,
            std::io::ErrorKind::ConnectionRefused => AttemptResult::Refused,
            _ => AttemptResult::Failed,
        },
        ConnError::Tls(message) if message.contains("timed out") => AttemptResult::TimedOut,
        ConnError::Tls(_) | ConnError::MissingAlpn => AttemptResult::TlsFailed,
        // connect_quic erases ConnectionError. Retain useful classifications
        // without treating arbitrary TLS/certificate failures as pin mismatches.
        ConnError::Quic(message) if message.contains("timed out") => AttemptResult::TimedOut,
        ConnError::Quic(message) if message.contains("cryptographic handshake failed") => {
            AttemptResult::TlsFailed
        }
        _ => AttemptResult::Failed,
    }
}

async fn at(deadline: Instant) {
    // sleep_until(now) can otherwise wait for the next timer tick. In
    // particular, the very first QUIC attempt must start in this poll at t=0.
    if Instant::now() < deadline {
        sleep_until(deadline).await;
    }
}

/// Race only through Dialer. Its handshake futures must release in-flight I/O
/// when dropped (as connect_quic/connect_tcp do). No detached handshake tasks
/// are spawned; dropping race drops all handshakes and closes all owned Conns.
pub async fn race(
    plan: &DialPlan,
    dialer: &dyn Dialer,
    expected_fp: [u8; 32],
    opts: &DialOpts,
) -> Result<DialOutcome, DialFailure> {
    let start = Instant::now();
    let deadline = start
        + if plan.derp_only {
            opts.derp_only_timeout
        } else {
            opts.timeout
        };
    let mut attempts = Vec::new();
    for candidate in &plan.candidates {
        for transport in [TransportKind::Quic, TransportKind::Tcp] {
            attempts.push(AttemptLog {
                candidate: candidate.clone(),
                transport: Some(transport),
                result: AttemptResult::NotStarted,
                started_at_us: None,
                finished_at_us: None,
            });
        }
    }
    for (candidate, skipped) in &plan.skipped {
        attempts.push(AttemptLog {
            candidate: candidate.clone(),
            transport: None,
            result: AttemptResult::Skipped(*skipped),
            started_at_us: None,
            finished_at_us: None,
        });
    }
    let mut pending = Vec::new();
    let mut ready: Vec<(usize, OwnedConn)> = Vec::new();
    let mut rejected = Vec::new();
    let mut next_quic = 0;
    let mut next_tcp = 0;
    let mut tcp_enabled = false;
    let mut grace_end = None;
    loop {
        let quic_due = start + opts.stagger.mul_f64(next_quic as f64);
        let tcp_due = start + opts.tcp_after + opts.stagger.mul_f64(next_tcp as f64);
        let more_quic = next_quic < plan.candidates.len();
        let more_tcp = next_tcp < plan.candidates.len();
        if pending.is_empty() && !more_quic && !more_tcp && ready.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            // A delayed executor poll must not accept a handshake after the
            // total budget, even when both futures have become ready.
            _ = at(deadline) => break,
            // Process completions on the grace boundary before selecting a winner.
            (log, result) = next_completed(&mut pending), if !pending.is_empty() => {
                attempts[log].finished_at_us = Some(now_us());
                match result {
                    Ok(conn) => {
                        let fp = peer_fingerprint(&conn.get().peer_certificates());
                        if fp == Some(expected_fp) {
                            attempts[log].result = AttemptResult::Connected;
                            ready.push((log, conn));
                            grace_end.get_or_insert_with(|| Instant::now() + opts.grace);
                        } else {
                            attempts[log].result = match fp {
                                Some(got) => AttemptResult::FingerprintMismatch { got },
                                None => AttemptResult::TlsFailed,
                            };
                            conn.get().close(CloseCode::Auth, b"Host identity mismatch");
                            rejected.push(conn);
                        }
                    }
                    Err(error) => attempts[log].result = failed_attempt(error),
                }
            }
            _ = at(grace_end.unwrap_or(deadline)), if grace_end.is_some() => break,
            _ = at(quic_due), if more_quic => {
                let log = next_quic * 2;
                attempts[log].started_at_us = Some(now_us());
                pending.push(Pending {
                    log,
                    future: dialer.dial(plan.candidates[next_quic].addr, TransportKind::Quic),
                });
                next_quic += 1;
            }
            _ = at(tcp_due), if more_tcp && (tcp_enabled || ready.is_empty()) => {
                // At t=250ms only an authenticated QUIC handshake suppresses TCP.
                // Once fallback starts, its staggered sequence runs through grace.
                tcp_enabled = true;
                let log = next_tcp * 2 + 1;
                attempts[log].started_at_us = Some(now_us());
                pending.push(Pending {
                    log,
                    future: dialer.dial(plan.candidates[next_tcp].addr, TransportKind::Tcp),
                });
                next_tcp += 1;
            }
        }
    }

    for pending in &pending {
        attempts[pending.log].result = if Instant::now() >= deadline {
            AttemptResult::TimedOut
        } else {
            AttemptResult::Cancelled
        };
        attempts[pending.log].finished_at_us = Some(now_us());
    }
    if ready.is_empty() && Instant::now() >= deadline {
        for attempt in &mut attempts {
            if attempt.result == AttemptResult::NotStarted {
                // The total budget can expire before a long candidate list
                // reaches its launch slot. It still has a timeout conclusion;
                // started_at_us=None distinguishes it from an actual handshake.
                attempt.result = AttemptResult::TimedOut;
                attempt.finished_at_us = Some(now_us());
            }
        }
    }
    // Cancel handshakes before awaiting established-connection cleanup.
    drop(pending);
    // Stable sorting preserves the first completion on equal priority. Only a
    // higher-priority path may displace it during grace, per §10.2.
    ready.sort_by_key(|(log, _)| attempts[*log].candidate.kind.order());
    let primary = if ready.is_empty() {
        None
    } else {
        Some(ready.remove(0))
    };
    let backup = primary.as_ref().and_then(|(primary_log, _)| {
        let primary = &attempts[*primary_log];
        let index = ready.iter().position(|(log, _)| {
            let other = &attempts[*log];
            other.candidate.kind.original() != primary.candidate.kind.original()
                || other.transport != primary.transport
        })?;
        Some(ready.remove(index))
    });
    rejected.extend(ready.into_iter().map(|(_, conn)| conn));
    for conn in &rejected {
        conn.get().close(CloseCode::Normal, b"dial race completed");
    }
    for conn in &rejected {
        conn.get().closed().await;
    }
    match primary {
        Some((log, primary)) => {
            if let Some((_, backup)) = &backup {
                if backup.get().kind() == TransportKind::Quic {
                    // TCP's Conn API delegates keep-alive to the session layer.
                    let _ = backup.get().set_keepalive(Duration::from_secs(15));
                }
            }
            let transport = primary.get().kind();
            Ok(DialOutcome {
                primary: primary.take(),
                primary_path: attempts[log].candidate.clone(),
                transport,
                backup: backup.map(|(log, conn)| (conn.take(), attempts[log].candidate.clone())),
                attempts,
            })
        }
        None => Err(DialFailure { attempts }),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Observations {
    pub host_version_ok: Option<bool>,
    pub failure: Option<DialFailure>,
    pub local: LocalNet,
    pub plan: DialPlan,
}

/// Precedence: version, authenticated identity mismatch, missing VPN, routed
/// intranet timeout. Unknown observations and
/// unrelated TLS/I/O failures map to the generic page; they must not be
/// presented as evidence that a policy blocked the connection.
pub fn classify_failure(obs: &Observations) -> ErrorCode {
    if obs.host_version_ok == Some(false) {
        return ErrorCode::HostTooOld;
    }
    let attempts = obs
        .failure
        .as_ref()
        .map_or(&[][..], |failure| failure.attempts.as_slice());
    let mismatches = attempts
        .iter()
        .any(|attempt| matches!(attempt.result, AttemptResult::FingerprintMismatch { .. }));
    if mismatches
        && !attempts
            .iter()
            .any(|attempt| attempt.result == AttemptResult::Connected)
    {
        return ErrorCode::FingerprintMismatch;
    }
    let candidates: Vec<_> = obs
        .plan
        .candidates
        .iter()
        .chain(obs.plan.skipped.iter().map(|(candidate, _)| candidate))
        .chain(attempts.iter().map(|attempt| &attempt.candidate))
        .collect();
    if !candidates.is_empty()
        && !obs.local.has_vpn()
        && candidates.iter().all(|candidate| {
            *candidate.kind.original() == PathKind::Intranet
                && !obs.local.is_direct(candidate.addr.ip())
        })
    {
        return ErrorCode::VpnMissing;
    }
    let both_timeout = |candidate: &Candidate| {
        [TransportKind::Quic, TransportKind::Tcp]
            .into_iter()
            .all(|transport| {
                attempts.iter().any(|attempt| {
                    attempt.candidate.addr == candidate.addr
                        && attempt.transport == Some(transport)
                        && attempt.started_at_us.is_some()
                        && attempt.result == AttemptResult::TimedOut
                })
            })
    };
    if candidates.iter().any(|candidate| {
        *candidate.kind.original() == PathKind::Intranet
            && (obs.local.has_vpn() || obs.local.is_direct(candidate.addr.ip()))
            && both_timeout(candidate)
    }) {
        return ErrorCode::PortBlocked;
    }
    ErrorCode::Other
}
