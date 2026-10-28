//! Durable host pairing, revocation and trust synchronization state.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use prost::Message;
use rsp_core::ids::{fp_b64url, fp_from_b64url};
use rsp_core::pb;
use rsp_core::trust::{
    check_bind_node, spki_der_from_cert, valid_subject_node, BindNodeClaim, DeviceView,
    KnownDevice, Permission, RejectReason,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

const VERSION: u32 = 1;
const PAIRED_FILE: &str = "paired.json";
const REVOKED_FILE: &str = "revoked.json";
const BOUND_FILE: &str = "bound_nodes.json";
const TRUST_FILE: &str = "trust_state.json";
const PENDING_MAX: usize = 256;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("store I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt store file {file}")]
    Corrupt { file: String },
    #[error("invalid fingerprint in store file {file}")]
    InvalidFingerprint { file: String },
    #[error("invalid permission in store file {file}")]
    InvalidPermission { file: String },
    #[error("device has been permanently revoked")]
    Revoked,
    #[error("store lock was poisoned")]
    Poisoned,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DevicePlatform {
    Ios,
    Ipados,
    Android,
    Macos,
    Windows,
    /// 未知值（包括以后的新平台）都读成 Unknown，不让整份配对表读失败。
    #[default]
    #[serde(other)]
    Unknown,
}

impl From<rsp_core::pb::Platform> for DevicePlatform {
    fn from(platform: rsp_core::pb::Platform) -> Self {
        match platform {
            rsp_core::pb::Platform::Ios => Self::Ios,
            rsp_core::pb::Platform::Ipados => Self::Ipados,
            rsp_core::pb::Platform::Android => Self::Android,
            rsp_core::pb::Platform::Macos => Self::Macos,
            rsp_core::pb::Platform::Windows => Self::Windows,
            _ => Self::Unknown,
        }
    }
}

/// Records without path metadata use the default listener path, tailnet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingPath {
    #[default]
    Tailnet,
    Intranet,
    Lan,
}

impl PairingPath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tailnet => "tailnet",
            Self::Intranet => "intranet",
            Self::Lan => "lan",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPermission {
    #[default]
    Allow,
    ViewOnly,
    Deny,
}

/// Route restrictions supplement the device's overall permission. §04 does
/// not prescribe stricter defaults; all routes default to allow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathPermissions {
    pub tailnet: PathPermission,
    pub intranet: PathPermission,
    pub lan: PathPermission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    pub fp: [u8; 32],
    pub device_id: String,
    pub name: String,
    pub cert_der: Vec<u8>,
    pub permission: Permission,
    pub paired_at_us: u64,
    pub require_confirm: bool,
    pub platform: DevicePlatform,
    pub paired_via: PairingPath,
    pub whois_user: Option<String>,
    pub whois_node: Option<String>,
    pub whois_node_name: Option<String>,
    pub path_permissions: PathPermissions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevokedDevice {
    pub fp: [u8; 32],
    pub revoked_at_us: u64,
    pub source_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundNode {
    pub fp: [u8; 32],
    pub node: String,
    pub issued_at_us: u64,
    pub source_seq: u64,
}

/// An unverified declaration retained until its signer becomes paired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingBind {
    pub fp: [u8; 32],
    pub signed: Vec<u8>,
    pub issued_at_us: u64,
    pub source_seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustState {
    pub last_applied_seq: u64,
    pub last_trust_sync_at_us: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    Unknown,
    Revoked,
    Paired {
        permission: Permission,
        require_confirm: bool,
        whois_user: Option<String>,
        whois_node: Option<String>,
        bound_node: Option<String>,
        path_permissions: PathPermissions,
    },
}

#[derive(Clone, Debug, Default)]
struct State {
    paired: BTreeMap<[u8; 32], PairedDevice>,
    revoked: BTreeMap<[u8; 32], RevokedDevice>,
    bound: BTreeMap<[u8; 32], BoundNode>,
    pending: BTreeMap<[u8; 32], PendingBind>,
    trust: TrustState,
}

#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    state: Mutex<State>,
}

#[derive(Serialize, Deserialize)]
struct PairedFile {
    version: u32,
    devices: Vec<PairedJson>,
}

#[derive(Serialize, Deserialize)]
struct PairedJson {
    fp: String,
    device_id: String,
    name: String,
    cert_der: String,
    permission: String,
    paired_at_us: u64,
    require_confirm: bool,
    #[serde(default)]
    platform: DevicePlatform,
    #[serde(default)]
    paired_via: PairingPath,
    #[serde(default)]
    whois_user: Option<String>,
    #[serde(default)]
    whois_node: Option<String>,
    #[serde(default)]
    whois_node_name: Option<String>,
    #[serde(default)]
    path_permissions: PathPermissions,
}

#[derive(Serialize, Deserialize)]
struct RevokedFile {
    version: u32,
    revoked: Vec<RevokedJson>,
}

#[derive(Serialize, Deserialize)]
struct RevokedJson {
    fp: String,
    revoked_at_us: u64,
    source_seq: u64,
}

#[derive(Serialize, Deserialize)]
struct BoundFile {
    version: u32,
    nodes: Vec<BoundJson>,
    // Preserve the original on-disk representation when there are no pending claims.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<PendingJson>,
}

#[derive(Serialize, Deserialize)]
struct BoundJson {
    fp: String,
    node: String,
    issued_at_us: u64,
    source_seq: u64,
}

#[derive(Serialize, Deserialize)]
struct PendingJson {
    fp: String,
    issued_at_us: u64,
    source_seq: u64,
    signed: String,
}

#[derive(Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    last_applied_seq: u64,
    last_trust_sync_at_us: u64,
}

impl Store {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        tighten_dir(&dir)?;
        for file in [PAIRED_FILE, REVOKED_FILE, BOUND_FILE, TRUST_FILE] {
            let tmp = dir.join(format!(".{file}.tmp"));
            if tmp.exists() {
                fs::remove_file(tmp)?;
            }
            let path = dir.join(file);
            if path.exists() {
                tighten_file(&path)?;
            }
        }

        let mut state = State {
            paired: read_paired(&dir.join(PAIRED_FILE))?,
            revoked: read_revoked(&dir.join(REVOKED_FILE))?,
            trust: read_trust(&dir.join(TRUST_FILE))?,
            ..Default::default()
        };
        read_bound(&dir.join(BOUND_FILE), &mut state)?;
        // A crash can persist paired/revoked.json before bound_nodes.json.
        // Finish consuming those pending claims before exposing the store.
        for fp in state.pending.keys().copied().collect::<Vec<_>>() {
            if state.revoked.contains_key(&fp) {
                state.pending.remove(&fp);
            } else if let Some(device) = state.paired.get(&fp) {
                consume_pending_bind(device, &mut state.bound, &mut state.pending);
            }
        }
        let store = Self {
            dir,
            state: Mutex::new(state),
        };
        // Materialize missing files so a newly created store has the documented
        // four-file layout.
        {
            let guard = store.lock()?;
            store.persist_locked(&guard)?;
        }
        Ok(store)
    }

    pub fn paired(&self) -> Result<Vec<PairedDevice>, StoreError> {
        let state = self.lock()?;
        Ok(state.paired.values().cloned().collect())
    }

    pub fn bound_nodes(&self) -> Result<Vec<BoundNode>, StoreError> {
        Ok(self.lock()?.bound.values().cloned().collect())
    }

    pub fn pending_binds(&self) -> Result<Vec<PendingBind>, StoreError> {
        Ok(self.lock()?.pending.values().cloned().collect())
    }

    pub fn get(&self, fp: &[u8; 32]) -> Result<Option<PairedDevice>, StoreError> {
        Ok(self.lock()?.paired.get(fp).cloned())
    }

    pub fn upsert(&self, device: PairedDevice) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        let fp = device.fp;
        if state.revoked.contains_key(&fp) {
            return Err(StoreError::Revoked);
        }
        let mut next = state.clone();
        next.paired.insert(fp, device.clone());
        consume_pending_bind(&device, &mut next.bound, &mut next.pending);
        self.persist_locked(&next)?;
        *state = next;
        Ok(())
    }

    pub fn remove(&self, fp: &[u8; 32]) -> Result<Option<PairedDevice>, StoreError> {
        let mut state = self.lock()?;
        let removed = state.paired.remove(fp);
        let bound = state.bound.remove(fp);
        let pending = state.pending.remove(fp);
        if removed.is_some() || bound.is_some() || pending.is_some() {
            self.persist_locked(&state)?;
        }
        Ok(removed)
    }

    /// Update a paired device; returns false when the fingerprint is unknown.
    /// Revocation always takes precedence over an update.
    pub fn set_permission(
        &self,
        fp: &[u8; 32],
        permission: Permission,
    ) -> Result<bool, StoreError> {
        self.update_device(fp, |device| device.permission = permission)
    }

    pub fn set_path_permission(
        &self,
        fp: &[u8; 32],
        path: PairingPath,
        permission: PathPermission,
    ) -> Result<bool, StoreError> {
        self.update_device(fp, |device| match path {
            PairingPath::Tailnet => device.path_permissions.tailnet = permission,
            PairingPath::Intranet => device.path_permissions.intranet = permission,
            PairingPath::Lan => device.path_permissions.lan = permission,
        })
    }

    fn update_device(
        &self,
        fp: &[u8; 32],
        update: impl FnOnce(&mut PairedDevice),
    ) -> Result<bool, StoreError> {
        let mut state = self.lock()?;
        if state.revoked.contains_key(fp) {
            return Err(StoreError::Revoked);
        }
        let Some(device) = state.paired.get_mut(fp) else {
            return Ok(false);
        };
        let previous = device.clone();
        update(device);
        if let Err(error) = self.persist_locked(&state) {
            state.paired.insert(*fp, previous);
            return Err(error);
        }
        Ok(true)
    }

    pub fn is_revoked(&self, fp: &[u8; 32]) -> Result<bool, StoreError> {
        Ok(self.lock()?.revoked.contains_key(fp))
    }

    pub fn revoke(&self, fp: [u8; 32], seq: u64) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        state.paired.remove(&fp);
        state.bound.remove(&fp);
        state.pending.remove(&fp);
        state.revoked.insert(
            fp,
            RevokedDevice {
                fp,
                revoked_at_us: rsp_core::clock::unix_micros(),
                source_seq: seq,
            },
        );
        self.persist_locked(&state)
    }

    pub fn trust_state(&self) -> Result<TrustState, StoreError> {
        Ok(self.lock()?.trust.clone())
    }

    pub fn set_trust_state(&self, trust: TrustState) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        state.trust = trust;
        self.persist_locked(&state)
    }

    pub fn authorize_device(&self, fp: &[u8; 32]) -> Result<DeviceStatus, StoreError> {
        let state = self.lock()?;
        if state.revoked.contains_key(fp) {
            return Ok(DeviceStatus::Revoked);
        }
        if let Some(device) = state.paired.get(fp) {
            return Ok(DeviceStatus::Paired {
                permission: device.permission,
                require_confirm: device.require_confirm,
                whois_user: device.whois_user.clone(),
                whois_node: device.whois_node.clone(),
                bound_node: state.bound.get(fp).map(|bound| bound.node.clone()),
                path_permissions: device.path_permissions,
            });
        }
        Ok(DeviceStatus::Unknown)
    }

    pub(crate) fn snapshot(&self) -> Result<StateSnapshot, StoreError> {
        let state = self.lock()?;
        Ok(StateSnapshot {
            paired: state.paired.clone(),
            revoked: state.revoked.clone(),
            bound: state.bound.clone(),
            pending: state.pending.clone(),
            trust: state.trust.clone(),
        })
    }

    pub(crate) fn replace_snapshot(&self, snapshot: StateSnapshot) -> Result<(), StoreError> {
        let mut state = self.lock()?;
        let old = state.clone();
        state.paired = snapshot.paired;
        state.revoked = snapshot.revoked;
        state.bound = snapshot.bound;
        state.pending = snapshot.pending;
        state.trust = snapshot.trust;
        if let Err(error) = self.persist_locked(&state) {
            *state = old;
            return Err(error);
        }
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, StoreError> {
        self.state.lock().map_err(|_| StoreError::Poisoned)
    }

    // Persist paired.json, revoked.json, bound_nodes.json, then trust_state.json.
    // Each file is atomically replaced and synced; trust_state is the checkpoint.
    // A crash between writes leaves last_applied_seq unchanged, so restarting
    // replays those entries. Approvals never overwrite existing pairings or
    // revive revoked keys; revokes retain their original metadata, and setting
    // require_confirm again is harmless. BIND_NODE with the same issued_at_us
    // and node leaves the binding unchanged. These idempotent actions converge
    // before the checkpoint advances, even after partial writes.
    fn persist_locked(&self, state: &State) -> Result<(), StoreError> {
        write_json(&self.dir.join(PAIRED_FILE), &paired_file(state))?;
        write_json(&self.dir.join(REVOKED_FILE), &revoked_file(state))?;
        write_json(&self.dir.join(BOUND_FILE), &bound_file(state))?;
        write_json(&self.dir.join(TRUST_FILE), &trust_file(state))
    }
}

pub(crate) struct StateSnapshot {
    pub(crate) paired: BTreeMap<[u8; 32], PairedDevice>,
    pub(crate) revoked: BTreeMap<[u8; 32], RevokedDevice>,
    pub(crate) bound: BTreeMap<[u8; 32], BoundNode>,
    pub(crate) pending: BTreeMap<[u8; 32], PendingBind>,
    pub(crate) trust: TrustState,
}

pub(crate) fn insert_pending_bind(
    pending: &mut BTreeMap<[u8; 32], PendingBind>,
    entry: PendingBind,
) {
    if pending
        .get(&entry.fp)
        .is_some_and(|old| old.issued_at_us >= entry.issued_at_us)
    {
        return;
    }
    pending.insert(entry.fp, entry);
    if pending.len() > PENDING_MAX {
        if let Some(fp) = pending
            .values()
            .min_by_key(|entry| entry.source_seq)
            .map(|e| e.fp)
        {
            pending.remove(&fp);
        }
    }
}

/// H21 replacement rules, shared by immediate and deferred verification.
pub(crate) fn record_bound_node(
    bound: &mut BTreeMap<[u8; 32], BoundNode>,
    claim: BindNodeClaim,
    source_seq: u64,
) -> Result<(), RejectReason> {
    let old = bound.get(&claim.fp);
    if old.is_some_and(|old| {
        old.issued_at_us > claim.issued_at_us
            || (old.issued_at_us == claim.issued_at_us && old.node != claim.node)
    }) {
        return Err(RejectReason::StaleBinding);
    }
    if old.is_none_or(|old| old.issued_at_us != claim.issued_at_us) {
        bound.insert(
            claim.fp,
            BoundNode {
                fp: claim.fp,
                node: claim.node,
                issued_at_us: claim.issued_at_us,
                source_seq,
            },
        );
    }
    Ok(())
}

pub(crate) fn consume_pending_bind(
    device: &PairedDevice,
    bound: &mut BTreeMap<[u8; 32], BoundNode>,
    pending: &mut BTreeMap<[u8; 32], PendingBind>,
) {
    let Some(entry) = pending.remove(&device.fp) else {
        return;
    };
    let result = (|| {
        let signed = pb::SignedStatement::decode(entry.signed.as_slice())
            .map_err(|_| RejectReason::InvalidStatement)?;
        let spki = spki_der_from_cert(&device.cert_der)?;
        let claim = check_bind_node(&signed, &spki, None)?;
        if claim.fp != device.fp {
            return Err(RejectReason::BindSignerMismatch);
        }
        record_bound_node(bound, claim, entry.source_seq)
    })();
    match result {
        Ok(()) => {
            info!(device_id = %device.device_id, source_seq = entry.source_seq, "pending node binding applied")
        }
        Err(reason) => {
            warn!(device_id = %device.device_id, ?reason, "pending node binding discarded")
        }
    }
}

impl DeviceView for Store {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice> {
        let state = self.state.lock().ok()?;
        if state.revoked.contains_key(fp) {
            return Some(KnownDevice {
                spki_der: Vec::new(),
                permission: Permission::ViewOnly,
                revoked: true,
            });
        }
        if let Some(device) = state.paired.get(fp) {
            let spki_der = certificate_spki(&device.cert_der).unwrap_or_default();
            return Some(KnownDevice {
                spki_der,
                permission: device.permission,
                revoked: false,
            });
        }
        None
    }
}

fn certificate_spki(cert_der: &[u8]) -> Result<Vec<u8>, ()> {
    let (remaining, cert) = x509_parser::parse_x509_certificate(cert_der).map_err(|_| ())?;
    if !remaining.is_empty() {
        return Err(());
    }
    Ok(cert.public_key().raw.to_vec())
}

fn paired_file(state: &State) -> PairedFile {
    PairedFile {
        version: VERSION,
        devices: state
            .paired
            .values()
            .map(|device| PairedJson {
                fp: fp_b64url(&device.fp),
                device_id: device.device_id.clone(),
                name: device.name.clone(),
                cert_der: data_encoding::BASE64URL_NOPAD.encode(&device.cert_der),
                permission: permission_name(device.permission).to_owned(),
                paired_at_us: device.paired_at_us,
                require_confirm: device.require_confirm,
                platform: device.platform,
                paired_via: device.paired_via,
                whois_user: device.whois_user.clone(),
                whois_node: device.whois_node.clone(),
                whois_node_name: device.whois_node_name.clone(),
                path_permissions: device.path_permissions,
            })
            .collect(),
    }
}

fn revoked_file(state: &State) -> RevokedFile {
    RevokedFile {
        version: VERSION,
        revoked: state
            .revoked
            .values()
            .map(|entry| RevokedJson {
                fp: fp_b64url(&entry.fp),
                revoked_at_us: entry.revoked_at_us,
                source_seq: entry.source_seq,
            })
            .collect(),
    }
}

fn trust_file(state: &State) -> TrustFile {
    TrustFile {
        version: VERSION,
        last_applied_seq: state.trust.last_applied_seq,
        last_trust_sync_at_us: state.trust.last_trust_sync_at_us,
    }
}

fn bound_file(state: &State) -> BoundFile {
    BoundFile {
        version: VERSION,
        nodes: state
            .bound
            .values()
            .map(|entry| BoundJson {
                fp: fp_b64url(&entry.fp),
                node: entry.node.clone(),
                issued_at_us: entry.issued_at_us,
                source_seq: entry.source_seq,
            })
            .collect(),
        pending: state
            .pending
            .values()
            .map(|entry| PendingJson {
                fp: fp_b64url(&entry.fp),
                issued_at_us: entry.issued_at_us,
                source_seq: entry.source_seq,
                signed: data_encoding::BASE64URL_NOPAD.encode(&entry.signed),
            })
            .collect(),
    }
}

fn read_paired(path: &Path) -> Result<BTreeMap<[u8; 32], PairedDevice>, StoreError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file = read_json::<PairedFile>(path)?;
    if file.version != VERSION {
        return Err(corrupt(path));
    }
    let mut result = BTreeMap::new();
    for device in file.devices {
        let fp = parse_fp(&device.fp, path)?;
        let cert_der = data_encoding::BASE64URL_NOPAD
            .decode(device.cert_der.as_bytes())
            .map_err(|_| corrupt(path))?;
        let permission = parse_permission(&device.permission, path)?;
        result.insert(
            fp,
            PairedDevice {
                fp,
                device_id: device.device_id,
                name: device.name,
                cert_der,
                permission,
                paired_at_us: device.paired_at_us,
                require_confirm: device.require_confirm,
                platform: device.platform,
                paired_via: device.paired_via,
                whois_user: device.whois_user,
                whois_node: device.whois_node,
                whois_node_name: device.whois_node_name,
                path_permissions: device.path_permissions,
            },
        );
    }
    Ok(result)
}

fn read_revoked(path: &Path) -> Result<BTreeMap<[u8; 32], RevokedDevice>, StoreError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file = read_json::<RevokedFile>(path)?;
    if file.version != VERSION {
        return Err(corrupt(path));
    }
    let mut result = BTreeMap::new();
    for entry in file.revoked {
        let fp = parse_fp(&entry.fp, path)?;
        result.insert(
            fp,
            RevokedDevice {
                fp,
                revoked_at_us: entry.revoked_at_us,
                source_seq: entry.source_seq,
            },
        );
    }
    Ok(result)
}

fn read_trust(path: &Path) -> Result<TrustState, StoreError> {
    if !path.exists() {
        return Ok(TrustState::default());
    }
    let file = read_json::<TrustFile>(path)?;
    if file.version != VERSION {
        return Err(corrupt(path));
    }
    Ok(TrustState {
        last_applied_seq: file.last_applied_seq,
        last_trust_sync_at_us: file.last_trust_sync_at_us,
    })
}

fn read_bound(path: &Path, state: &mut State) -> Result<(), StoreError> {
    if !path.exists() {
        return Ok(());
    }
    let file = read_json::<BoundFile>(path)?;
    if file.version != VERSION {
        return Err(corrupt(path));
    }
    for entry in file.nodes {
        let fp = fp_from_b64url(&entry.fp).map_err(|_| corrupt(path))?;
        state.bound.insert(
            fp,
            BoundNode {
                fp,
                node: entry.node,
                issued_at_us: entry.issued_at_us,
                source_seq: entry.source_seq,
            },
        );
    }
    for entry in file.pending {
        let fp = fp_from_b64url(&entry.fp).map_err(|_| corrupt(path))?;
        let signed = data_encoding::BASE64URL_NOPAD
            .decode(entry.signed.as_bytes())
            .map_err(|_| corrupt(path))?;
        let decoded = pb::SignedStatement::decode(signed.as_slice()).map_err(|_| corrupt(path))?;
        let statement =
            pb::Statement::decode(decoded.statement.as_slice()).map_err(|_| corrupt(path))?;
        if decoded.signer_fp != fp
            || statement.subject_fp != fp
            || statement.kind != pb::statement::Kind::BindNode as i32
            || decoded.signature.is_empty()
            || !valid_subject_node(&statement.subject_node)
            || entry.source_seq == 0
            || statement.issued_at_us != entry.issued_at_us
        {
            return Err(corrupt(path));
        }
        insert_pending_bind(
            &mut state.pending,
            PendingBind {
                fp,
                signed,
                issued_at_us: entry.issued_at_us,
                source_seq: entry.source_seq,
            },
        );
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path).map_err(StoreError::Io)?;
    serde_json::from_slice(&bytes).map_err(|_| corrupt(path))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| corrupt(path))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut file = options.open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    tighten_file(path)?;
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(())
}

fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::ViewOnly => "view_only",
        Permission::Control => "control",
        Permission::Unattended => "unattended",
    }
}

fn parse_permission(value: &str, path: &Path) -> Result<Permission, StoreError> {
    match value {
        "view_only" => Ok(Permission::ViewOnly),
        "control" => Ok(Permission::Control),
        "unattended" => Ok(Permission::Unattended),
        _ => Err(StoreError::InvalidPermission {
            file: file_name(path),
        }),
    }
}

fn parse_fp(value: &str, path: &Path) -> Result<[u8; 32], StoreError> {
    fp_from_b64url(value).map_err(|_| StoreError::InvalidFingerprint {
        file: file_name(path),
    })
}

fn corrupt(path: &Path) -> StoreError {
    StoreError::Corrupt {
        file: file_name(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(unix)]
fn tighten_dir(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o700 {
        warn!(path = %path.display(), "tightening host store directory permissions");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_dir(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
fn tighten_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        warn!(path = %path.display(), "tightening host store file permissions");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_file(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn sync_dir(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}
