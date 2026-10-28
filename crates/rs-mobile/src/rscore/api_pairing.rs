//! Pairing and the locally paired Host list for the RsCore JSON boundary.

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};

use data_encoding::BASE64URL_NOPAD;
use rsp_client::session::{
    EndReason, PairingCredential, Session, SessionError, SessionEvent, SessionHandle, SessionParams,
};
use rsp_core::errors::ErrorCode;
use rsp_core::ids::{fp_b64url, fp_from_b64url, id_from_fp};
use rsp_core::pairfile::{self, PairFile, PAIRFILE_MAX_BYTES};
use rsp_core::pb;
use rsp_transport::identity::LocalIdentity;
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::api_diag::discover_address;
use super::api_sessions::record_host_seen;
use super::candidates::{host_target, qr_candidates, HostTarget};
use super::dto_core::{
    net_path_from_pb, CoreEvent, DevicePlatform, NetPath, PairFileSummary, PairOutcome,
    PairStartArg, PairStep, PairTarget, PairedHost,
};
use super::error::CoreError;
use super::netinfo::{local_net_excluding, read_interfaces};
use super::runtime::{core, Core};
use super::settings::SettingsState;
use super::store::PairedHostRow;

const FILE_CACHE_LIFETIME_US: u64 = 10 * 60 * 1_000_000;
const FILE_CACHE_CAPACITY: usize = 8;
/// The App maps this failure to its unsupported-target pairing message.
pub const NOT_A_HOST: &str = "not a remote host";

pub enum LoadFileSrc {
    Text { text: Zeroizing<String> },
    Uri { uri: String },
}

impl std::fmt::Debug for LoadFileSrc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoadFileSrc(<redacted>)")
    }
}

impl<'de> serde::Deserialize<'de> for LoadFileSrc {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn secret_text<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Zeroizing<String>>, D::Error> {
            Ok(Option::<String>::deserialize(deserializer)?.map(Zeroizing::new))
        }
        #[derive(serde::Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "secret_text")]
            text: Option<Zeroizing<String>>,
            uri: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        match (wire.text, wire.uri) {
            (Some(text), None) => Ok(Self::Text { text }),
            (None, Some(uri)) => Ok(Self::Uri { uri }),
            _ => Err(serde::de::Error::custom("expected text or uri")),
        }
    }
}

#[derive(Debug)]
struct CachedFile {
    id: String,
    loaded_at: u64,
    file: Arc<PairFile>,
}

#[derive(Debug, Default)]
pub struct PairFileCache(Mutex<VecDeque<CachedFile>>);

impl PairFileCache {
    fn prune(entries: &mut VecDeque<CachedFile>, now: u64) {
        entries.retain(|entry| now.saturating_sub(entry.loaded_at) < FILE_CACHE_LIFETIME_US);
    }

    fn insert(&self, id: String, file: PairFile, now: u64) {
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        Self::prune(&mut entries, now);
        entries.retain(|entry| entry.id != id);
        entries.push_back(CachedFile {
            id,
            loaded_at: now,
            file: Arc::new(file),
        });
        while entries.len() > FILE_CACHE_CAPACITY {
            entries.pop_front();
        }
    }

    fn get(&self, id: &str, now: u64) -> Option<Arc<PairFile>> {
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        Self::prune(&mut entries, now);
        entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.file.clone())
    }

    fn remove(&self, id: &str, now: u64) {
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        Self::prune(&mut entries, now);
        entries.retain(|entry| entry.id != id);
    }
}

pub fn load_file(core: &Arc<Core>, src: LoadFileSrc) -> Result<PairFileSummary, CoreError> {
    let invalid = || CoreError::internal("not a pairing file");
    let file = match src {
        LoadFileSrc::Text { text } => pairfile::parse(&text).map_err(|_| invalid())?,
        LoadFileSrc::Uri { uri } => {
            if !uri.starts_with("file://") {
                return Err(CoreError::unsupported("only file:// is supported"));
            }
            let path = url::Url::parse(&uri)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(invalid)?;
            let mut bytes = Zeroizing::new(Vec::with_capacity(PAIRFILE_MAX_BYTES + 1));
            std::fs::File::open(path)
                .map_err(|_| invalid())?
                .take((PAIRFILE_MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| invalid())?;
            let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
            pairfile::parse(text).map_err(|_| invalid())?
        }
    };
    let now = core.now_us();
    let summary = PairFileSummary {
        file_id: BASE64URL_NOPAD.encode(&file.file_id),
        host_id: file.host_id.clone(),
        host_name: file.name.clone(),
        fp: fp_b64url(&file.fp),
        addrs: file.addrs.iter().map(ToString::to_string).collect(),
        expires_at: file.expires_at_s.saturating_mul(1_000_000),
        expired: file.is_expired(now),
    };
    core.state::<PairFileCache>()
        .insert(summary.file_id.clone(), file, now);
    Ok(summary)
}

#[derive(Default)]
pub struct PairingState {
    pub current: Mutex<Option<Arc<watch::Sender<bool>>>>,
}

/// Only the owning attempt may clear the current cancellation handle. Closing
/// on drop also handles a caller dropping its start future after Session::open.
pub struct PairingRun {
    pub state: Arc<PairingState>,
    pub cancellation: Arc<watch::Sender<bool>>,
    pub session: Option<SessionHandle>,
}

impl Drop for PairingRun {
    fn drop(&mut self) {
        if let Some(handle) = &self.session {
            handle.close();
        }
        let mut current = self
            .state
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current
            .as_ref()
            .is_some_and(|sender| Arc::ptr_eq(sender, &self.cancellation))
        {
            *current = None;
        }
    }
}

pub fn parse_qr(text: &str) -> Option<PairTarget> {
    let qr = rsp_core::qr::parse(text).ok()?;
    Some(PairTarget {
        v: 1,
        host_id: qr.host_id,
        fp: fp_b64url(&qr.fp),
        code: qr.code,
        name: qr.name,
        addrs: qr.addrs.iter().map(ToString::to_string).collect(),
    })
}

pub fn pairing_error(message: impl ToString) -> PairOutcome {
    PairOutcome::Error {
        code: None,
        message: message.to_string(),
    }
}

pub fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

pub async fn resolve_target(
    core: &Arc<Core>,
    arg: PairStartArg,
) -> Result<(HostTarget, PairingCredential), PairOutcome> {
    let from_file = matches!(&arg, PairStartArg::File { .. });
    let (host_id, fp, name, credential, qr_addrs) = match arg {
        PairStartArg::Target(target) => {
            if target.v != 1 {
                return Err(pairing_error("unsupported pairing QR version"));
            }
            let fp = fp_from_b64url(&target.fp)
                .map_err(|_| pairing_error("invalid host fingerprint"))?;
            if id_from_fp(&fp) != target.host_id {
                return Err(pairing_error("host id mismatch"));
            }
            if !valid_code(&target.code) {
                return Err(pairing_error("pairing code must contain six ASCII digits"));
            }
            (
                target.host_id,
                fp,
                target.name,
                PairingCredential::Code(target.code),
                qr_candidates(&target.addrs),
            )
        }
        PairStartArg::CodeAddr { addr, port, code } => {
            if !valid_code(&code) {
                return Err(pairing_error("pairing code must contain six ASCII digits"));
            }
            // SPAKE2 binds both TLS fingerprints, so the one learned here is
            // trusted only once the Host confirms the code over it.
            core.events().emit(&progress(PairStep::Dialing));
            let (candidates, fp) = match discover_address(core, &addr, port).await {
                Ok(Some(found)) => found,
                Ok(None) => return Err(pairing_error(NOT_A_HOST)),
                Err(error) => return Err(pairing_error(error)),
            };
            (
                id_from_fp(&fp),
                fp,
                addr.trim().to_owned(),
                PairingCredential::Code(code),
                candidates,
            )
        }
        PairStartArg::File { file_id } => {
            let file = cached_file(core, &file_id)?;
            let addrs = file
                .addrs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            (
                file.host_id.clone(),
                file.fp,
                file.name.clone(),
                PairingCredential::File {
                    file_id: file.file_id,
                    secret: file.secret.clone(),
                },
                qr_candidates(&addrs),
            )
        }
    };
    let file_name = from_file.then(|| name.clone());
    let mut target = build_target(core, host_id, fp, name, qr_addrs)?;
    if let Some(name) = file_name {
        target.host_name = name;
    }
    Ok((target, credential))
}

fn cached_file(core: &Arc<Core>, id: &str) -> Result<Arc<PairFile>, PairOutcome> {
    let file = core
        .state::<PairFileCache>()
        .get(id, core.now_us())
        .ok_or_else(|| pairing_error("pairing file not loaded"))?;
    if file.is_expired(core.now_us()) {
        return Err(PairOutcome::Expired);
    }
    Ok(file)
}

fn build_target(
    core: &Arc<Core>,
    host_id: String,
    fp: [u8; 32],
    name: String,
    qr_addrs: Vec<rsp_transport::dial::Candidate>,
) -> Result<HostTarget, PairOutcome> {
    let settings = core.state::<SettingsState>();
    let (vpn_pools, intranet_cidrs, work_profile_ifaces) = {
        let settings = settings.0.lock().unwrap_or_else(|error| error.into_inner());
        (
            settings.vpn_pools(),
            settings.intranet_cidrs(),
            settings.work_profile_ifaces.clone(),
        )
    };
    let raw = read_interfaces().map_err(pairing_error)?;
    let local = local_net_excluding(&raw, &vpn_pools, &work_profile_ifaces);
    // This temporary row supplies the pinned identity and the QR addresses,
    // not paired status; host_target adds the network settings.
    let row = PairedHostRow {
        host_id: host_id.clone(),
        fp,
        name,
        addresses: qr_addrs.iter().map(|candidate| candidate.addr).collect(),
        paired_at_us: 0,
        permission: None,
    };
    let mut target =
        host_target(&host_id, Some(&row), None, &local, &intranet_cidrs).map_err(|error| {
            PairOutcome::Error {
                code: Some(error.code() as u32),
                message: error.to_string(),
            }
        })?;
    let mut candidates = qr_addrs;
    candidates.append(&mut target.candidates);
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.addr));
    target.candidates = candidates;
    Ok(target)
}

pub fn progress(step: PairStep) -> CoreEvent {
    CoreEvent::PairingProgress {
        step,
        host_name: None,
        path: None,
        latency_ms: None,
        fp: None,
        timeout_s: None,
    }
}

pub fn result_outcome(
    result: pb::PairResult,
    expected_host_id: &str,
    path: Option<NetPath>,
    latency_ms: u32,
) -> Option<PairOutcome> {
    use pb::pair_result::Status;
    Some(match Status::try_from(result.status) {
        Ok(Status::WaitingLocal) => return None,
        Ok(Status::Ok) if result.host_id != expected_host_id => pairing_error("host id mismatch"),
        Ok(Status::Ok) => PairOutcome::Ok {
            host_id: result.host_id,
            host_name: result.host_name,
            permission: None,
            path,
            latency_ms: Some(latency_ms),
        },
        Ok(Status::BadCode) => PairOutcome::BadCode {
            attempts_left: result.attempts_left,
        },
        Ok(Status::Denied) => PairOutcome::Denied,
        Ok(Status::Expired) => PairOutcome::Expired,
        Ok(Status::Locked) => PairOutcome::Locked {
            retry_after_s: (result.retry_after_s > 0).then_some(result.retry_after_s),
        },
        Ok(Status::MethodDisabled) => PairOutcome::MethodDisabled,
        Ok(Status::DisabledOnPath) => PairOutcome::DisabledOnPath,
        _ => pairing_error("unknown pairing result"),
    })
}

pub fn ended_outcome(code: Option<ErrorCode>, kind: EndReason, reason: String) -> PairOutcome {
    // rsp-client does not expose confirmation failures separately. Pairing
    // protocol termination is therefore treated as MITM by this API.
    if kind == EndReason::Protocol {
        PairOutcome::Mitm
    } else {
        PairOutcome::Error {
            code: code.map(|code| code as u32),
            message: reason,
        }
    }
}

pub fn open_error(error: SessionError) -> PairOutcome {
    match error {
        SessionError::Dial {
            code: ErrorCode::FingerprintMismatch,
            ..
        } => PairOutcome::Mitm,
        SessionError::Dial { code, .. } => PairOutcome::Error {
            code: Some(code as u32),
            message: format!("dial failed: {code:?}"),
        },
        error => pairing_error(error),
    }
}

pub fn save_pairing(
    core: &Arc<Core>,
    target: &HostTarget,
    outcome: &PairOutcome,
) -> Result<(), CoreError> {
    let PairOutcome::Ok {
        host_id, host_name, ..
    } = outcome
    else {
        return Ok(());
    };
    let now = core.now_us();
    core.store().upsert_paired_host(&PairedHostRow {
        host_id: host_id.clone(),
        fp: target.fp,
        name: host_name.clone(),
        addresses: target
            .candidates
            .iter()
            .map(|candidate| candidate.addr)
            .collect(),
        paired_at_us: now,
        permission: None,
    })?;
    Ok(())
}

pub async fn pair(
    core: &Arc<Core>,
    arg: PairStartArg,
    identity: Arc<LocalIdentity>,
    device_name: String,
    session: &mut Option<SessionHandle>,
) -> PairOutcome {
    pair_request(core, arg, identity, device_name, session).await
}

async fn pair_request(
    core: &Arc<Core>,
    request: PairStartArg,
    identity: Arc<LocalIdentity>,
    device_name: String,
    session: &mut Option<SessionHandle>,
) -> PairOutcome {
    let file_id = match &request {
        PairStartArg::File { file_id } => Some(file_id.clone()),
        _ => None,
    };
    // Address pairing already reported dialing while learning the fingerprint.
    let dialed = matches!(&request, PairStartArg::CodeAddr { .. });
    let (target, credential) = match resolve_target(core, request).await {
        Ok(target) => target,
        Err(outcome) => return outcome,
    };
    let password_pairing = matches!(&credential, PairingCredential::Password { .. });
    if !dialed {
        core.events().emit(&progress(PairStep::Dialing));
    }
    let params = SessionParams::from_shared(target.candidates.clone(), target.fp, identity);
    let mut params = match credential {
        PairingCredential::Code(code) => params.with_pairing_code(code),
        PairingCredential::File { file_id, secret } => params.with_pairing_file(file_id, secret),
        PairingCredential::Password { account, password } => {
            params.with_pairing_password(account, password)
        }
    };
    params.intent = pb::Intent::Pair;
    params.net = core.net();
    params.device_name = device_name;
    params.platform = match core.config().platform {
        DevicePlatform::Ios => pb::Platform::Ios,
        DevicePlatform::Ipados => pb::Platform::Ipados,
        DevicePlatform::Android => pb::Platform::Android,
        DevicePlatform::Macos => pb::Platform::Macos,
        DevicePlatform::Windows => pb::Platform::Windows,
    };
    params.local_net = target.observations.local.clone();
    params.intranet_cidrs = target.intranet_cidrs.clone();
    params.observations = target.observations.clone();
    let (handle, mut events) = match Session::open(params).await {
        Ok(session) => session,
        Err(error) => return open_error(error),
    };
    let latency_ms = u32::try_from(handle.handshake_timings().dial_us / 1000).unwrap_or(u32::MAX);
    *session = Some(handle);
    let mut event = progress(PairStep::Verifying);
    if let CoreEvent::PairingProgress {
        fp, latency_ms: ms, ..
    } = &mut event
    {
        *fp = Some(fp_b64url(&target.fp));
        *ms = Some(latency_ms);
    }
    core.events().emit(&event);
    let mut path = None;
    let mut name = target.host_name.clone();
    let mut seen = None;
    let outcome = loop {
        match events.recv().await {
            Some(SessionEvent::ServerHello(hello)) => {
                seen = Some(hello.clone());
                if !hello.host_name.is_empty() {
                    name = hello.host_name.clone();
                }
                path = hello
                    .peer
                    .and_then(|peer| pb::PathKind::try_from(peer.path).ok())
                    .and_then(net_path_from_pb);
                let mut event = progress(PairStep::Exchanging);
                if let CoreEvent::PairingProgress {
                    host_name,
                    path: event_path,
                    ..
                } = &mut event
                {
                    *host_name = Some(hello.host_name);
                    *event_path = path;
                }
                core.events().emit(&event);
            }
            Some(SessionEvent::PairResult(result)) => {
                if !password_pairing
                    && result.status == pb::pair_result::Status::WaitingLocal as i32
                {
                    let mut event = progress(PairStep::WaitingLocal);
                    if let CoreEvent::PairingProgress {
                        host_name,
                        timeout_s,
                        ..
                    } = &mut event
                    {
                        // Pairing hellos and WAITING_LOCAL may withhold the
                        // name; use the QR/file name until OK supplies it.
                        *host_name = Some(if result.host_name.is_empty() {
                            name.clone()
                        } else {
                            result.host_name.clone()
                        });
                        *timeout_s = Some(30);
                    }
                    core.events().emit(&event);
                }
                if let Some(outcome) = result_outcome(result, &target.host_id, path, latency_ms) {
                    break outcome;
                }
            }
            Some(SessionEvent::Ended { code, kind, reason }) => {
                break ended_outcome(code, kind, reason)
            }
            None => break pairing_error("pairing session ended without a result"),
            _ => {}
        }
    };
    if matches!(outcome, PairOutcome::Ok { .. }) {
        if let Some(id) = file_id {
            core.state::<PairFileCache>().remove(&id, core.now_us());
        }
    }
    match save_pairing(core, &target, &outcome) {
        Ok(()) => {
            // The pairing hello is this Host's first metadata; sessions add
            // the OS and displays it withholds from pairing callers.
            if let (PairOutcome::Ok { host_id, .. }, Some(hello)) = (&outcome, &seen) {
                record_host_seen(core, host_id, hello, false);
            }
            outcome
        }
        Err(error) => pairing_error(error),
    }
}

pub async fn start(core: &Arc<Core>, arg: PairStartArg) -> Result<PairOutcome, CoreError> {
    start_request(core, arg).await
}

async fn start_request(core: &Arc<Core>, request: PairStartArg) -> Result<PairOutcome, CoreError> {
    // Identity failures reject without emitting pairing.done or replacing a
    // currently running attempt.
    let identity = core.local_identity()?;
    let stored = core
        .store()
        .identity()?
        .ok_or_else(|| CoreError::internal("identity not created"))?;
    let (sender, mut cancelled) = watch::channel(false);
    let cancellation = Arc::new(sender);
    let state = core.state::<PairingState>();
    {
        let mut current = state
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = current.replace(cancellation.clone()) {
            previous.send_replace(true);
        }
    }
    let mut run = PairingRun {
        state,
        cancellation,
        session: None,
    };
    let outcome = tokio::select! {
        biased;
        _ = cancelled.wait_for(|cancelled| *cancelled) => PairOutcome::Cancelled,
        _ = tokio::time::sleep(core.timeouts().pairing) => PairOutcome::Timeout,
        outcome = pair_request(core, request, identity, stored.device_name, &mut run.session) => outcome,
    };
    // Close before publishing completion, including cancellation and timeout.
    drop(run);
    core.events().emit(&CoreEvent::PairingDone {
        outcome: outcome.clone(),
    });
    Ok(outcome)
}

pub fn cancel(core: &Arc<Core>) {
    let state = core.state::<PairingState>();
    let current = state
        .current
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(sender) = current.as_ref() {
        sender.send_replace(true);
    }
}

pub fn paired(core: &Arc<Core>) -> Result<Vec<PairedHost>, CoreError> {
    let store = core.store();
    store
        .paired_hosts()?
        .into_iter()
        .map(|row| {
            let seen = store.host_seen(&row.host_id)?;
            Ok(PairedHost {
                host_id: row.host_id,
                fp: fp_b64url(&row.fp),
                name: row.name,
                addresses: row.addresses.iter().map(ToString::to_string).collect(),
                paired_at: row.paired_at_us,
                os: seen.as_ref().and_then(|v| v.os.clone()),
                os_version: seen.as_ref().and_then(|v| v.os_version.clone()),
                host_version: seen.as_ref().and_then(|v| v.host_version.clone()),
                features: seen.as_ref().and_then(|v| v.features),
                displays: seen
                    .as_ref()
                    .map(|v| v.displays.clone())
                    .unwrap_or_default(),
                last_seen_at: seen.map(|v| v.seen_at_us),
            })
        })
        .collect()
}

pub fn forget(core: &Arc<Core>, host_id: &str) -> Result<(), CoreError> {
    core.store().forget_host(host_id)?;
    Ok(())
}

#[uniffi::export]
pub fn pairing_parse_qr(text: String) -> Option<String> {
    parse_qr(&text).and_then(|target| serde_json::to_string(&target).ok())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn pairing_start(arg_json: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move {
            let arg = serde_json::from_str(&arg_json)?;
            Ok(serde_json::to_string(&start(&c, arg).await?)?)
        })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn pairing_load_file(src_json: String) -> Result<String, CoreError> {
    let src_json = Zeroizing::new(src_json);
    let c = core()?;
    c.clone()
        .run(async move {
            let src = serde_json::from_str(&src_json)
                .map_err(|_| CoreError::internal("not a pairing file"))?;
            Ok(serde_json::to_string(&load_file(&c, src)?)?)
        })
        .await
}

#[uniffi::export]
pub fn pairing_cancel() {
    if let Ok(c) = core() {
        cancel(&c);
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn hosts_paired() -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&paired(&c)?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn hosts_forget(host_id: String) -> Result<(), CoreError> {
    let c = core()?;
    c.clone().run(async move { forget(&c, &host_id) }).await
}
