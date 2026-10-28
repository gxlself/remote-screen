//! Resilient SSH shells and the native terminal byte pump.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use rs_ssh::reconnect::{Connector, ReconnectPolicy, ResilientShell, TerminalEvent};
use rs_ssh::{PtyOpts, SshError, SshSession};
use tokio::sync::{broadcast, mpsc, watch};
use zeroize::Zeroizing;

use super::api_ssh::{
    connect_once, lock, reject_prompts, resolve_server, CorePrompter, PromptRelay,
};
use super::dto_core::{id_str, CoreEvent, SshOpenOpts, SshState};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::sshbook::{tmux_id, ResolvedTarget};
use super::tables::{SshEntry, SshTable, TermInput, TermTap};
use crate::uniffi_api::EventSink;

#[derive(Default)]
pub struct SshTuning(pub Mutex<ReconnectPolicy>);

pub const TMUX_UNAVAILABLE: &[u8] = b"\r\n[rs] tmux unavailable, using a plain shell\r\n";

/// Serialize state changes, including callbacks from the authentication adapter.
/// A terminal state is irreversible and is published at most once.
pub fn emit_state(core: &Core, entry: &SshEntry, state: SshState, message: Option<String>) {
    let mut current = lock(&entry.state);
    if matches!(*current, SshState::Closed | SshState::Failed) {
        return;
    }
    *current = state;
    core.events().emit(&CoreEvent::SshState {
        ssh_id: id_str(entry.id),
        server_id: entry.server_id.clone(),
        state,
        message,
    });
}

pub fn error_kind(error: &SshError) -> &'static str {
    match error {
        SshError::AuthFailed { .. } => "auth_failed",
        SshError::HostKeyChanged { .. } => "host_key_changed",
        SshError::HostKeyRejected => "host_key_rejected",
        SshError::Unsupported(_) => "unsupported",
        SshError::InvalidKey => "invalid_key",
        SshError::SigningFailed => "signing_failed",
        SshError::Timeout => "timeout",
        SshError::Cancelled => "cancelled",
        SshError::InvalidOptions(_) => "invalid_options",
        SshError::RequestRejected => "request_rejected",
        SshError::Closed => "closed",
        SshError::Transport(_) => "transport",
    }
}

pub fn cancel_entry(entry: &SshEntry) {
    entry.cancelled.store(true, Ordering::Release);
    entry.cancel.notify_waiters();
}

/// Register before reading the flag so immediate close cannot lose a wakeup.
pub async fn cancelled(entry: &SshEntry) {
    let notified = entry.cancel.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if !entry.cancelled.load(Ordering::Acquire) {
        notified.await;
    }
}

/// `connect_once` takes a concrete CorePrompter. Observe its URL events and its
/// keyboard flag without replacing it or routing terminal bytes through events.
pub struct AuthenticationState {
    pub core: Weak<Core>,
    pub entry: Weak<SshEntry>,
    pub asked: AtomicBool,
    pub delivered: watch::Sender<u64>,
}

impl AuthenticationState {
    pub fn authenticating(&self) {
        if self.asked.swap(true, Ordering::AcqRel) {
            return;
        }
        if let (Some(core), Some(entry)) = (self.core.upgrade(), self.entry.upgrade()) {
            emit_state(&core, &entry, SshState::Authenticating, None);
        }
    }
}

impl EventSink for AuthenticationState {
    fn on_event(&self, seq: u64, payload: String) {
        if let Ok(CoreEvent::SshAuthUrl { ssh_id, .. }) = serde_json::from_str(&payload) {
            if self
                .entry
                .upgrade()
                .is_some_and(|entry| ssh_id == id_str(entry.id))
            {
                self.authenticating();
            }
        }
        self.delivered.send_replace(seq);
    }
}

pub struct CoreConnector {
    pub core: Arc<Core>,
    pub resolved: ResolvedTarget,
    pub prompter: Arc<CorePrompter>,
    pub authentication: Arc<AuthenticationState>,
}

impl Connector for CoreConnector {
    fn connect(&self) -> Pin<Box<dyn Future<Output = Result<SshSession, SshError>> + Send + '_>> {
        Box::pin(async move {
            let connect = connect_once(&self.core, &self.resolved, self.prompter.clone());
            tokio::pin!(connect);
            let result = poll_fn(|cx| {
                let result = connect.as_mut().poll(cx);
                if self.prompter.relay.keyboard_asked.load(Ordering::Acquire) {
                    self.authentication.authenticating();
                }
                result
            })
            .await;
            // URL callbacks run on EventHub's dispatcher. Let it consume events
            // already queued by connect_once before publishing Open/Failed.
            // A reload can clear subscriptions, so this fence must be bounded.
            let through = self.core.events().last_seq();
            let mut delivered = self.authentication.delivered.subscribe();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while *delivered.borrow_and_update() < through {
                    if delivered.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
            result
        })
    }
}

/// Starts before authentication. Unclaimed input before shell readiness is
/// discarded; the TermTap retains the most recent size independently.
pub async fn pump_input(
    entry: Arc<SshEntry>,
    mut input: mpsc::UnboundedReceiver<TermInput>,
    relay: Arc<PromptRelay>,
    mut ready: watch::Receiver<Option<Arc<ResilientShell>>>,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancelled(&entry) => break,
            changed = ready.changed() => {
                if changed.is_err() { break; }
                let shell = ready.borrow_and_update().clone();
                if let (Some(shell), Some((cols, rows))) = (shell, entry.term.last_size()) {
                    let _ = shell.resize(u32::from(cols), u32::from(rows)).await;
                }
            }
            input = input.recv() => {
                let Some(input) = input else { break };
                let shell = ready.borrow().clone();
                match input {
                    TermInput::Bytes(bytes) => {
                        let bytes = Zeroizing::new(bytes);
                        if !relay.feed(&bytes) {
                            if let Some(shell) = shell {
                                // ResilientShell counts WriteOutcome::Dropped;
                                // lifecycle failures arrive through its events.
                                let _ = shell.write(&bytes).await;
                            }
                        }
                    }
                    TermInput::Resize { cols, rows } => {
                        if let Some(shell) = shell {
                            let _ = shell.resize(u32::from(cols), u32::from(rows)).await;
                        }
                    }
                }
            }
        }
    }
}

pub async fn open(
    core: &Arc<Core>,
    server_id: &str,
    opts: Option<SshOpenOpts>,
) -> Result<u64, CoreError> {
    let opts = opts.unwrap_or(SshOpenOpts {
        tmux: None,
        target: None,
    });
    let resolved = resolve_server(core, server_id, opts.target.as_ref())
        .await
        .map_err(CoreError::internal)?;
    let (term, input) = TermTap::new();
    let entry = core.state::<SshTable>().insert(server_id, term);
    emit_state(core, &entry, SshState::Connecting, None);
    let id = entry.id;
    let weak = Arc::downgrade(&entry);
    core.add_shutdown_hook(Box::new(move || {
        if let Some(entry) = weak.upgrade() {
            cancel_entry(&entry);
        }
    }));
    let c = core.clone();
    core.spawn(async move { run_ssh(&c, entry, input, resolved, opts.tmux.unwrap_or(true)).await });
    Ok(id)
}

pub async fn run_ssh(
    core: &Arc<Core>,
    entry: Arc<SshEntry>,
    input: mpsc::UnboundedReceiver<TermInput>,
    resolved: ResolvedTarget,
    tmux: bool,
) {
    let relay = Arc::new(PromptRelay::default());
    let prompter = Arc::new(CorePrompter::new(
        core.clone(),
        Some(entry.id),
        entry.server_id.clone(),
        resolved.target.user.clone(),
        Some(entry.clone()),
        relay.clone(),
    ));
    let (ready, receiver) = watch::channel(None);
    let pump = core.spawn(pump_input(entry.clone(), input, relay.clone(), receiver));
    let (delivered, _) = watch::channel(core.events().last_seq());
    let authentication = Arc::new(AuthenticationState {
        core: Arc::downgrade(core),
        entry: Arc::downgrade(&entry),
        asked: AtomicBool::new(false),
        delivered,
    });
    let subscription = core.events().subscribe(authentication.clone());
    let connector = Arc::new(CoreConnector {
        core: core.clone(),
        resolved,
        prompter,
        authentication,
    });
    let (cols, rows) = entry.term.last_size().unwrap_or((80, 24));
    let pty = PtyOpts {
        cols: u32::from(cols),
        rows: u32::from(rows),
        tmux: tmux.then(|| tmux_id(&entry.server_id)),
        ..Default::default()
    };
    let policy = lock(&core.state::<SshTuning>().0).clone();
    let opened = tokio::select! {
        biased;
        _ = cancelled(&entry) => None,
        result = ResilientShell::open(connector, pty, policy) => Some(result),
    };
    let mut active = None;
    let (state, message) = match opened {
        None => (SshState::Closed, None),
        Some(Err(error)) => (SshState::Failed, Some(error_kind(&error).into())),
        Some(Ok(mut shell)) => {
            // Keep draining independently of writes: the resilient shell has a
            // bounded output queue, so waiting for a write must not block output.
            let mut output = std::mem::replace(shell.output(), mpsc::channel(1).1);
            let mut events = shell.events();
            let shell = Arc::new(shell);
            ready.send_replace(Some(shell.clone()));
            emit_state(core, &entry, SshState::Open, None);
            active = Some(shell);
            let mut output_open = true;
            loop {
                tokio::select! {
                    biased;
                    _ = cancelled(&entry) => break (SshState::Closed, None),
                    bytes = output.recv(), if output_open => match bytes {
                        Some(bytes) => entry.term.push_output(&bytes),
                        None => output_open = false,
                    },
                    event = events.recv() => match event {
                        Ok(TerminalEvent::Connected { .. }) => {},
                        Ok(TerminalEvent::Reconnecting { attempt, .. }) => {
                            emit_state(core, &entry, SshState::Connecting, Some(format!("reconnecting {attempt}")));
                        }
                        Ok(TerminalEvent::Reconnected { .. }) => emit_state(core, &entry, SshState::Open, None),
                        Ok(TerminalEvent::TmuxUnavailable) => entry.term.push_output(TMUX_UNAVAILABLE),
                        Ok(TerminalEvent::Closed { status }) => break (SshState::Closed, status.map(|status| status.to_string())),
                        Ok(TerminalEvent::GaveUp(error)) => break (SshState::Failed, Some(error_kind(&error).into())),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break (SshState::Closed, None),
                    }
                }
            }
        }
    };
    ready.send_replace(None);
    pump.abort();
    let _ = pump.await;
    relay.cancel();
    reject_prompts(core, entry.id);
    if let Some(shell) = active {
        let shell = Arc::try_unwrap(shell)
            .ok()
            .expect("input pump is joined and the ready channel no longer owns the shell");
        shell.close().await;
    }
    core.events().unsubscribe(subscription);
    entry.term.close();
    core.state::<SshTable>().remove(entry.id);
    emit_state(core, &entry, state, message);
}

pub fn close(core: &Arc<Core>, id: u64) {
    if let Some(entry) = core.state::<SshTable>().get(id) {
        cancel_entry(&entry);
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_open(server_id: String, opts_json: Option<String>) -> Result<u64, CoreError> {
    let c = core()?;
    let opts = opts_json.as_deref().map(serde_json::from_str).transpose()?;
    c.clone()
        .run(async move { open(&c, &server_id, opts).await })
        .await
}

#[uniffi::export]
pub fn ssh_close(id: u64) {
    if let Ok(c) = core() {
        close(&c, id);
    }
}
