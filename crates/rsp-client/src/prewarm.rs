//! At most four QUIC/TLS handshakes, without opening an RSP control stream.
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rsp_transport::dial::{self, AttemptLog, Candidate, DialFailure, DialOpts, DialPlan};
use rsp_transport::dialer::Dialer;
use rsp_transport::{CloseCode, Conn, ConnError, TransportKind};
use thiserror::Error;

pub trait Clock: Send + Sync {
    fn now_us(&self) -> u64;
}
pub struct SystemClock {
    start: std::time::Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

#[derive(Debug, Error)]
pub enum PrewarmError {
    #[error("prewarm requires a QUIC connection")]
    RequiresQuic,
    #[error("prewarm Host fingerprint does not match the authenticated peer")]
    FingerprintMismatch,
    #[error(transparent)]
    Dial(#[from] DialFailure),
}
/// The dial result behind a pooled connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmReport {
    pub path: Candidate,
    pub transport: TransportKind,
    pub attempts: Vec<AttemptLog>,
}
struct Entry {
    fingerprint: [u8; 32],
    conn: Conn,
    inserted_us: u64,
    keeper: Option<Arc<dyn Dialer>>,
}
struct Inner {
    entries: Mutex<Vec<Entry>>,
    clock: Arc<dyn Clock>,
    capacity: usize,
    ttl_us: u64,
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
impl Inner {
    fn expire(&self, now: u64) -> usize {
        let mut entries = lock(&self.entries);
        let before = entries.len();
        entries.retain(|entry| {
            if now.saturating_sub(entry.inserted_us) >= self.ttl_us {
                entry.conn.close(CloseCode::Normal, b"prewarm expired");
                false
            } else {
                true
            }
        });
        before - entries.len()
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        for entry in lock(&self.entries).drain(..) {
            entry.conn.close(CloseCode::Normal, b"prewarm pool dropped");
        }
    }
}

#[derive(Clone)]
pub struct PrewarmPool {
    inner: Arc<Inner>,
}
impl Default for PrewarmPool {
    fn default() -> Self {
        Self::new()
    }
}
impl PrewarmPool {
    pub const CAPACITY: usize = 4;
    pub const TTL_US: u64 = 60_000_000;
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock::default()))
    }
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::with_limits(clock, Self::CAPACITY, Self::TTL_US)
    }
    pub fn with_limits(clock: Arc<dyn Clock>, capacity: usize, ttl_us: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(Vec::new()),
                clock,
                capacity: capacity.min(Self::CAPACITY),
                ttl_us,
            }),
        }
    }
    pub fn insert(&self, fingerprint: [u8; 32], conn: Conn) -> Result<(), PrewarmError> {
        self.store(fingerprint, conn, None)
    }
    fn store(
        &self,
        fingerprint: [u8; 32],
        conn: Conn,
        keeper: Option<Arc<dyn Dialer>>,
    ) -> Result<(), PrewarmError> {
        if conn.kind() != TransportKind::Quic {
            conn.close(CloseCode::Normal, b"prewarm requires QUIC");
            return Err(PrewarmError::RequiresQuic);
        }
        if rsp_transport::tls::peer_fingerprint(&conn.peer_certificates()) != Some(fingerprint) {
            conn.close(CloseCode::Auth, b"prewarm identity mismatch");
            return Err(PrewarmError::FingerprintMismatch);
        }
        let now = self.inner.clock.now_us();
        self.inner.expire(now);
        let mut entries = lock(&self.inner.entries);
        if let Some(index) = entries.iter().position(|e| e.fingerprint == fingerprint) {
            entries
                .remove(index)
                .conn
                .close(CloseCode::Normal, b"prewarm replaced");
        }
        if self.inner.capacity == 0 {
            conn.close(CloseCode::Normal, b"prewarm disabled");
            return Ok(());
        }
        while entries.len() >= self.inner.capacity {
            entries
                .remove(0)
                .conn
                .close(CloseCode::Normal, b"prewarm pool full");
        }
        let _ = conn.set_keepalive(Duration::from_secs(15));
        entries.push(Entry {
            fingerprint,
            conn,
            inserted_us: now,
            keeper,
        });
        drop(entries);
        let weak = Arc::downgrade(&self.inner);
        let ttl = Duration::from_micros(self.inner.ttl_us);
        // A timer owns only a Weak reference, so dropping a pool closes its
        // entries immediately. Injected clocks can also expire explicitly.
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            if let Some(inner) = weak.upgrade() {
                inner.expire(inner.clock.now_us());
            }
        });
        Ok(())
    }
    pub async fn warm(
        &self,
        plan: &DialPlan,
        dialer: Arc<dyn Dialer>,
        fingerprint: [u8; 32],
    ) -> Result<(), PrewarmError> {
        self.warm_report(plan, dialer, fingerprint).await.map(drop)
    }
    pub async fn warm_report(
        &self,
        plan: &DialPlan,
        dialer: Arc<dyn Dialer>,
        fingerprint: [u8; 32],
    ) -> Result<WarmReport, PrewarmError> {
        let quic = QuicOnly(Arc::clone(&dialer));
        let result = dial::race(plan, &quic, fingerprint, &DialOpts::default()).await?;
        if let Some((backup, _)) = result.backup {
            backup.close(CloseCode::Normal, b"prewarm spare discarded");
        }
        self.store(fingerprint, result.primary, Some(dialer))?;
        Ok(WarmReport {
            path: result.primary_path,
            transport: result.transport,
            attempts: result.attempts,
        })
    }
    pub(crate) fn take_owned(
        &self,
        fingerprint: [u8; 32],
    ) -> Option<(Conn, Option<Arc<dyn Dialer>>)> {
        self.expire();
        let mut entries = lock(&self.inner.entries);
        let index = entries.iter().position(|e| e.fingerprint == fingerprint)?;
        let entry = entries.remove(index);
        Some((entry.conn, entry.keeper))
    }
    pub fn take(&self, fingerprint: [u8; 32]) -> Option<Conn> {
        self.take_owned(fingerprint).map(|(conn, _)| conn)
    }
    pub fn expire(&self) -> usize {
        self.inner.expire(self.inner.clock.now_us())
    }
    pub fn len(&self) -> usize {
        self.expire();
        lock(&self.inner.entries).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }
    pub fn ttl_us(&self) -> u64 {
        self.inner.ttl_us
    }
}
struct QuicOnly(Arc<dyn Dialer>);
impl Dialer for QuicOnly {
    fn dial<'a>(
        &'a self,
        addr: std::net::SocketAddr,
        kind: TransportKind,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Conn, ConnError>> + Send + 'a>>
    {
        Box::pin(async move {
            if kind != TransportKind::Quic {
                return Err(ConnError::Unsupported("prewarm is QUIC only"));
            }
            self.0.dial(addr, kind).await
        })
    }
}
