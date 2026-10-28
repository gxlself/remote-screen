//! Authorization after TLS, before starting media or accepting remote input.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rsp_core::{control::encode, errors::ErrorCode, features, pb};
use rsp_transport::{identity::Fingerprint, tls::peer_fingerprint, CloseCode, Conn, ControlStream};
use tokio::sync::oneshot;
use tokio::time::{timeout_at, Instant};
use tracing::warn;

use super::{
    approval::APPROVAL_TIMEOUT,
    handlers,
    manager::{Admission, ConnKey, ResumeOutcome, SessionManager},
};
use crate::auth::{self, AuthDecision, PathClass, Role, WhoIsOutcome};
use crate::pairing::{PairingConfirmOutcome, PairingManager, PairingReply, PairingSession};
use crate::platform::Platform;
use crate::store::Store;
use crate::whois::WhoIsSource;
use crate::{RuntimeOptions, RuntimeResult};

#[derive(Debug, Clone)]
pub struct ConnectionApprovalRequest {
    pub request_id: String,
    pub device_id: String,
    pub device_name: String,
    pub device_fp: Fingerprint,
    pub role: Role,
    pub reasons: Vec<auth::ApprovalReason>,
    pub peer: pb::PeerInfo,
    pub platform: pb::Platform,
    pub path: auth::PathClass,
    /// Connection intent for approval UI metadata; `None` preserves legacy payloads.
    pub intent: Option<pb::Intent>,
}

/// A refusal and an expired approval deadline have distinct wire/audit reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionApprovalOutcome {
    Allowed,
    Rejected,
    TimedOut,
}

#[async_trait]
pub trait ConnectionApprover: Send + Sync {
    async fn request(&self, request: ConnectionApprovalRequest) -> bool;

    /// Use the handshake's deadline while preserving existing custom approvers.
    async fn request_until(
        &self,
        request: ConnectionApprovalRequest,
        deadline: Instant,
    ) -> ConnectionApprovalOutcome {
        match timeout_at(deadline, self.request(request)).await {
            Ok(allowed) if Instant::now() < deadline => {
                if allowed {
                    ConnectionApprovalOutcome::Allowed
                } else {
                    ConnectionApprovalOutcome::Rejected
                }
            }
            _ => ConnectionApprovalOutcome::TimedOut,
        }
    }
}

/// The IPC backend routes approval.respond to respond(). The outer handshake
/// owns the deadline too, including for custom approvers.
pub struct IpcConnectionApprover {
    notifier: crate::ipc::Notifier,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

struct PendingApproval<'a> {
    requests: &'a Mutex<HashMap<String, oneshot::Sender<bool>>>,
    id: &'a str,
    notifier: &'a crate::ipc::Notifier,
    deadline: Instant,
    answered: bool,
}

impl Drop for PendingApproval<'_> {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(self.id);
        }
        // An outer timeout may drop us before our own timer is polled.
        // Explicit replies retain the legacy IPC event behavior.
        if !self.answered && Instant::now() >= self.deadline {
            let _ = self
                .notifier
                .send(crate::ipc::Event::ApprovalClosed(serde_json::json!({
                    "request_id": self.id, "kind": "connection", "outcome": "timeout",
                })));
        }
    }
}

impl IpcConnectionApprover {
    pub fn new(notifier: crate::ipc::Notifier) -> Self {
        Self {
            notifier,
            pending: Mutex::new(HashMap::new()),
        }
    }
    pub fn respond(&self, request_id: &str, allow: bool) -> bool {
        self.pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(request_id))
            .is_some_and(|sender| sender.send(allow).is_ok())
    }
}

#[async_trait]
impl ConnectionApprover for IpcConnectionApprover {
    async fn request(&self, request: ConnectionApprovalRequest) -> bool {
        self.request_until(request, Instant::now() + APPROVAL_TIMEOUT)
            .await
            == ConnectionApprovalOutcome::Allowed
    }

    async fn request_until(
        &self,
        request: ConnectionApprovalRequest,
        deadline: Instant,
    ) -> ConnectionApprovalOutcome {
        let request_id = request.request_id.clone();
        let peer = request.peer;
        let mut payload = serde_json::json!({
            "request_id": request_id, "kind": "connection", "device_id": request.device_id,
            "device_name": request.device_name, "device_fp": rsp_core::ids::fp_b64url(&request.device_fp),
            "role": match request.role { Role::Controller => "controller", Role::Viewer => "viewer" },
            "reasons": request.reasons.iter().map(|reason| format!("{reason:?}")).collect::<Vec<_>>(),
            "peer": { "user": peer.user, "node": peer.node, "device_id": peer.device_id,
                "path": peer.path, "derp_region": peer.derp_region }, "timeout_s": 30,
        });
        if let Some(intent) = request.intent {
            payload["intent"] = super::approval::connection_intent_name(intent).into();
        }
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id.clone(), sender);
        } else {
            return ConnectionApprovalOutcome::Rejected;
        }
        let mut pending = PendingApproval {
            requests: &self.pending,
            id: &request_id,
            notifier: &self.notifier,
            deadline,
            answered: false,
        };
        let notified = self
            .notifier
            .send(crate::ipc::Event::ApprovalRequested(payload))
            .is_ok();
        if !notified {
            pending.answered = true;
            return ConnectionApprovalOutcome::Rejected;
        }
        match timeout_at(deadline, receiver).await {
            Ok(answer) => {
                pending.answered = true;
                if matches!(answer, Ok(true)) {
                    ConnectionApprovalOutcome::Allowed
                } else {
                    ConnectionApprovalOutcome::Rejected
                }
            }
            Err(_) => ConnectionApprovalOutcome::TimedOut,
        }
    }
}

pub struct Accepted {
    pub role: Role,
    pub can_control: bool,
    pub peer: pb::PeerInfo,
    pub device_fp: Option<Fingerprint>,
    pub device_name: String,
    pub platform: pb::Platform,
    pub intent: pb::Intent,
    pub client_features: u64,
    pub resumed: Option<Admission>,
}

pub enum HandshakeOutcome {
    Accepted(Box<Accepted>),
    Probe,
    Pair(Box<PairingSession>),
    PairDenied,
    Denied,
}

pub struct HandshakeContext {
    pub identity: Arc<crate::identity::HostIdentity>,
    pub store: Arc<Store>,
    pub pairing: PairingManager,
    pub approver: Option<Arc<dyn ConnectionApprover>>,
    pub audit: Option<crate::audit::AuditQueue>,
    pub whois: Arc<dyn WhoIsSource>,
}

impl HandshakeContext {
    fn audit_denial(
        &self,
        now_us: u64,
        event: &str,
        peer: Option<&pb::PeerInfo>,
        detail: serde_json::Value,
    ) {
        let Some(audit) = &self.audit else {
            return;
        };
        let record = crate::audit::AuditEvent {
            ts: now_us,
            user: peer
                .filter(|peer| !peer.user.is_empty())
                .map(|peer| peer.user.clone()),
            device_id: peer
                .filter(|peer| !peer.device_id.is_empty())
                .map(|peer| peer.device_id.clone()),
            host_id: Some(self.identity.host_id()),
            server: None,
            event: event.into(),
            path: peer.and_then(|peer| match pb::PathKind::try_from(peer.path).ok()? {
                pb::PathKind::TailnetDirect => Some(crate::audit::AuditPath::TailnetDirect),
                pb::PathKind::TailnetDerp => Some(crate::audit::AuditPath::TailnetDerp),
                pb::PathKind::Intranet => Some(crate::audit::AuditPath::Intranet),
                pb::PathKind::Lan => Some(crate::audit::AuditPath::Lan),
                _ => None,
            }),
            duration_s: None,
            detail: Some(detail),
        };
        if let Err(error) = audit.push(record) {
            warn!(%error, "persist denial audit failed");
        }
    }
}

pub struct SessionContext {
    pub security: Arc<HandshakeContext>,
    pub local: SocketAddr,
    pub path: crate::listen::PathClass,
    pub tcp_tokens: Option<rsp_transport::tcp::TcpTokenRegistry>,
}

pub(crate) enum HandshakeWriter<'a> {
    Transport(&'a ControlStream),
}

struct PeerIdentity {
    certificates: Vec<rustls::pki_types::CertificateDer<'static>>,
    remote: SocketAddr,
    connection_id: String,
}

pub async fn authenticate<P: Platform>(
    conn: &Conn,
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
    options: &RuntimeOptions,
    hello: pb::ClientHello,
    send: &ControlStream,
    context: &SessionContext,
) -> RuntimeResult<HandshakeOutcome> {
    authenticate_peer(
        PeerIdentity {
            certificates: conn.peer_certificates(),
            remote: conn.remote_addr(),
            connection_id: conn.stable_id(),
        },
        mgr,
        key,
        options,
        hello,
        &mut HandshakeWriter::Transport(send),
        context,
    )
    .await
}

async fn authenticate_peer<P: Platform>(
    connection: PeerIdentity,
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
    options: &RuntimeOptions,
    hello: pb::ClientHello,
    send: &mut HandshakeWriter<'_>,
    context: &SessionContext,
) -> RuntimeResult<HandshakeOutcome> {
    let security = &context.security;
    let platform = mgr.platform();
    let path = match context.path {
        crate::listen::PathClass::Tailnet => PathClass::Tailnet,
        crate::listen::PathClass::Intranet => PathClass::Intranet,
        crate::listen::PathClass::Lan => PathClass::Lan,
    };
    let certs = connection.certificates;
    let fp = peer_fingerprint(&certs);
    let intent = pb::Intent::try_from(hello.intent).unwrap_or(pb::Intent::Unspecified);
    let stored_device: auth::DeviceStatus = match (intent, fp) {
        (pb::Intent::Probe | pb::Intent::Pair, _) | (_, None) => auth::DeviceStatus::Unknown,
        (_, Some(fp)) => security.store.authorize_device(&fp)?.into(),
    };
    let device =
        if options.dev_allow_unpaired && matches!(stored_device, auth::DeviceStatus::Unknown) {
            auth::DeviceStatus::paired(rsp_core::trust::Permission::Control)
        } else {
            stored_device
        };

    let whois_value = if path == PathClass::Tailnet {
        security.whois.whois(connection.remote).await.ok()
    } else {
        None
    };
    let whois = whois_value.as_ref().map_or(
        if path == PathClass::Tailnet {
            WhoIsOutcome::Unavailable
        } else {
            WhoIsOutcome::NotApplicable
        },
        |value| WhoIsOutcome::Ok {
            login: value.login.clone(),
            stable_id: value.stable_id.clone(),
            node_name: value.node_name.clone(),
        },
    );
    let peer_path = if path == PathClass::Tailnet {
        security.whois.peer_path(connection.remote.ip()).await.ok()
    } else {
        None
    };
    let peer = pb::PeerInfo {
        user: whois_value
            .as_ref()
            .map_or_else(String::new, |value| value.login.clone()),
        node: whois_value
            .as_ref()
            .map_or_else(String::new, |value| value.node_name.clone()),
        device_id: fp
            .as_ref()
            .map_or_else(String::new, rsp_core::ids::id_from_fp),
        path: match path {
            PathClass::Tailnet if peer_path.as_ref().is_some_and(|value| !value.direct) => {
                pb::PathKind::TailnetDerp
            }
            PathClass::Tailnet => pb::PathKind::TailnetDirect,
            PathClass::Intranet => pb::PathKind::Intranet,
            PathClass::Lan => pb::PathKind::Lan,
            PathClass::Unknown => pb::PathKind::PathUnspecified,
        } as i32,
        derp_region: peer_path.and_then(|value| value.relay).unwrap_or_default(),
    };
    let mut policy = if options.live_auth_policy {
        let config = mgr.services().config.get();
        let mut policy = crate::auth_config::auth_policy_from_config(&config);
        policy.forced_denial = options.auth_policy.forced_denial;
        policy
    } else {
        let mut policy = options.auth_policy.clone();
        policy.max_viewers = mgr.services().config.get().access.max_viewers;
        policy
    };
    if policy.allowed_users.is_empty() {
        if let Some(owner) = mgr.owner_login() {
            policy.allowed_users = vec![owner];
        } else if path == PathClass::Tailnet {
            mgr.warn_owner_unknown();
        }
    }
    let input = auth::AuthInput {
        intent,
        local_addr: context.local,
        peer_addr: connection.remote,
        path_class: path,
        device,
        whois,
        policy,
        trust_stale_24h: false,
        session: if options.session_snapshot != auth::SessionSnapshot::default() {
            options.session_snapshot
        } else {
            handlers::roster::auth_snapshot(mgr, &peer.device_id)
        },
        local_activity_within_60s: options.local_activity_within_60s
            || handlers::activity::active_within_60s(mgr),
    };
    let decision = auth::decide(&input);
    let mut control_input = input.clone();
    control_input.intent = pb::Intent::Control;
    control_input.session = auth::SessionSnapshot::default();
    let can_control = !options.view_only
        && matches!(
            auth::decide(&control_input),
            AuthDecision::Accept {
                role: Role::Controller,
                ..
            }
        );

    if let AuthDecision::Denied { code, log } = decision {
        security.audit_denial(
            mgr.now_us(),
            "auth.denied",
            Some(&peer),
            serde_json::json!({"code": code.code(), "reason": log}),
        );
        deny(send, code, log).await?;
        return Ok(HandshakeOutcome::Denied);
    }
    // Do not disclose host name, OS or display inventory to probes or pairing
    // callers; a denied normal connection receives no identifying ServerHello.
    let mut server = pb::ServerHello {
        protocol: features::PROTOCOL_VERSION,
        host_version: env!("CARGO_PKG_VERSION").into(),
        host_id: security.identity.host_id(),
        peer: Some(peer.clone()),
        ..Default::default()
    };
    if matches!(decision, AuthDecision::Accept { .. }) {
        let info = platform.info();
        server.host_name = info.name;
        server.os = info.os as i32;
        server.os_version = info.os_version;
        server.features = handlers::host_features()
            | if platform.audio_capture_supported() {
                features::AUDIO_OPUS
            } else {
                0
            }
            | handlers::vdisplay::advertised(mgr);
        server.displays = handlers::displays::hello_displays(mgr);
    }
    if matches!(decision, AuthDecision::Pair { .. }) {
        server.features = handlers::host_features();
    }
    send_frame(send, server).await?;
    match decision {
        AuthDecision::Probe => {
            send_frame(
                send,
                pb::Goodbye {
                    reason: pb::goodbye::Reason::UserClosed as i32,
                    message: String::new(),
                },
            )
            .await?;
            Ok(HandshakeOutcome::Probe)
        }
        AuthDecision::Pair { allowed, reason } => {
            if !allowed || fp.is_none() {
                warn!("pairing denied on path or without device certificate");
                security.audit_denial(
                    mgr.now_us(),
                    "pairing.denied",
                    Some(&peer),
                    serde_json::json!({"reason": "path or missing certificate"}),
                );
                send_frame(
                    send,
                    pb::PairResult {
                        status: if allowed {
                            pb::pair_result::Status::Denied
                        } else {
                            reason
                        } as i32,
                        ..Default::default()
                    },
                )
                .await?;
                return Ok(HandshakeOutcome::PairDenied);
            }
            let via = match context.path {
                crate::listen::PathClass::Tailnet => crate::store::PairingPath::Tailnet,
                crate::listen::PathClass::Intranet => crate::store::PairingPath::Intranet,
                crate::listen::PathClass::Lan => crate::store::PairingPath::Lan,
            };
            let Some(fp) = fp else {
                return Ok(HandshakeOutcome::PairDenied);
            };
            let cert = certs
                .first()
                .map_or_else(Vec::new, |cert| cert.as_ref().to_vec());
            Ok(HandshakeOutcome::Pair(Box::new(
                security
                    .pairing
                    .session_with_whois(fp, cert, whois_value, via),
            )))
        }
        AuthDecision::Accept {
            role,
            mut needs_approval,
            warnings,
        } => {
            for warning in warnings {
                warn!(?warning, "authentication warning");
            }
            if let Some(err) = handlers::power::console_check(mgr) {
                security.audit_denial(
                    mgr.now_us(),
                    "auth.denied",
                    Some(&peer),
                    serde_json::json!({"code": err.code}),
                );
                send_frame(
                    send,
                    pb::AuthResult {
                        status: pb::auth_result::Status::Denied as i32,
                        error: Some(err),
                        ..Default::default()
                    },
                )
                .await?;
                return Ok(HandshakeOutcome::Denied);
            }
            // Roster resume consumes the token and inherits its role. FILES must
            // always use fresh, independent admission, even with a valid roster
            // token. Its own tokens are never registered in the roster either.
            let can_resume = intent != pb::Intent::Files && fp.is_some();
            let resumed = if can_resume && !hello.resume_token.is_empty() {
                match handlers::roster::resume(mgr, key, &hello.resume_token, &peer.device_id) {
                    ResumeOutcome::Fresh => None,
                    ResumeOutcome::Resumed(admission) => Some(admission),
                    ResumeOutcome::Deny(code) => {
                        security.audit_denial(
                            mgr.now_us(),
                            "auth.denied",
                            Some(&peer),
                            serde_json::json!({"code": code.code()}),
                        );
                        deny(send, code, "resume denied").await?;
                        return Ok(HandshakeOutcome::Denied);
                    }
                }
            } else {
                None
            };
            let role = if options.view_only {
                Role::Viewer
            } else {
                role
            };
            let paired_at_us = fp
                .as_ref()
                .map(|fp| security.store.get(fp))
                .transpose()?
                .flatten()
                .map(|d| d.paired_at_us);
            if resumed.is_none()
                && handlers::approval::confirm_new_device(mgr, &peer.device_id, paired_at_us)
            {
                let reasons = needs_approval.get_or_insert_with(Vec::new);
                if !reasons.contains(&auth::ApprovalReason::DeviceRequiresConfirmation) {
                    reasons.push(auth::ApprovalReason::DeviceRequiresConfirmation);
                }
            }
            if let Some(reasons) = needs_approval.filter(|_| resumed.is_none()) {
                send_frame(
                    send,
                    pb::AuthResult {
                        status: pb::auth_result::Status::PendingApproval as i32,
                        role: role_to_pb(role),
                        approval_timeout_s: 30,
                        ..Default::default()
                    },
                )
                .await?;
                let request = ConnectionApprovalRequest {
                    request_id: format!("conn-{}-{}", mgr.now_us(), connection.connection_id),
                    device_id: peer.device_id.clone(),
                    device_name: hello.device_name.clone(),
                    device_fp: fp.unwrap_or([0; 32]),
                    role,
                    reasons,
                    peer: peer.clone(),
                    platform: pb::Platform::try_from(hello.platform).unwrap_or_default(),
                    path,
                    intent: Some(intent),
                };
                let approver = security
                    .approver
                    .clone()
                    .or_else(|| handlers::approval::connection_approver(mgr));
                // One absolute deadline governs both the local prompt and the
                // fallback for custom approvers. Preserve timeout as an outcome.
                let _pending = handlers::approval::pending_connection(mgr);
                let deadline = Instant::now() + APPROVAL_TIMEOUT;
                let approved = timeout_at(deadline, async {
                    match &approver {
                        Some(approver) => approver.request_until(request, deadline).await,
                        None => std::future::pending::<ConnectionApprovalOutcome>().await,
                    }
                })
                .await
                .unwrap_or(ConnectionApprovalOutcome::TimedOut);
                if approved != ConnectionApprovalOutcome::Allowed {
                    let reason = match approved {
                        ConnectionApprovalOutcome::TimedOut => "timeout",
                        _ => "rejected",
                    };
                    security.audit_denial(
                        mgr.now_us(),
                        "auth.denied",
                        Some(&peer),
                        serde_json::json!({"code": 140, "reason": reason}),
                    );
                    deny(send, ErrorCode::ApprovalDenied, reason).await?;
                    return Ok(HandshakeOutcome::Denied);
                }
            }
            Ok(HandshakeOutcome::Accepted(Box::new(Accepted {
                role,
                can_control,
                peer,
                device_fp: fp,
                device_name: hello.device_name,
                platform: pb::Platform::try_from(hello.platform).unwrap_or_default(),
                intent,
                client_features: hello.features,
                resumed,
            })))
        }
        AuthDecision::Denied { .. } => unreachable!("denial handled before ServerHello"),
    }
}

pub(crate) async fn send_frame(
    send: &mut HandshakeWriter<'_>,
    env: impl Into<pb::Envelope>,
) -> RuntimeResult<()> {
    let frame = encode(&env.into());
    match send {
        HandshakeWriter::Transport(send) => send.send(&frame[4..]).await?,
    }
    Ok(())
}

async fn deny(send: &mut HandshakeWriter<'_>, code: ErrorCode, reason: &str) -> RuntimeResult<()> {
    warn!(
        event = "auth.denied",
        code = code.code(),
        reason,
        "connection denied"
    );
    send_frame(
        send,
        pb::AuthResult {
            status: pb::auth_result::Status::Denied as i32,
            error: Some(pb::ErrorInfo {
                code: u32::from(code.code()),
                message: code.as_str().into(),
                retry_after_s: 0,
                detail: HashMap::from([("reason".into(), reason.into())]),
            }),
            ..Default::default()
        },
    )
    .await
}

pub(crate) fn role_to_pb(role: Role) -> i32 {
    match role {
        Role::Controller => pb::Role::Controller as i32,
        Role::Viewer => pb::Role::Viewer as i32,
    }
}

/// Shared H4 control path. Media and input stay disabled for the entire exchange.
pub(crate) async fn pairing_message<P: Platform>(
    session: &mut PairingSession,
    message: pb::envelope::Msg,
    send: &mut HandshakeWriter<'_>,
    security: &HandshakeContext,
    now_us: u64,
    mgr: &Arc<SessionManager<P>>,
) -> RuntimeResult<Option<CloseCode>> {
    let reply = match message {
        pb::envelope::Msg::PairPassword(msg) => {
            use crate::pairing::{PasswordPolicy, PermissionName};
            use rsp_core::trust::Permission;
            let config = mgr.services().config.get();
            let policy = PasswordPolicy {
                enabled: config.security.password_pairing,
                permission: match config.security.password_pairing_permission {
                    PermissionName::ViewOnly => Permission::ViewOnly,
                    PermissionName::Control => Permission::Control,
                    PermissionName::Unattended => Permission::Unattended,
                },
                authenticator: mgr.platform().system_authenticator(),
            };
            PairingReply::Result(session.on_password(now_us, msg, policy).await?)
        }
        pb::envelope::Msg::PairStart(start) => session.on_start(start, true)?,
        pb::envelope::Msg::PairSpake(spake) => session.on_spake(spake)?,
        pb::envelope::Msg::PairConfirm(confirm) => {
            let result = match session.on_confirm_outcome(confirm)? {
                PairingConfirmOutcome::Paired { confirm, result } => {
                    send_frame(send, confirm).await?;
                    result
                }
                PairingConfirmOutcome::WaitingLocal { confirm, result } => {
                    send_frame(send, confirm).await?;
                    send_frame(send, result).await?;
                    session.approve().await?
                }
                PairingConfirmOutcome::Result(result) => result,
            };
            PairingReply::Result(result)
        }
        _ => return Ok(Some(CloseCode::Protocol)),
    };
    match reply {
        PairingReply::Continue => Ok(None),
        PairingReply::Spake(spake) => {
            send_frame(send, spake).await?;
            Ok(None)
        }
        PairingReply::Result(result) => {
            let allowed = result.status == pb::pair_result::Status::Ok as i32;
            if allowed {
                handlers::approval::on_paired(mgr, &session.device_id());
                if session.method() == "password" {
                    security.audit_denial(
                        now_us,
                        "pairing.password",
                        Some(&pb::PeerInfo {
                            device_id: session.device_id(),
                            ..Default::default()
                        }),
                        serde_json::json!({"result": "ok", "account": session.password_account()}),
                    );
                }
                if session.method() == "file" {
                    security.audit_denial(
                        now_us,
                        "pairing.file",
                        Some(&pb::PeerInfo {
                            device_id: session.device_id(),
                            ..Default::default()
                        }),
                        serde_json::json!({"file_id": session.file_id()}),
                    );
                }
            } else {
                security.audit_denial(
                    now_us,
                    "pairing.denied",
                    None,
                    serde_json::json!({"status": result.status, "method": session.method()}),
                );
                warn!(
                    event = "pairing.denied",
                    status = result.status,
                    method = session.method(),
                    "pairing denied"
                );
            }
            send_frame(send, result).await?;
            Ok(Some(if allowed {
                CloseCode::Normal
            } else {
                CloseCode::Auth
            }))
        }
    }
}

#[allow(
    dead_code,
    reason = "legacy helper; wire ids are serialized by DisplayRegistry"
)]
pub(crate) fn display_to_pb(display: crate::platform::DisplayInfo) -> pb::Display {
    pb::Display {
        id: display.id,
        name: display.name,
        width_px: display.width_px,
        height_px: display.height_px,
        scale: display.scale,
        x: display.x,
        y: display.y,
        refresh_hz: display.refresh_hz,
        primary: display.primary,
        rotation: display.rotation,
        is_virtual: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{Event, Notifier};

    fn request() -> ConnectionApprovalRequest {
        ConnectionApprovalRequest {
            request_id: "deadline-test".into(),
            device_id: "test-device".into(),
            device_name: "test phone".into(),
            device_fp: [2; 32],
            role: Role::Controller,
            reasons: vec![auth::ApprovalReason::AttendedMode],
            peer: pb::PeerInfo::default(),
            platform: pb::Platform::Ios,
            path: PathClass::Lan,
            intent: None,
        }
    }

    struct LateReply;

    #[async_trait]
    impl ConnectionApprover for LateReply {
        async fn request(&self, _: ConnectionApprovalRequest) -> bool {
            tokio::time::advance(APPROVAL_TIMEOUT).await;
            false
        }
    }

    #[tokio::test(start_paused = true)]
    async fn legacy_approver_reply_at_deadline_is_timeout() {
        assert_eq!(
            LateReply
                .request_until(request(), Instant::now() + APPROVAL_TIMEOUT)
                .await,
            ConnectionApprovalOutcome::TimedOut
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ipc_timeout_closes_once_whether_waited_or_dropped() {
        for drop_at_deadline in [false, true] {
            let (events, mut received) = tokio::sync::broadcast::channel(8);
            let approver = Arc::new(IpcConnectionApprover::new(Notifier::from_sender(events)));
            let deadline = Instant::now() + APPROVAL_TIMEOUT;
            let mut wait = Box::pin(approver.request_until(request(), deadline));
            tokio::select! {
                biased;
                _ = &mut wait => panic!("approval must wait for a reply"),
                () = std::future::ready(()) => {}
            }
            assert!(matches!(
                received.try_recv().unwrap(),
                Event::ApprovalRequested(_)
            ));
            tokio::time::advance(APPROVAL_TIMEOUT).await;
            if drop_at_deadline {
                drop(wait);
            } else {
                assert_eq!(wait.await, ConnectionApprovalOutcome::TimedOut);
            }
            let Event::ApprovalClosed(payload) = received.try_recv().unwrap() else {
                panic!("expected approvalClosed");
            };
            assert_eq!(payload["outcome"], "timeout");
            assert!(received.try_recv().is_err());
            assert!(!approver.respond("deadline-test", true));
        }
    }
}
