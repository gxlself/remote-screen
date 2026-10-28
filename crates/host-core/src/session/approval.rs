//! 本机确认与超时。见 docs/dev/06-host.md host-core。
//!
//! All local confirmation prompts share one registry. While at least one
//! prompt is visible, [`LocalPrompts::remote_input_allowed`] is false so the
//! input router can discard remote input (09 SEC-13).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_core::pb;
use rsp_core::trust::Permission as TrustPermission;
use serde_json::json;
use tokio::sync::{oneshot, watch};
use tracing::{debug, info, warn};

use crate::auth::{ApprovalReason, PathClass};
use crate::ipc;
use crate::pairing::{PairApproval, PairApprovalRequest, PairApprover};

/// The time the person at the host has to answer a confirmation prompt.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Wire value for `event.approvalRequested.timeout_s`.
pub const APPROVAL_TIMEOUT_S: u32 = 30;

/// Which kind of local confirmation is being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Connection,
    Pairing,
    Takeover,
}

impl PromptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Pairing => "pairing",
            Self::Takeover => "takeover",
        }
    }
}

/// Details presented to the person at the host.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptRequest {
    pub kind: PromptKind,
    pub device_name: String,
    pub device_id: String,
    pub platform: pb::Platform,
    pub whois_login: Option<String>,
    pub path: Option<String>,
    pub reasons: Vec<String>,
    pub device_fp: Option<[u8; 32]>,
    /// Omitted for legacy callers, pairing, and takeover prompts.
    pub intent: Option<pb::Intent>,
}

/// The person's answer to a confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptAnswer {
    pub allow: bool,
    pub permission: Option<ipc::Permission>,
}

/// How a prompt finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    Answered(PromptAnswer),
    TimedOut,
    Cancelled,
    Unavailable,
}

impl PromptOutcome {
    /// Only an explicit allow is successful.
    pub fn allowed(&self) -> bool {
        matches!(
            self,
            Self::Answered(PromptAnswer {
                allow: true,
                permission: _
            })
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Answered(PromptAnswer { allow: true, .. }) => "allowed",
            Self::Answered(PromptAnswer { allow: false, .. }) => "denied",
            Self::TimedOut => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "cancelled",
        }
    }
}

/// Event sink used by [`LocalPrompts`]. A `false` return means no host UI is
/// listening, so a new prompt must not count as visible.
pub trait PromptEvents: Send + Sync {
    fn emit(&self, event: ipc::Event) -> bool;
}

impl PromptEvents for ipc::Notifier {
    fn emit(&self, event: ipc::Event) -> bool {
        self.send(event).is_ok()
    }
}

struct PendingPrompt {
    kind: PromptKind,
    reply: Option<oneshot::Sender<PromptOutcome>>,
    payload: serde_json::Value,
    seq: u64,
}

struct Inner {
    events: Arc<dyn PromptEvents>,
    pending: Mutex<HashMap<String, PendingPrompt>>,
    next_seq: AtomicU64,
    gate: watch::Sender<bool>,
}

/// Shared registry for all local confirmation prompts.
#[derive(Clone)]
pub struct LocalPrompts {
    inner: Arc<Inner>,
}

/// One open confirmation prompt. Dropping it closes it as cancelled before
/// its deadline, or timed out once the deadline has elapsed.
pub struct OpenPrompt {
    pub request_id: String,
    reply: oneshot::Receiver<PromptOutcome>,
    deadline: tokio::time::Instant,
    finished: bool,
    immediate: Option<PromptOutcome>,
    prompts: LocalPrompts,
}

impl LocalPrompts {
    pub fn new(events: Arc<dyn PromptEvents>) -> Self {
        let (gate, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                events,
                pending: Mutex::new(HashMap::new()),
                next_seq: AtomicU64::new(0),
                gate,
            }),
        }
    }

    /// Register a prompt and notify the host UI. The 30 second deadline starts
    /// here, before the event is emitted.
    pub fn open(&self, request: PromptRequest) -> OpenPrompt {
        self.open_with_id(request, None)
    }

    fn open_with_id(
        &self,
        request: PromptRequest,
        preferred_request_id: Option<String>,
    ) -> OpenPrompt {
        self.open_with_id_until(
            request,
            preferred_request_id,
            tokio::time::Instant::now() + APPROVAL_TIMEOUT,
        )
    }

    fn open_with_id_until(
        &self,
        request: PromptRequest,
        preferred_request_id: Option<String>,
        deadline: tokio::time::Instant,
    ) -> OpenPrompt {
        let request_id = match preferred_request_id.as_deref() {
            Some(id) if !id.is_empty() => Some(id.to_owned()),
            _ => match random_request_id() {
                Ok(id) => Some(id),
                Err(error) => {
                    warn!(%error, "could not generate approval request id");
                    None
                }
            },
        };
        let Some(mut request_id) = request_id else {
            return OpenPrompt::immediate(self.clone(), PromptOutcome::Unavailable);
        };

        let (sender, receiver) = oneshot::channel();
        let kind = request.kind;
        let mut pending = match self.inner.pending.lock() {
            Ok(pending) => pending,
            Err(_) => {
                warn!("approval registry lock was poisoned");
                return OpenPrompt::immediate(self.clone(), PromptOutcome::Unavailable);
            }
        };

        // A caller-provided id is reused only when free. Random ids are also
        // checked for collisions so an insert never replaces another prompt.
        if preferred_request_id.is_none() || pending.contains_key(&request_id) {
            match unique_request_id(&pending) {
                Ok(id) => request_id = id,
                Err(error) => {
                    warn!(%error, "could not generate approval request id");
                    return OpenPrompt::immediate(self.clone(), PromptOutcome::Unavailable);
                }
            }
        }
        let mut payload = json!({
            "request_id": request_id,
            "kind": kind.as_str(),
            "device_name": request.device_name,
            "device_id": request.device_id,
            "platform": platform_name(request.platform),
            "whois_login": request.whois_login,
            "path": request.path,
            "reasons": request.reasons,
            "timeout_s": APPROVAL_TIMEOUT_S,
            "permission_choice": kind == PromptKind::Pairing,
            "device_fp": request.device_fp.as_ref().map(fp_b64url),
        });
        if let Some(intent) = request.intent {
            payload["intent"] = connection_intent_name(intent).into();
        }
        pending.insert(
            request_id.clone(),
            PendingPrompt {
                kind,
                reply: Some(sender),
                payload: payload.clone(),
                seq: self.inner.next_seq.fetch_add(1, Ordering::Relaxed),
            },
        );
        drop(pending);
        // Register before broadcasting: a fast UI can otherwise respond in the
        // interval between the event and insertion in `pending`.
        if !self
            .inner
            .events
            .emit(ipc::Event::ApprovalRequested(payload))
        {
            if let Ok(mut pending) = self.inner.pending.lock() {
                pending.remove(&request_id);
                sync_gate(&self.inner.gate, &pending);
            }
            debug!(
                %request_id,
                kind = kind.as_str(),
                "approval prompt unavailable: no UI listener"
            );
            return OpenPrompt::immediate(self.clone(), PromptOutcome::Unavailable);
        }

        if let Ok(pending) = self.inner.pending.lock() {
            sync_gate(&self.inner.gate, &pending);
        } else {
            warn!(%request_id, "approval registry lock was poisoned");
        }
        debug!(%request_id, kind = kind.as_str(), "approval prompt opened");
        OpenPrompt {
            request_id,
            reply: receiver,
            deadline,
            finished: false,
            immediate: None,
            prompts: self.clone(),
        }
    }

    /// Open a prompt and wait for its result.
    pub async fn ask(&self, request: PromptRequest) -> PromptOutcome {
        self.open(request).wait().await
    }

    /// Wait using the connection handshake's existing absolute deadline.
    pub(crate) async fn ask_until(
        &self,
        request: PromptRequest,
        deadline: tokio::time::Instant,
    ) -> PromptOutcome {
        self.open_with_id_until(request, None, deadline)
            .wait()
            .await
    }

    /// Deliver an answer from `approval.respond`. Unknown, already answered,
    /// timed out, cancelled, or unavailable requests return false.
    pub fn respond(
        &self,
        request_id: &str,
        allow: bool,
        permission: Option<ipc::Permission>,
    ) -> bool {
        self.finish(
            request_id,
            PromptOutcome::Answered(PromptAnswer { allow, permission }),
        )
    }

    /// Finish a prompt as cancelled. Returns false when it was already closed.
    pub fn cancel(&self, request_id: &str) -> bool {
        self.finish(request_id, PromptOutcome::Cancelled)
    }

    /// Number of currently open prompts.
    pub fn open_count(&self) -> usize {
        match self.inner.pending.lock() {
            Ok(pending) => pending.len(),
            Err(_) => 0,
        }
    }

    /// Current requested payloads in opening order. `timeout_s` retains the
    /// original 30 second duration, rather than the remaining time.
    pub fn pending_requests(&self) -> Vec<serde_json::Value> {
        let pending = match self.inner.pending.lock() {
            Ok(pending) => pending,
            Err(_) => {
                warn!("approval registry lock was poisoned");
                return Vec::new();
            }
        };
        let mut entries: Vec<_> = pending.values().collect();
        entries.sort_by_key(|entry| entry.seq);
        entries
            .into_iter()
            .map(|entry| entry.payload.clone())
            .collect()
    }

    /// Remote input is permitted only when no local prompt is visible.
    pub fn remote_input_allowed(&self) -> bool {
        match self.inner.pending.lock() {
            Ok(pending) => pending.is_empty(),
            Err(_) => false,
        }
    }

    /// Watch the prompt gate. `true` means at least one prompt is visible.
    pub fn subscribe_gate(&self) -> watch::Receiver<bool> {
        self.inner.gate.subscribe()
    }

    fn finish(&self, request_id: &str, outcome: PromptOutcome) -> bool {
        let (entry, kind) = {
            let mut pending = match self.inner.pending.lock() {
                Ok(pending) => pending,
                Err(_) => {
                    warn!(%request_id, "approval registry lock was poisoned");
                    return false;
                }
            };
            let entry = pending.remove(request_id);
            let kind = entry.as_ref().map(|entry| entry.kind);
            sync_gate(&self.inner.gate, &pending);
            (entry, kind)
        };
        let (Some(mut entry), Some(kind)) = (entry, kind) else {
            return false;
        };
        if let Some(sender) = entry.reply.take() {
            let _ = sender.send(outcome);
        }
        let payload = json!({
            "request_id": request_id,
            "kind": kind.as_str(),
            "outcome": outcome.as_str(),
        });
        if !self.inner.events.emit(ipc::Event::ApprovalClosed(payload)) {
            debug!(
                %request_id,
                kind = kind.as_str(),
                outcome = outcome.as_str(),
                "approval close event had no UI listener"
            );
        }
        info!(
            %request_id,
            kind = kind.as_str(),
            outcome = outcome.as_str(),
            "approval prompt closed"
        );
        true
    }
}

impl OpenPrompt {
    fn immediate(prompts: LocalPrompts, outcome: PromptOutcome) -> Self {
        let (_sender, receiver) = oneshot::channel();
        Self {
            request_id: String::new(),
            reply: receiver,
            deadline: tokio::time::Instant::now(),
            finished: true,
            immediate: Some(outcome),
            prompts,
        }
    }

    /// Wait for an answer. The deadline is measured from [`LocalPrompts::open`].
    pub async fn wait(mut self) -> PromptOutcome {
        if let Some(outcome) = self.immediate.take() {
            self.finished = true;
            return outcome;
        }
        let outcome = match tokio::time::timeout_at(self.deadline, &mut self.reply).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                self.prompts
                    .finish(&self.request_id, PromptOutcome::Cancelled);
                PromptOutcome::Cancelled
            }
            Err(_) => {
                if self
                    .prompts
                    .finish(&self.request_id, PromptOutcome::TimedOut)
                {
                    PromptOutcome::TimedOut
                } else {
                    // Another closer won the race at the deadline. Its result
                    // is authoritative and is delivered through the channel.
                    match (&mut self.reply).await {
                        Ok(outcome) => outcome,
                        Err(_) => PromptOutcome::Cancelled,
                    }
                }
            }
        };
        self.finished = true;
        outcome
    }
}

impl Drop for OpenPrompt {
    fn drop(&mut self) {
        if !self.finished {
            // The handshake's fallback can win polling at this same deadline.
            // Dropping its future must still close the UI as a timeout.
            let outcome = if tokio::time::Instant::now() >= self.deadline {
                PromptOutcome::TimedOut
            } else {
                PromptOutcome::Cancelled
            };
            self.prompts.finish(&self.request_id, outcome);
            self.finished = true;
        }
    }
}

#[async_trait]
impl PairApprover for LocalPrompts {
    async fn request(&self, request: PairApprovalRequest) -> PairApproval {
        let device_id = id_from_fp(&request.device_fp);
        let prompt = PromptRequest {
            kind: PromptKind::Pairing,
            device_name: request.device_name,
            device_id,
            platform: request.platform,
            whois_login: request.whois_login,
            path: None,
            reasons: Vec::new(),
            device_fp: Some(request.device_fp),
            intent: None,
        };
        let preferred = (!request.request_id.is_empty()).then_some(request.request_id);
        match self.open_with_id(prompt, preferred).wait().await {
            PromptOutcome::Answered(PromptAnswer {
                allow: true,
                permission,
            }) => PairApproval {
                allow: true,
                permission: trust_permission(permission.unwrap_or(ipc::Permission::Control)),
            },
            _ => PairApproval {
                allow: false,
                permission: TrustPermission::ViewOnly,
            },
        }
    }
}

/// Stable wire name for an authentication approval reason.
pub fn reason_name(reason: ApprovalReason) -> &'static str {
    match reason {
        ApprovalReason::AttendedMode => "attended_mode",
        ApprovalReason::LocalActivity => "local_activity",
        ApprovalReason::IntranetConfirmation => "intranet_confirmation",
        ApprovalReason::NodeMismatch => "node_mismatch",
        ApprovalReason::DeviceRequiresConfirmation => "device_requires_confirmation",
    }
}

/// Stable wire name for a selected route. Unknown routes are not displayed.
pub fn path_name(path: PathClass) -> Option<&'static str> {
    match path {
        PathClass::Tailnet => Some("tailnet"),
        PathClass::Intranet => Some("intranet"),
        PathClass::Lan => Some("lan"),
        PathClass::Unknown => None,
    }
}

pub(crate) fn connection_intent_name(intent: pb::Intent) -> &'static str {
    match intent {
        pb::Intent::Files => "files",
        pb::Intent::View => "view",
        // Unspecified connections retain the legacy CONTROL behavior.
        _ => "control",
    }
}

fn platform_name(platform: pb::Platform) -> &'static str {
    match platform {
        pb::Platform::Ios => "ios",
        pb::Platform::Ipados => "ipados",
        pb::Platform::Android => "android",
        pb::Platform::Macos => "macos",
        pb::Platform::Windows => "windows",
        pb::Platform::Linux => "linux",
        pb::Platform::Unspecified => "unspecified",
    }
}

fn trust_permission(permission: ipc::Permission) -> TrustPermission {
    match permission {
        ipc::Permission::ViewOnly => TrustPermission::ViewOnly,
        ipc::Permission::Control => TrustPermission::Control,
        ipc::Permission::Unattended => TrustPermission::Unattended,
    }
}

fn random_request_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)?;
    Ok(format!("a-{}", data_encoding::HEXLOWER.encode(&bytes)))
}

fn unique_request_id(pending: &HashMap<String, PendingPrompt>) -> Result<String, getrandom::Error> {
    loop {
        let id = random_request_id()?;
        if !pending.contains_key(&id) {
            return Ok(id);
        }
    }
}

fn sync_gate(gate: &watch::Sender<bool>, pending: &HashMap<String, PendingPrompt>) {
    let visible = !pending.is_empty();
    gate.send_if_modified(|current| {
        if *current == visible {
            false
        } else {
            *current = visible;
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn dropped_prompt_uses_shared_deadline_without_polling_wait() {
        for expired in [false, true] {
            let (events, mut received) = tokio::sync::broadcast::channel(8);
            let prompts = LocalPrompts::new(Arc::new(ipc::Notifier::from_sender(events)));
            let request = PromptRequest {
                kind: PromptKind::Connection,
                device_name: "test phone".into(),
                device_id: "test-device".into(),
                platform: pb::Platform::Ios,
                whois_login: None,
                path: None,
                reasons: Vec::new(),
                device_fp: None,
                intent: None,
            };
            let open = prompts.open_with_id_until(
                request,
                None,
                tokio::time::Instant::now() + APPROVAL_TIMEOUT,
            );
            assert!(matches!(
                received.try_recv().unwrap(),
                ipc::Event::ApprovalRequested(_)
            ));
            if expired {
                tokio::time::advance(APPROVAL_TIMEOUT).await;
            }
            drop(open);
            let ipc::Event::ApprovalClosed(payload) = received.try_recv().unwrap() else {
                panic!("expected approvalClosed");
            };
            assert_eq!(
                payload["outcome"],
                if expired { "timeout" } else { "cancelled" }
            );
            assert!(received.try_recv().is_err());
            assert!(prompts.remote_input_allowed());
        }
    }
}
