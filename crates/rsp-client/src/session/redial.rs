//! Dial inputs and cancellable §10.2 races used by the session runner.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use prost::Message;
use rsp_core::pb;
use rsp_transport::dial::{self, Candidate, DialFailure, DialOpts, DialOutcome, IpNet, LocalNet};
use rsp_transport::dialer::Dialer;
use rsp_transport::{CloseCode, Conn, ConnError, ControlStream, TransportKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub(super) struct DialContext {
    pub candidates: Vec<Candidate>,
    pub local_net: LocalNet,
    pub intranet_cidrs: Vec<IpNet>,
    pub derp_only: bool,
    pub dial_options: DialOpts,
    pub host_fingerprint: [u8; 32],
    pub dialer: Arc<dyn Dialer>,
}

impl DialContext {
    pub fn plan(&self, candidates: Vec<Candidate>) -> dial::DialPlan {
        let mut plan = dial::plan(candidates, &self.local_net, &self.intranet_cidrs);
        plan.derp_only = self.derp_only;
        plan
    }

    /// The same race with every TCP attempt failing at once, for upgrade probes.
    pub fn quic_only(&self) -> Self {
        Self {
            dialer: Arc::new(QuicOnly(Arc::clone(&self.dialer))),
            ..self.clone()
        }
    }
}

struct QuicOnly(Arc<dyn Dialer>);

impl Dialer for QuicOnly {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async move {
            if kind != TransportKind::Quic {
                return Err(ConnError::Unsupported("upgrade probe is QUIC only"));
            }
            self.0.dial(addr, kind).await
        })
    }
}

pub(super) type DialResult = Result<OwnedOutcome, DialFailure>;

/// A result can already be queued when its caller cancels the race. Conn has
/// background tasks, so dropping the channel must explicitly close its paths.
pub(super) struct OwnedOutcome(Option<DialOutcome>);

impl OwnedOutcome {
    pub fn take(mut self) -> DialOutcome {
        self.0.take().expect("dial outcome is returned once")
    }
}

impl Drop for OwnedOutcome {
    fn drop(&mut self) {
        if let Some(outcome) = self.0.take() {
            outcome
                .primary
                .close(CloseCode::Normal, b"dial result discarded");
            if let Some((backup, _)) = outcome.backup {
                backup.close(CloseCode::Normal, b"dial result discarded");
            }
        }
    }
}

pub(super) struct DialTask {
    task: JoinHandle<()>,
    result: mpsc::UnboundedReceiver<DialResult>,
}

impl DialTask {
    pub fn spawn(context: DialContext, candidates: Vec<Candidate>) -> Self {
        let (tx, result) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let plan = context.plan(candidates);
            let outcome = dial::race(
                &plan,
                context.dialer.as_ref(),
                context.host_fingerprint,
                &context.dial_options,
            )
            .await
            .map(|outcome| OwnedOutcome(Some(outcome)));
            let _ = tx.send(outcome);
        });
        Self { task, result }
    }

    pub async fn recv(&mut self) -> DialResult {
        self.result.recv().await.unwrap_or_else(|| {
            Err(DialFailure {
                attempts: Vec::new(),
            })
        })
    }

    pub async fn cancel(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
        // Dropping self.result closes every queued OwnedOutcome. Dropping a
        // race also closes handshakes that completed inside dial::race.
    }
}

impl Drop for DialTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The candidate's control stream after the Host answered the upgrade resume.
pub(super) struct Resumed {
    pub control: ControlStream,
    pub hello: Option<pb::ServerHello>,
    pub auth: pb::AuthResult,
}

pub(super) type ResumeResult = Result<Resumed, ConnError>;

/// Sends the resume `ClientHello` on a candidate connection and reads the
/// Host's answer without blocking the primary path.
pub(super) struct ResumeTask {
    task: JoinHandle<()>,
    result: mpsc::UnboundedReceiver<ResumeResult>,
}

impl ResumeTask {
    pub fn spawn(conn: Conn, hello: Vec<u8>) -> Self {
        let (tx, result) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let _ = tx.send(resume(conn, hello).await);
        });
        Self { task, result }
    }

    pub async fn recv(&mut self) -> ResumeResult {
        self.result.recv().await.unwrap_or(Err(ConnError::Closed))
    }
}

impl Drop for ResumeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn resume(conn: Conn, hello: Vec<u8>) -> ResumeResult {
    let control = conn.open_control().await?;
    control.send(&hello).await?;
    let mut server_hello = None;
    loop {
        let bytes = control.recv().await?.ok_or(ConnError::Closed)?;
        let envelope =
            pb::Envelope::decode(bytes).map_err(|error| ConnError::Protocol(error.to_string()))?;
        match envelope.msg {
            Some(pb::envelope::Msg::ServerHello(hello)) => server_hello = Some(hello),
            Some(pb::envelope::Msg::AuthResult(auth)) => {
                return Ok(Resumed {
                    control,
                    hello: server_hello,
                    auth,
                })
            }
            _ => tracing::debug!("ignoring control message before upgrade AuthResult"),
        }
    }
}
