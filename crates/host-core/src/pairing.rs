//! Host side pairing lifecycle and policy.
//!
//! The SPAKE2 state machine lives in [`rsp_core::pairing::HostPairing`]. This
//! module owns the short lived code, rate limits, local approval and durable
//! store update around that state machine. It deliberately has no transport
//! dependency; a session caller can map [`PairingReply`] to control frames.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use data_encoding::BASE64URL_NOPAD;
use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_core::pairfile::{file_name, PairFile};
use rsp_core::pairing::{HostPairing, PairingError as CorePairingError, MAX_CODE_FAILURES};
use rsp_core::pb;
use rsp_core::qr::PairQr;
use rsp_core::trust::Permission;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::ipc::{Event, Notifier};
use crate::pairfile_store::{PairFileError, PairFileStore, PairFileSummary};
use crate::session::approval::PromptEvents;
use crate::store::{PairedDevice, PairingPath, PathPermissions, Store, StoreError};
use crate::whois::WhoIs;

pub const PAIRING_CODE_LIFETIME_US: u64 = 5 * 60 * 1_000_000;
pub const PAIRING_ATTEMPT_WINDOW_US: u64 = PAIRING_CODE_LIFETIME_US;
pub const MAX_PAIRING_ATTEMPTS: usize = 20;

/// A source of cryptographically secure random bytes. Tests can inject a
/// deterministic implementation while production uses `getrandom`.
pub trait RandomSource: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), getrandom::Error>;
}

#[derive(Debug, Default)]
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), getrandom::Error> {
        getrandom::fill(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTicket {
    pub code: String,
    pub qr_url: String,
    pub expires_at: u64,
}

#[derive(Debug)]
pub struct ExportFileRequest {
    pub permission: Permission,
    pub label: Option<String>,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    pub file_id: String,
    pub path: PathBuf,
    pub file_name: String,
    pub expires_at_us: u64,
}

/// Default export directory. A newly created directory is private to its owner.
pub fn default_export_dir(state_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    let dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(|| state_dir.join("exports"));
    #[cfg(not(target_os = "macos"))]
    let dir = state_dir.join("exports");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Err(error) = builder.create(&dir) {
        warn!(%error, "create default pairing export directory failed");
    }
    dir
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingCodeState {
    Active,
    Cancelled,
    Expired,
    Used,
    Failed,
}

#[derive(Debug, Clone)]
struct ActiveCode {
    ticket: PairingTicket,
    failures: u32,
    state: PairingCodeState,
}

#[derive(Debug, Default)]
struct ManagerState {
    active: Option<ActiveCode>,
    attempts: VecDeque<u64>,
    last_now: u64,
}

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("pairing manager lock was poisoned")]
    Poisoned,
    #[error("random number generation failed: {0}")]
    Random(#[from] getrandom::Error),
    #[error("pairing code is unavailable")]
    CodeUnavailable,
    #[error("pairing code has expired")]
    Expired,
    #[error("pairing attempts are locked")]
    Locked,
    #[error("pairing is disabled on this path")]
    DisabledOnPath,
    #[error("invalid pairing path")]
    InvalidPath,
    #[error("SPAKE2 transition failed: {0}")]
    Protocol(#[from] CorePairingError),
    #[error("pairing store failed: {0}")]
    Store(#[from] StoreError),
    #[error("pairing approval failed")]
    Approval,
    #[error("pairing file: {0}")]
    PairFile(#[from] PairFileError),
    #[error("pairing files are unavailable")]
    FilesUnavailable,
    #[error("export failed: {0}")]
    Export(std::io::Error),
}

/// The transport-independent response produced by a session transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingReply {
    Spake(pb::PairSpake),
    /// A state transition with no control frame to emit.
    Continue,
    Result(pb::PairResult),
}

/// Details presented to the person approving a pairing request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairApprovalRequest {
    pub request_id: String,
    pub device_name: String,
    pub platform: pb::Platform,
    pub whois_login: Option<String>,
    pub device_fp: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairApproval {
    pub allow: bool,
    pub permission: Permission,
}

#[async_trait]
pub trait PairApprover: Send + Sync {
    async fn request(&self, request: PairApprovalRequest) -> PairApproval;
}

struct DenyApprover;

#[async_trait]
impl PairApprover for DenyApprover {
    async fn request(&self, _request: PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: false,
            permission: Permission::ViewOnly,
        }
    }
}

/// IPC-backed approver. `request` emits `event.approvalRequested`; the IPC
/// backend should call [`IpcPairApprover::respond`] for `approval.respond`.
pub struct IpcPairApprover {
    notifier: Notifier,
    pending: Mutex<std::collections::HashMap<String, oneshot::Sender<PairApproval>>>,
}

impl IpcPairApprover {
    pub fn new(notifier: Notifier) -> Self {
        Self {
            notifier,
            pending: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Deliver a response from the `approval.respond` IPC method. Returns
    /// `false` when the request is unknown or already timed out.
    pub fn respond(&self, request_id: &str, approval: PairApproval) -> bool {
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(request_id));
        sender.is_some_and(|sender| sender.send(approval).is_ok())
    }
}

#[async_trait]
impl PairApprover for IpcPairApprover {
    async fn request(&self, request: PairApprovalRequest) -> PairApproval {
        let request_id = request.request_id.clone();
        let payload = json!({
            "request_id": request.request_id,
            "kind": "pairing",
            "device_name": request.device_name,
            "platform": platform_name(request.platform),
            "whois_login": request.whois_login,
            "device_fp": fp_b64url(&request.device_fp),
        });
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id.clone(), sender);
        } else {
            return PairApproval {
                allow: false,
                permission: Permission::ViewOnly,
            };
        }
        // Register before broadcasting so a fast IPC client cannot respond in
        // the interval between the event and insertion in `pending`.
        if self
            .notifier
            .send(Event::ApprovalRequested(payload))
            .is_err()
        {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&request_id);
            }
            return PairApproval {
                allow: false,
                permission: Permission::ViewOnly,
            };
        }
        let result = tokio::time::timeout(Duration::from_secs(30), receiver).await;
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request_id);
        }
        match result {
            Ok(Ok(approval)) => approval,
            _ => PairApproval {
                allow: false,
                permission: Permission::ViewOnly,
            },
        }
    }
}

/// Outcome of the confirmation transition, including a BAD_CODE/EXPIRED
/// result without requiring a transport caller to inspect an error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingConfirmOutcome {
    Paired {
        confirm: pb::PairConfirm,
        result: pb::PairResult,
    },
    WaitingLocal {
        confirm: pb::PairConfirm,
        result: pb::PairResult,
    },
    Result(pb::PairResult),
}

/// A host pairing manager. Cloning it shares the active code and rate limit.
#[derive(Clone)]
pub struct PairingManager {
    inner: Arc<Mutex<ManagerState>>,
    store: Arc<Store>,
    files: Option<Arc<PairFileStore>>,
    password_lock: Option<Arc<crate::pwpair::PasswordLock>>,
    host_fp: [u8; 32],
    host_name: String,
    addresses: Vec<std::net::SocketAddr>,
    rng: Arc<dyn RandomSource>,
    approver: Arc<dyn PairApprover>,
    notifier: Option<Arc<dyn PromptEvents>>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for PairingManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingManager")
            .field("host_fp", &self.host_fp)
            .field("host_name", &self.host_name)
            .finish_non_exhaustive()
    }
}

impl PairingManager {
    pub fn new(
        store: Arc<Store>,
        host_fp: [u8; 32],
        host_name: impl Into<String>,
        addresses: Vec<std::net::SocketAddr>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerState::default())),
            store,
            files: None,
            password_lock: None,
            host_fp,
            host_name: host_name.into(),
            addresses,
            rng: Arc::new(OsRandom),
            approver: Arc::new(DenyApprover),
            notifier: None,
            clock: Arc::new(rsp_core::clock::unix_micros),
        }
    }

    pub fn with_rng(mut self, rng: Arc<dyn RandomSource>) -> Self {
        self.rng = rng;
        self
    }
    pub fn with_approver(mut self, approver: Arc<dyn PairApprover>) -> Self {
        self.approver = approver;
        self
    }
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(Arc::new(notifier));
        self
    }
    pub fn with_events(mut self, events: Arc<dyn PromptEvents>) -> Self {
        self.notifier = Some(events);
        self
    }
    pub fn has_events(&self) -> bool {
        self.notifier.is_some()
    }
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_files(mut self, files: Arc<PairFileStore>) -> Self {
        self.files = Some(files);
        self
    }

    pub fn with_password_lock(mut self, lock: Arc<crate::pwpair::PasswordLock>) -> Self {
        self.password_lock = Some(lock);
        self
    }

    pub fn has_files(&self) -> bool {
        self.files.is_some()
    }

    pub fn export_file(
        &self,
        now_us: u64,
        req: ExportFileRequest,
    ) -> Result<ExportedFile, PairingError> {
        if !req.dir.is_absolute() || !req.dir.is_dir() {
            return Err(PairingError::InvalidPath);
        }
        let files = self.files.as_ref().ok_or(PairingError::FilesUnavailable)?;
        let material = files.create(now_us, req.permission, req.label.as_deref())?;
        let file_id = BASE64URL_NOPAD.encode(&material.file_id);
        let pairfile = PairFile {
            v: 1,
            host_id: id_from_fp(&self.host_fp),
            fp: self.host_fp,
            file_id: material.file_id,
            secret: material.secret,
            name: self.host_name.clone(),
            addrs: self.addresses.iter().take(8).copied().collect(),
            expires_at_s: material.expires_at_us / 1_000_000,
        };
        let name = file_name(&self.host_name, now_us / 1_000_000);
        let stem = name.strip_suffix(".rspair").unwrap_or(&name);
        let write = || -> std::io::Result<(PathBuf, String)> {
            let uri = Zeroizing::new(pairfile.to_uri() + "\n");
            for number in 1u64.. {
                let name = if number == 1 {
                    name.clone()
                } else {
                    format!("{stem} ({number}).rspair")
                };
                let path = req.dir.join(&name);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = match options.open(&path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                };
                if let Err(error) = file
                    .write_all(uri.as_bytes())
                    .and_then(|()| file.sync_all())
                {
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok((path, name));
            }
            Err(std::io::Error::other("pairing export names exhausted"))
        };
        let (path, file_name) = match write() {
            Ok(exported) => exported,
            Err(error) => {
                files.revoke(&file_id)?;
                return Err(PairingError::Export(error));
            }
        };
        info!(%file_id, "pairing file exported");
        Ok(ExportedFile {
            file_id,
            path,
            file_name,
            expires_at_us: material.expires_at_us,
        })
    }

    pub fn list_files(&self, now_us: u64) -> Result<Vec<PairFileSummary>, PairingError> {
        Ok(self
            .files
            .as_ref()
            .ok_or(PairingError::FilesUnavailable)?
            .list(now_us))
    }

    pub fn revoke_file(&self, file_id: &str) -> Result<(), PairingError> {
        Ok(self
            .files
            .as_ref()
            .ok_or(PairingError::FilesUnavailable)?
            .revoke(file_id)?)
    }

    /// Start a code at the caller supplied Unix-microsecond timestamp.
    pub fn start(&self, now: u64) -> Result<PairingTicket, PairingError> {
        let code = generate_code(self.rng.as_ref())?;
        let qr = PairQr {
            v: 1,
            host_id: id_from_fp(&self.host_fp),
            fp: self.host_fp,
            code: code.clone(),
            name: self.host_name.clone(),
            addrs: self.addresses.clone(),
        };
        let ticket = PairingTicket {
            code,
            qr_url: qr.to_url(),
            expires_at: now.saturating_add(PAIRING_CODE_LIFETIME_US),
        };
        let mut state = self.inner.lock().map_err(|_| PairingError::Poisoned)?;
        state.last_now = now;
        state.active = Some(ActiveCode {
            ticket: ticket.clone(),
            failures: 0,
            state: PairingCodeState::Active,
        });
        info!("pairing code started");
        Ok(ticket)
    }

    pub fn cancel(&self) -> Result<(), PairingError> {
        let mut state = self.inner.lock().map_err(|_| PairingError::Poisoned)?;
        if let Some(active) = state.active.as_mut() {
            active.state = PairingCodeState::Cancelled;
        }
        info!("pairing code cancelled");
        Ok(())
    }

    pub fn code_state(&self, now: u64) -> Result<Option<(PairingCodeState, u32)>, PairingError> {
        let mut state = self.inner.lock().map_err(|_| PairingError::Poisoned)?;
        state.last_now = now;
        if let Some(active) = state.active.as_mut() {
            if active.state == PairingCodeState::Active && now >= active.ticket.expires_at {
                active.state = PairingCodeState::Expired;
            }
            return Ok(Some((active.state, active.failures)));
        }
        Ok(None)
    }

    pub fn session(
        &self,
        device_fp: [u8; 32],
        cert_der: Vec<u8>,
        whois_login: Option<String>,
        paired_via: impl Into<String>,
    ) -> PairingSession {
        PairingSession {
            manager: self.clone(),
            device_fp,
            cert_der,
            whois_login,
            whois_node: None,
            whois_node_name: None,
            paired_via: paired_via.into(),
            host_pairing: None,
            file: None,
            last_result: None,
            start: None,
            password_started: false,
            password_account: None,
            verified: false,
            request_id: String::new(),
            terminal: false,
        }
    }

    /// Supply the full, caller-resolved WhoIs identity for a new pairing.
    /// `session` remains available for callers that only have a login name.
    pub fn session_with_whois(
        &self,
        device_fp: [u8; 32],
        cert_der: Vec<u8>,
        whois: Option<WhoIs>,
        paired_via: PairingPath,
    ) -> PairingSession {
        let mut session = self.session(device_fp, cert_der, None, paired_via.as_str());
        if let Some(whois) = whois {
            session.whois_login = Some(whois.login);
            session.whois_node = Some(whois.stable_id);
            session.whois_node_name = Some(whois.node_name);
        }
        session
    }

    fn now(&self) -> u64 {
        self.inner
            .lock()
            .ok()
            .map(|state| state.last_now)
            .filter(|now| *now != 0)
            .unwrap_or_else(|| (self.clock)())
    }

    fn result(&self, status: pb::pair_result::Status, attempts_left: u32) -> PairingReply {
        PairingReply::Result(pb::PairResult {
            status: status as i32,
            host_id: if status == pb::pair_result::Status::Ok {
                id_from_fp(&self.host_fp)
            } else {
                String::new()
            },
            host_name: if status == pb::pair_result::Status::Ok {
                self.host_name.clone()
            } else {
                String::new()
            },
            attempts_left,
            ..Default::default()
        })
    }

    fn emit_result(
        &self,
        result: &pb::PairResult,
        device_fp: &[u8; 32],
        method: &str,
        device_name: &str,
        account: Option<&str>,
    ) {
        if let Some(events) = &self.notifier {
            let mut payload = json!({ "status": result.status, "host_id": result.host_id, "host_name": result.host_name, "attempts_left": result.attempts_left, "device_fp": fp_b64url(device_fp), "method": method, "device_name": device_name });
            if result.status == pb::pair_result::Status::Ok as i32 {
                if let Some(account) = account {
                    payload["account"] = json!(account);
                }
            }
            let _ = events.emit(Event::PairingResult(payload));
        }
    }

    fn check_start(&self, now: u64, allowed_on_path: bool) -> Result<(), PairingReply> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| self.result(pb::pair_result::Status::Expired, 0))?;
        state.last_now = now;
        while state
            .attempts
            .front()
            .is_some_and(|timestamp| now.saturating_sub(*timestamp) >= PAIRING_ATTEMPT_WINDOW_US)
        {
            state.attempts.pop_front();
        }
        if state.attempts.len() >= MAX_PAIRING_ATTEMPTS {
            let retry_after_s = state.attempts.front().map_or(0, |first| {
                PAIRING_ATTEMPT_WINDOW_US
                    .saturating_sub(now.saturating_sub(*first))
                    .div_ceil(1_000_000) as u32
            });
            return Err(PairingReply::Result(pb::PairResult {
                status: pb::pair_result::Status::Locked as i32,
                retry_after_s,
                ..Default::default()
            }));
        }
        state.attempts.push_back(now);
        if !allowed_on_path {
            return Err(self.result(pb::pair_result::Status::DisabledOnPath, 0));
        }
        Ok(())
    }

    fn check_code(&self, now: u64) -> Result<String, PairingReply> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| self.result(pb::pair_result::Status::Expired, 0))?;
        let Some(active) = state.active.as_mut() else {
            return Err(self.result(pb::pair_result::Status::Expired, 0));
        };
        if active.state != PairingCodeState::Active || now >= active.ticket.expires_at {
            if active.state == PairingCodeState::Active {
                active.state = PairingCodeState::Expired;
            }
            return Err(self.result(pb::pair_result::Status::Expired, 0));
        }
        Ok(active.ticket.code.clone())
    }

    fn fail_code(&self) -> Result<u32, PairingError> {
        let mut state = self.inner.lock().map_err(|_| PairingError::Poisoned)?;
        let active = state.active.as_mut().ok_or(PairingError::Expired)?;
        active.failures = active.failures.saturating_add(1);
        if active.failures >= MAX_CODE_FAILURES {
            active.state = PairingCodeState::Failed;
        }
        Ok(MAX_CODE_FAILURES.saturating_sub(active.failures))
    }

    fn consume_code(&self) -> Result<(), PairingError> {
        let mut state = self.inner.lock().map_err(|_| PairingError::Poisoned)?;
        let active = state.active.as_mut().ok_or(PairingError::Expired)?;
        active.state = PairingCodeState::Used;
        Ok(())
    }
}

/// Password pairing policy resolved from the current host configuration.
pub struct PasswordPolicy {
    pub enabled: bool,
    pub permission: Permission,
    pub authenticator: Option<Arc<dyn crate::sysauth::SystemAuthenticator>>,
}

/// One connection's pairing exchange. It is intentionally single-use.
pub struct PairingSession {
    manager: PairingManager,
    device_fp: [u8; 32],
    cert_der: Vec<u8>,
    whois_login: Option<String>,
    whois_node: Option<String>,
    whois_node_name: Option<String>,
    paired_via: String,
    host_pairing: Option<HostPairing>,
    // Only the public ID and exported permission survive SPAKE construction.
    file: Option<([u8; 16], Permission)>,
    last_result: Option<pb::PairResult>,
    start: Option<pb::PairStart>,
    password_started: bool,
    password_account: Option<String>,
    verified: bool,
    request_id: String,
    terminal: bool,
}

impl std::fmt::Debug for PairingSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingSession")
            .field("device_fp", &self.device_fp)
            .field("verified", &self.verified)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl PairingSession {
    pub fn method(&self) -> &'static str {
        if self.password_started {
            "password"
        } else if self
            .start
            .as_ref()
            .is_some_and(|start| !start.pair_file_id.is_empty())
        {
            "file"
        } else {
            "code"
        }
    }

    pub(crate) fn password_account(&self) -> Option<&str> {
        self.password_account.as_deref()
    }

    pub(crate) fn file_id(&self) -> Option<String> {
        self.file.as_ref().map(|(id, _)| BASE64URL_NOPAD.encode(id))
    }

    fn finish(&mut self, result: pb::PairResult) -> pb::PairResult {
        self.manager.emit_result(
            &result,
            &self.device_fp,
            self.method(),
            self.start
                .as_ref()
                .map_or("", |start| start.device_name.as_str()),
            self.password_account.as_deref(),
        );
        self.terminal = true;
        self.last_result = Some(result.clone());
        result
    }

    fn finish_reply(&mut self, reply: PairingReply) -> PairingReply {
        match reply {
            PairingReply::Result(result) => PairingReply::Result(self.finish(result)),
            other => other,
        }
    }

    fn fail_attempt(&mut self) -> Result<pb::PairResult, PairingError> {
        let attempts_left = if let Some((file_id, _)) = &self.file {
            self.manager
                .files
                .as_ref()
                .ok_or(PairingError::FilesUnavailable)?
                .fail((self.manager.clock)(), file_id)
        } else {
            self.manager.fail_code()?
        };
        let status = if attempts_left == 0 {
            pb::pair_result::Status::Expired
        } else {
            pb::pair_result::Status::BadCode
        };
        let PairingReply::Result(result) = self.manager.result(status, attempts_left) else {
            unreachable!("manager.result always returns PairResult");
        };
        Ok(self.finish(result))
    }

    pub(crate) fn device_id(&self) -> String {
        id_from_fp(&self.device_fp)
    }

    /// Authenticate once, persist the peer, and return a terminal result.
    pub async fn on_password(
        &mut self,
        now_us: u64,
        mut msg: pb::PairPassword,
        policy: PasswordPolicy,
    ) -> Result<pb::PairResult, PairingError> {
        let password = Zeroizing::new(std::mem::take(&mut msg.password));
        let deadline = tokio::time::Instant::now() + crate::pwpair::PASSWORD_MIN_FAILURE_DELAY;
        if self.terminal || self.start.is_some() || self.password_started {
            return Err(PairingError::Protocol(CorePairingError::Protocol));
        }
        self.password_started = true;
        self.start = Some(pb::PairStart {
            device_name: msg.device_name,
            platform: msg.platform,
            ..Default::default()
        });
        let reply = match (
            policy.enabled,
            policy.authenticator,
            self.manager.password_lock.clone(),
        ) {
            (true, Some(authenticator), Some(lock)) => {
                if let Err(reply) = self.manager.check_start(now_us, true) {
                    drop(password);
                    reply
                } else {
                    use crate::pwpair::PasswordVerdict;
                    let (status, attempts_left, retry_after_s) = match crate::pwpair::verify(
                        authenticator,
                        &lock,
                        &msg.account,
                        password,
                        now_us,
                    )
                    .await
                    {
                        PasswordVerdict::Ok(account) => {
                            match self
                                .manager
                                .store
                                .upsert(self.paired_device(policy.permission)?)
                            {
                                Ok(()) => {
                                    self.password_account = Some(account.short_name);
                                    (pb::pair_result::Status::Ok, 0, 0)
                                }
                                Err(StoreError::Revoked) => (pb::pair_result::Status::Denied, 0, 0),
                                Err(error) => return Err(error.into()),
                            }
                        }
                        PasswordVerdict::BadCredentials { attempts_left } => {
                            (pb::pair_result::Status::BadCode, attempts_left, 0)
                        }
                        PasswordVerdict::Locked { retry_after_s } => {
                            (pb::pair_result::Status::Locked, 0, retry_after_s)
                        }
                        PasswordVerdict::Unavailable => {
                            warn!(event = "pairing.password", result = "unavailable");
                            (pb::pair_result::Status::Denied, 0, 0)
                        }
                    };
                    let PairingReply::Result(mut result) =
                        self.manager.result(status, attempts_left)
                    else {
                        unreachable!("manager.result always returns PairResult");
                    };
                    result.retry_after_s = retry_after_s;
                    PairingReply::Result(result)
                }
            }
            _ => {
                drop(password);
                self.manager
                    .result(pb::pair_result::Status::MethodDisabled, 0)
            }
        };
        let PairingReply::Result(result) = reply else {
            unreachable!("password pairing always returns PairResult");
        };
        if result.status != pb::pair_result::Status::Ok as i32 {
            tokio::time::sleep_until(deadline).await;
        }
        Ok(self.finish(result))
    }

    pub fn on_start(
        &mut self,
        message: pb::PairStart,
        allowed_on_path: bool,
    ) -> Result<PairingReply, PairingError> {
        let now = if message.pair_file_id.is_empty() {
            self.manager.now()
        } else {
            (self.manager.clock)()
        };
        self.on_start_at(now, message, allowed_on_path)
    }

    pub fn on_start_at(
        &mut self,
        now: u64,
        message: pb::PairStart,
        allowed_on_path: bool,
    ) -> Result<PairingReply, PairingError> {
        if self.terminal || self.start.is_some() {
            return Err(PairingError::Protocol(CorePairingError::Protocol));
        }
        self.start = Some(message.clone());
        if let Err(reply) = self.manager.check_start(now, allowed_on_path) {
            return Ok(self.finish_reply(reply));
        }
        let mut pairing = if !message.pair_file_id.is_empty() {
            let grant = self
                .manager
                .files
                .as_ref()
                .ok_or(PairFileError::NotUsable)
                .and_then(|files| files.begin(now, &message.pair_file_id));
            let grant = match grant {
                Ok(grant) => grant,
                Err(PairFileError::NotUsable) => {
                    return Ok(
                        self.finish_reply(self.manager.result(pb::pair_result::Status::Expired, 0))
                    );
                }
                Err(error) => return Err(error.into()),
            };
            let file_id = <[u8; 16]>::try_from(message.pair_file_id.as_slice())
                .map_err(|_| PairFileError::NotUsable)?;
            let pairing = HostPairing::new_file(
                &file_id,
                &grant.secret,
                self.manager.host_fp,
                self.device_fp,
            );
            self.file = Some((file_id, grant.permission));
            pairing
        } else {
            let code = match self.manager.check_code(now) {
                Ok(code) => code,
                Err(reply) => return Ok(self.finish_reply(reply)),
            };
            HostPairing::new(code, self.manager.host_fp, self.device_fp)?
        };
        let spake = pairing.try_on_start(message.clone())?;
        self.host_pairing = Some(pairing);
        self.request_id = format!("pair-{}-{}", now, fp_b64url(&self.device_fp));
        debug!("pairing SPAKE2 started");
        Ok(PairingReply::Spake(spake))
    }

    pub fn on_spake(&mut self, message: pb::PairSpake) -> Result<PairingReply, PairingError> {
        if self.terminal {
            return Err(PairingError::Protocol(CorePairingError::Protocol));
        }
        let pairing = self
            .host_pairing
            .as_mut()
            .ok_or(PairingError::CodeUnavailable)?;
        if let Err(error) = pairing.on_spake(message) {
            // SPAKE2 cannot distinguish a wrong six digit password from a
            // malformed peer message without exposing an oracle. Both are a
            // failed attempt and consume one code failure.
            debug!(%error, "pairing SPAKE2 confirmation failed");
            return Ok(PairingReply::Result(self.fail_attempt()?));
        }
        // The app PairSpake is consumed here; its PairConfirm is handled next.
        Ok(PairingReply::Continue)
    }

    /// Verify the app confirmation. Codes return WAITING_LOCAL; files are
    /// consumed and persisted immediately with their exported permission.
    pub fn on_confirm(
        &mut self,
        message: pb::PairConfirm,
    ) -> Result<(pb::PairConfirm, pb::PairResult), PairingError> {
        if self.terminal {
            return Err(PairingError::Protocol(CorePairingError::Protocol));
        }
        let pairing = self
            .host_pairing
            .as_mut()
            .ok_or(PairingError::CodeUnavailable)?;
        let confirm = match pairing.on_confirm(message) {
            Ok(confirm) => confirm,
            Err(error @ (CorePairingError::BadCode | CorePairingError::ManInTheMiddle)) => {
                self.fail_attempt()?;
                return Err(PairingError::Protocol(error));
            }
            Err(error) => return Err(PairingError::Protocol(error)),
        };
        self.verified = true;
        if self.file.is_some() {
            let result = self.complete_file()?;
            return Ok((confirm, result));
        }
        Ok((
            confirm,
            pb::PairResult {
                status: pb::pair_result::Status::WaitingLocal as i32,
                host_id: String::new(),
                host_name: String::new(),
                attempts_left: MAX_CODE_FAILURES,
                ..Default::default()
            },
        ))
    }

    /// Confirmation API that exposes protocol failures as PairResult statuses.
    pub fn on_confirm_outcome(
        &mut self,
        message: pb::PairConfirm,
    ) -> Result<PairingConfirmOutcome, PairingError> {
        match self.on_confirm(message) {
            Ok((confirm, result)) if self.method() == "file" => {
                if result.status == pb::pair_result::Status::Ok as i32 {
                    Ok(PairingConfirmOutcome::Paired { confirm, result })
                } else {
                    Ok(PairingConfirmOutcome::Result(result))
                }
            }
            Ok((confirm, result)) => Ok(PairingConfirmOutcome::WaitingLocal { confirm, result }),
            Err(PairingError::Protocol(
                CorePairingError::BadCode | CorePairingError::ManInTheMiddle,
            )) => Ok(PairingConfirmOutcome::Result(
                self.last_result
                    .clone()
                    .ok_or(PairingError::CodeUnavailable)?,
            )),
            Err(error) => Err(error),
        }
    }

    fn complete_file(&mut self) -> Result<pb::PairResult, PairingError> {
        let (file_id, permission) = self.file.ok_or(PairingError::FilesUnavailable)?;
        let device = self.paired_device(permission)?;
        let files = self
            .manager
            .files
            .as_ref()
            .ok_or(PairingError::FilesUnavailable)?;
        let status = match files.consume((self.manager.clock)(), &file_id, &self.device_id()) {
            Err(PairFileError::NotUsable) => pb::pair_result::Status::Expired,
            Err(error) => return Err(error.into()),
            Ok(()) => match self.manager.store.upsert(device) {
                Ok(()) => {
                    info!(file_id = %BASE64URL_NOPAD.encode(&file_id), "pairing completed");
                    pb::pair_result::Status::Ok
                }
                Err(StoreError::Revoked) => pb::pair_result::Status::Denied,
                Err(error) => return Err(error.into()),
            },
        };
        let reply = self.manager.result(status, 0);
        let PairingReply::Result(result) = self.finish_reply(reply) else {
            unreachable!("manager.result always returns PairResult");
        };
        Ok(result)
    }

    fn paired_device(&self, permission: Permission) -> Result<PairedDevice, PairingError> {
        let start = self.start.as_ref().ok_or(PairingError::CodeUnavailable)?;
        let paired_via = match self.paired_via.as_str() {
            "tailnet" => PairingPath::Tailnet,
            "intranet" => PairingPath::Intranet,
            "lan" => PairingPath::Lan,
            _ => return Err(PairingError::InvalidPath),
        };
        Ok(PairedDevice {
            fp: self.device_fp,
            device_id: self.device_id(),
            name: start.device_name.clone(),
            cert_der: self.cert_der.clone(),
            permission,
            paired_at_us: (self.manager.clock)(),
            require_confirm: false,
            platform: pb::Platform::try_from(start.platform)
                .unwrap_or_default()
                .into(),
            paired_via,
            whois_user: self.whois_login.clone(),
            whois_node: self.whois_node.clone(),
            whois_node_name: self.whois_node_name.clone(),
            path_permissions: PathPermissions::default(),
        })
    }

    /// Ask the local approver and persist the device on approval. This method
    /// is separate from `on_confirm` so transports can send WAITING_LOCAL
    /// before waiting for a UI response.
    pub async fn approve(&mut self) -> Result<pb::PairResult, PairingError> {
        if !self.verified || self.terminal {
            return Err(PairingError::Protocol(CorePairingError::Protocol));
        }
        let start = self.start.clone().ok_or(PairingError::CodeUnavailable)?;
        let platform = match pb::Platform::try_from(start.platform) {
            Ok(platform) => platform,
            Err(_) => pb::Platform::Unspecified,
        };
        let approval = match tokio::time::timeout(
            Duration::from_secs(30),
            self.manager.approver.request(PairApprovalRequest {
                request_id: self.request_id.clone(),
                device_name: start.device_name.clone(),
                platform,
                whois_login: self.whois_login.clone(),
                device_fp: self.device_fp,
            }),
        )
        .await
        {
            Ok(approval) => approval,
            Err(_) => PairApproval {
                allow: false,
                permission: Permission::ViewOnly,
            },
        };
        if !approval.allow {
            self.manager.consume_code()?;
            self.terminal = true;
            let result = pb::PairResult {
                status: pb::pair_result::Status::Denied as i32,
                host_id: String::new(),
                host_name: String::new(),
                attempts_left: 0,
                ..Default::default()
            };
            return Ok(self.finish(result));
        }
        let device = self.paired_device(approval.permission)?;
        if let Err(error) = self.manager.store.upsert(device) {
            self.manager.consume_code()?;
            self.terminal = true;
            if matches!(error, StoreError::Revoked) {
                warn!("pairing rejected for revoked device");
                let result = pb::PairResult {
                    status: pb::pair_result::Status::Denied as i32,
                    host_id: String::new(),
                    host_name: String::new(),
                    attempts_left: 0,
                    ..Default::default()
                };
                return Ok(self.finish(result));
            }
            return Err(PairingError::Store(error));
        }
        self.manager.consume_code()?;
        self.terminal = true;
        let result = pb::PairResult {
            status: pb::pair_result::Status::Ok as i32,
            host_id: id_from_fp(&self.manager.host_fp),
            host_name: self.manager.host_name.clone(),
            attempts_left: 0,
            ..Default::default()
        };
        info!("pairing completed");
        Ok(self.finish(result))
    }

    pub fn paired_via(&self) -> &str {
        &self.paired_via
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingDigestEntry {
    pub fp: String,
    pub permission: PermissionName,
    pub paired_at_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionName {
    ViewOnly,
    Control,
    Unattended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PairingsDigest {
    pub devices: Vec<PairingDigestEntry>,
}

/// Build the H15 pairing summary, sorted by fingerprint.
pub fn pairings_digest(store: &Store) -> Result<PairingsDigest, StoreError> {
    let devices = store
        .paired()?
        .into_iter()
        .map(|device| PairingDigestEntry {
            fp: fp_b64url(&device.fp),
            permission: permission_name(device.permission),
            paired_at_us: device.paired_at_us,
        })
        .collect::<Vec<_>>();
    // `Store::paired` is backed by a BTreeMap keyed by the raw fingerprint,
    // so retaining its order gives the digest's required bytewise ordering.
    Ok(PairingsDigest { devices })
}

fn permission_name(permission: Permission) -> PermissionName {
    match permission {
        Permission::ViewOnly => PermissionName::ViewOnly,
        Permission::Control => PermissionName::Control,
        Permission::Unattended => PermissionName::Unattended,
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

fn generate_code(random: &dyn RandomSource) -> Result<String, PairingError> {
    // Rejection sampling over a u32 avoids modulo bias.
    const LIMIT: u64 = (u32::MAX as u64 / 1_000_000) * 1_000_000;
    loop {
        let mut bytes = [0u8; 4];
        random.fill(&mut bytes)?;
        let value = u64::from(u32::from_le_bytes(bytes));
        if value >= LIMIT {
            continue;
        }
        return Ok(format!("{value:06}", value = value % 1_000_000));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Events(Mutex<Vec<Event>>);

    impl PromptEvents for Events {
        fn emit(&self, event: Event) -> bool {
            self.0.lock().expect("events lock").push(event);
            true
        }
    }

    #[test]
    fn with_events_emits_pairing_result_payload() {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        let path =
            std::path::PathBuf::from(format!("target/pairing-events-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let store = Arc::new(Store::open(&path).expect("store"));
        let manager =
            PairingManager::new(store, [1; 32], "host", Vec::new()).with_events(events.clone());
        assert!(manager.has_events());
        let result = pb::PairResult {
            status: pb::pair_result::Status::Ok as i32,
            attempts_left: 0,
            ..Default::default()
        };
        manager.emit_result(&result, &[2; 32], "code", "phone", None);
        let emitted = events.0.lock().expect("events lock");
        assert!(
            matches!(emitted.as_slice(), [Event::PairingResult(payload)] if payload["method"] == "code" && payload["device_name"] == "phone")
        );
        let _ = std::fs::remove_dir_all(path);
    }
}
