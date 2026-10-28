//! Desktop notifications and the long-running bridge to local host approvals.

use std::collections::HashMap;
use std::future::poll_fn;
use std::path::PathBuf;
use std::time::Duration;

use host_core::ipc::ApprovalRespondParams;
use zbus::{
    export::futures_core::Stream, proxy::SignalStream, Connection, OwnedVariant, Proxy, Str,
};

use crate::ctl::{
    approval_body, approval_from_value, approval_title, notification_actions, pairing_result_text,
    ApprovalBook, ApprovalPrompt, CtlError, IpcClient, IpcEvent,
};

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT: &str = "/org/freedesktop/Notifications";

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("no notification server: {0}")]
    Unavailable(String),
    #[error("D-Bus: {0}")]
    DBus(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub critical: bool,
    pub timeout_ms: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyEvent {
    Action { id: u32, key: String },
    Closed { id: u32, reason: u32 },
}

pub struct Notifier {
    proxy: Proxy<'static>,
    action_signals: SignalStream<'static>,
    closed_signals: SignalStream<'static>,
    actions: bool,
}

impl Notifier {
    /// Install a sender-filtered subscription before querying capabilities.
    pub async fn new(conn: Connection) -> Result<Self, NotifyError> {
        let proxy = Proxy::new_owned(conn, SERVICE, OBJECT, SERVICE)
            .await
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        let action_signals = proxy
            .receive_signal("ActionInvoked")
            .await
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        let closed_signals = proxy
            .receive_signal("NotificationClosed")
            .await
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        let capabilities: Vec<String> = proxy
            .call("GetCapabilities", &())
            .await
            .map_err(|error| NotifyError::Unavailable(error.to_string()))?;
        Ok(Self {
            proxy,
            action_signals,
            closed_signals,
            actions: capabilities
                .iter()
                .any(|capability| capability == "actions"),
        })
    }

    pub fn supports_actions(&self) -> bool {
        self.actions
    }

    pub async fn show(&self, n: &Notification) -> Result<u32, NotifyError> {
        let actions: Vec<&str> = n
            .actions
            .iter()
            .flat_map(|(key, label)| [key.as_str(), label.as_str()])
            .collect();
        let mut hints: HashMap<&str, OwnedVariant> = HashMap::from([
            ("urgency", (if n.critical { 2_u8 } else { 1_u8 }).into()),
            ("desktop-entry", Str::from("remote-host").into()),
        ]);
        if !n.actions.is_empty() {
            hints.insert("resident", true.into());
        }
        self.proxy
            .call(
                "Notify",
                &(
                    "Remote Host",
                    0_u32,
                    "remote-host",
                    n.summary.as_str(),
                    n.body.as_str(),
                    actions,
                    hints,
                    n.timeout_ms,
                ),
            )
            .await
            .map_err(|error| NotifyError::DBus(error.to_string()))
    }

    pub async fn close(&self, id: u32) -> Result<(), NotifyError> {
        self.proxy
            .call("CloseNotification", &(id,))
            .await
            .map_err(|error| NotifyError::DBus(error.to_string()))
    }

    pub async fn next_event(&mut self) -> Option<NotifyEvent> {
        tokio::select! {
                message = poll_fn(|cx| std::pin::Pin::new(&mut self.action_signals).poll_next(cx)) => {
                    let message = message?;
                let (id, key) = message.body().deserialize::<(u32, String)>().ok()?;
                Some(NotifyEvent::Action { id, key })
            }
                message = poll_fn(|cx| std::pin::Pin::new(&mut self.closed_signals).poll_next(cx)) => {
                    let message = message?;
                let (id, reason) = message.body().deserialize::<(u32, u32)>().ok()?;
                Some(NotifyEvent::Closed { id, reason })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotifyLoopOptions {
    pub socket: PathBuf,
    pub reconnect: Duration,
}

impl Default for NotifyLoopOptions {
    fn default() -> Self {
        Self {
            socket: crate::ctl::default_socket(),
            reconnect: Duration::from_secs(3),
        }
    }
}

fn notification_for_prompt(prompt: &ApprovalPrompt, with_actions: bool) -> Notification {
    let body = approval_body(prompt);
    Notification {
        summary: approval_title(prompt),
        body: if with_actions {
            // Notification body supports markup. Escape peer-supplied fields.
            escape_markup(&body)
        } else {
            format!(
                "{}；请在终端运行 remote-host-ctl approve 处理",
                escape_markup(&body)
            )
        },
        actions: if with_actions {
            notification_actions(prompt)
                .into_iter()
                .map(|(key, label)| (key.to_owned(), label.to_owned()))
                .collect()
        } else {
            Vec::new()
        },
        critical: with_actions,
        timeout_ms: prompt.timeout_s.saturating_mul(1000).min(i32::MAX as u64) as i32,
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn show_prompt(
    notifier: &Notifier,
    book: &mut ApprovalBook,
    prompt: ApprovalPrompt,
) -> Result<(), NotifyError> {
    // Pending snapshots and live events can describe the same request.
    if book.contains_request(&prompt.request_id) {
        return Ok(());
    }
    let id = notifier
        .show(&notification_for_prompt(
            &prompt,
            notifier.supports_actions(),
        ))
        .await?;
    // Track fallback notifications too so agent closure/restart removes them.
    book.add(prompt, id);
    Ok(())
}

async fn handle_agent_event(
    event: IpcEvent,
    notifier: &Notifier,
    book: &mut ApprovalBook,
) -> Result<(), NotifyError> {
    match event.method.as_str() {
        "event.approvalRequested" => {
            if let Some(prompt) = approval_from_value(&event.params) {
                show_prompt(notifier, book, prompt).await?;
            }
        }
        "event.approvalClosed" => {
            if let Some(id) = event
                .params
                .get("request_id")
                .and_then(|v| v.as_str())
                .and_then(|request_id| book.on_request_closed(request_id))
            {
                notifier.close(id).await?;
            }
        }
        "event.pairingResult" => {
            notifier
                .show(&Notification {
                    summary: "Remote Host 配对".to_owned(),
                    body: pairing_result_text(&event.params),
                    actions: Vec::new(),
                    critical: false,
                    timeout_ms: 5000,
                })
                .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn respond(client: &mut IpcClient, params: ApprovalRespondParams) -> Result<(), CtlError> {
    let result = client.approval_respond(&params).await;
    // RPC messages can contain peer data; record only the outcome.
    tracing::debug!(
        request_id = %params.request_id,
        allow = params.allow,
        result = if result.is_ok() { "ok" } else { "failed" },
        "approval response"
    );
    match result {
        Ok(()) | Err(CtlError::Rpc { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn connect_agent(
    socket: &std::path::Path,
) -> Result<(IpcClient, Vec<ApprovalPrompt>), CtlError> {
    let mut client = IpcClient::connect(socket).await?;
    let pending = match client.approval_pending().await {
        Ok(pending) => pending,
        // Older agents may not provide a pending snapshot.
        Err(CtlError::Rpc { .. }) => {
            tracing::debug!(result = "unavailable", "approval.pending");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    Ok((client, pending))
}

async fn connected_loop(
    client: &mut IpcClient,
    notifier: &mut Notifier,
    book: &mut ApprovalBook,
) -> Result<(), NotifyError> {
    loop {
        tokio::select! {
            event = client.next_event() => {
                match event {
                    Ok(event) => handle_agent_event(event, notifier, book).await?,
                    Err(_) => return Ok(()),
                }
            }
            event = notifier.next_event() => {
                let event = event.ok_or_else(|| NotifyError::DBus(
                    "notification connection closed".to_owned()
                ))?;
                let params = match event {
                    NotifyEvent::Action { id, key } => {
                        if notifier.supports_actions() {
                            let params = book.on_action(id, &key);
                            if params.is_some() {
                                // A resident notification may survive its action.
                                notifier.close(id).await?;
                            }
                            params
                        } else {
                            None
                        }
                    }
                    NotifyEvent::Closed { id, reason } => {
                        book.on_closed(id, reason)
                    }
                };
                if let Some(params) = params {
                    if respond(client, params).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn connection_state(previous: &mut Option<bool>, connected: bool) {
    if *previous != Some(connected) {
        if connected {
            tracing::info!(result = "connected", "host agent connection");
        } else {
            tracing::warn!(result = "unavailable", "host agent connection");
        }
        *previous = Some(connected);
    }
}

/// Keep an IPC listener registered while desktop approval notifications are active.
pub async fn run_notify(
    opts: NotifyLoopOptions,
    mut notifier: Notifier,
) -> Result<(), NotifyError> {
    let reconnect = if opts.reconnect.is_zero() {
        Duration::from_secs(3)
    } else {
        opts.reconnect
    };
    let mut state = None;
    loop {
        // Watch D-Bus even while the agent is absent, so systemd can restart us
        // promptly if the notification connection closes.
        let connected = tokio::select! {
            result = connect_agent(&opts.socket) => result,
            event = notifier.next_event() => {
                if event.is_none() {
                    return Err(NotifyError::DBus("notification connection closed".to_owned()));
                }
                continue;
            }
        };
        if let Ok((mut client, pending)) = connected {
            connection_state(&mut state, true);
            let mut book = ApprovalBook::default();
            for prompt in pending {
                show_prompt(&notifier, &mut book, prompt).await?;
            }
            connected_loop(&mut client, &mut notifier, &mut book).await?;
            for id in book.drain() {
                notifier.close(id).await?;
            }
        }
        connection_state(&mut state, false);
        let retry = tokio::time::sleep(reconnect);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                _ = &mut retry => break,
                event = notifier.next_event() => {
                    if event.is_none() {
                        return Err(NotifyError::DBus("notification connection closed".to_owned()));
                    }
                }
            }
        }
    }
}
