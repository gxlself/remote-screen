//! Durable second-node bindings and the authentication boundary.
use std::{
    convert::Infallible,
    fs,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use host_core::{
    auth::{
        self, ApprovalReason, AuthDecision, AuthInput, AuthPolicy, AuthWarning, PathClass,
        SessionSnapshot, WhoIsOutcome,
    },
    store::{BoundNode, DeviceStatus, PairedDevice, Store, StoreError},
    trust::{apply, ApplyReport},
};
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::EncodePrivateKey,
};
use prost::Message;
use rsp_core::{
    ids::{fp_b64url, id_from_fp},
    pb,
    trust::{
        build_approve, build_bind_node, build_statement, sign_with, subject_fingerprint,
        Permission, RejectReason, Rejected, TrustAction,
    },
};
use serde_json::{json, Value};

const USER: &str = "alice";
const ISSUED: u64 = 1_000_000;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        let path = root.join(format!(
            "rs-bind-node-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn device(byte: u8, node: Option<&str>) -> (SigningKey, PairedDevice) {
    // Test-only deterministic private key, never used by production code.
    let key = SigningKey::from_bytes((&[byte; 32]).into()).unwrap();
    let pkcs8 = key.to_pkcs8_der().unwrap();
    let cert_key = rcgen::KeyPair::try_from(pkcs8.as_bytes()).unwrap();
    let cert_der = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .unwrap()
        .self_signed(&cert_key)
        .unwrap()
        .der()
        .to_vec();
    let fp = subject_fingerprint(&cert_der).unwrap();
    (
        key,
        PairedDevice {
            fp,
            device_id: id_from_fp(&fp),
            name: "test phone".into(),
            cert_der,
            permission: Permission::Control,
            paired_at_us: 42,
            require_confirm: false,
            platform: Default::default(),
            paired_via: Default::default(),
            whois_user: Some(USER.into()),
            whois_node: node.map(str::to_owned),
            whois_node_name: None,
            path_permissions: Default::default(),
        },
    )
}
fn signed(
    key: &SigningKey,
    fp: [u8; 32],
    statement: &pb::Statement,
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let signed = sign_with(statement, fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .unwrap();
    rsp_core::trust::TrustEntry { seq, signed }
}
fn control_statement(
    kind: pb::statement::Kind,
    fp: [u8; 32],
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let (key, issuer) = device(6, None);
    signed(
        &key,
        issuer.fp,
        &build_statement(kind, USER, fp, [], "", ISSUED, [3; 16]),
        seq,
    )
}
fn open_store(path: &std::path::Path) -> Result<Store, StoreError> {
    let store = Store::open(path)?;
    let issuer = device(6, None).1;
    if store.get(&issuer.fp)?.is_none() {
        store.upsert(issuer)?;
    }
    Ok(store)
}
struct Fixture {
    store: Arc<Store>,
    key: SigningKey,
    device: PairedDevice,
    dir: Dir,
}
impl Fixture {
    fn new(node: Option<&str>) -> Self {
        let dir = Dir::new();
        let store = Arc::new(open_store(&dir.0).unwrap());
        let (key, device) = device(7, node);
        store.upsert(device.clone()).unwrap();
        Self {
            store,
            key,
            device,
            dir,
        }
    }
    fn bound() -> Self {
        let f = Self::new(Some("n1"));
        let report = f.bind("n2", ISSUED, 1);
        assert!(report.rejected.is_empty());
        assert_eq!(report.applied.len(), 1);
        f
    }
    fn entry(&self, node: &str, issued_at_us: u64, seq: u64) -> rsp_core::trust::TrustEntry {
        signed(
            &self.key,
            self.device.fp,
            &build_bind_node(USER, self.device.fp, node, issued_at_us, [3; 16]).unwrap(),
            seq,
        )
    }
    fn bind(&self, node: &str, issued_at_us: u64, seq: u64) -> ApplyReport {
        apply(&self.store, USER, &[self.entry(node, issued_at_us, seq)]).unwrap()
    }
    fn nodes(&self) -> Vec<BoundNode> {
        self.store.bound_nodes().unwrap()
    }
    fn bound_bytes(&self) -> Vec<u8> {
        fs::read(self.dir.0.join("bound_nodes.json")).unwrap()
    }
    fn assert_mismatch(&self, node: &str, mismatch: bool) {
        let input = AuthInput {
            intent: pb::Intent::Control,
            local_addr: "100.64.0.1:0".parse().unwrap(),
            peer_addr: "100.64.0.2:0".parse().unwrap(),
            path_class: PathClass::Tailnet,
            device: self.store.authorize_device(&self.device.fp).unwrap().into(),
            whois: WhoIsOutcome::Ok {
                login: USER.into(),
                stable_id: node.into(),
                node_name: "phone".into(),
            },
            policy: AuthPolicy {
                allowed_users: vec![USER.into()],
                ..Default::default()
            },
            trust_stale_24h: false,
            session: SessionSnapshot::default(),
            local_activity_within_60s: false,
        };
        let AuthDecision::Accept {
            needs_approval,
            warnings,
            ..
        } = auth::decide(&input)
        else {
            panic!("expected accepted connection");
        };
        assert_eq!(
            needs_approval
                .unwrap_or_default()
                .contains(&ApprovalReason::NodeMismatch),
            mismatch
        );
        assert_eq!(warnings.contains(&AuthWarning::NodeMismatch), mismatch);
    }
}
fn assert_stale(report: ApplyReport, seq: u64) {
    assert!(report.applied.is_empty());
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq,
            reason: RejectReason::StaleBinding
        }]
    );
}
fn assert_bound(store: &Store, fp: &[u8; 32], expected: Option<&str>) {
    let DeviceStatus::Paired { bound_node, .. } = store.authorize_device(fp).unwrap() else {
        panic!("expected paired device");
    };
    assert_eq!(bound_node.as_deref(), expected);
}

#[test]
fn h01_binding_is_durable_private_and_advances_seq() {
    let f = Fixture::bound();
    assert_bound(&f.store, &f.device.fp, Some("n2"));
    assert_eq!(
        f.nodes(),
        vec![BoundNode {
            fp: f.device.fp,
            node: "n2".into(),
            issued_at_us: ISSUED,
            source_seq: 1
        }]
    );
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 1);
    let path = f.dir.0.join("bound_nodes.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&f.bound_bytes()).unwrap(),
        json!({"version": 1, "nodes": [{"fp": fp_b64url(&f.device.fp), "node": "n2", "issued_at_us": ISSUED, "source_seq": 1}]})
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(path.exists());
    drop(f.store);
    let reopened = open_store(&f.dir.0).unwrap();
    assert_bound(&reopened, &f.device.fp, Some("n2"));
    assert_eq!(reopened.trust_state().unwrap().last_applied_seq, 1);
}
#[test]
fn h02_bound_node_matches() {
    Fixture::bound().assert_mismatch("n2", false);
}
#[test]
fn h03_original_node_matches() {
    Fixture::bound().assert_mismatch("n1", false);
}
#[test]
fn h04_unrecognized_node_requires_confirmation() {
    Fixture::bound().assert_mismatch("n3", true);
}
#[test]
fn h05_without_original_node_binding_does_not_restrict() {
    let f = Fixture::new(None);
    assert!(f.bind("n2", ISSUED, 1).rejected.is_empty());
    for node in ["n1", "n2", "n3", "any-node"] {
        f.assert_mismatch(node, false);
    }
}
#[test]
fn h06_newer_binding_replaces_old_node() {
    let f = Fixture::bound();
    let report = f.bind("n3", ISSUED + 1, 2);
    assert!(report.rejected.is_empty());
    assert_eq!(report.applied.len(), 1);
    assert_bound(&f.store, &f.device.fp, Some("n3"));
    assert_eq!(f.nodes()[0].source_seq, 2);
    f.assert_mismatch("n2", true);
    f.assert_mismatch("n3", false);
    f.assert_mismatch("n1", false);
}
#[test]
fn h07_older_binding_rejected_but_seq_advances() {
    let f = Fixture::bound();
    let before = f.bound_bytes();
    assert_stale(f.bind("n3", ISSUED - 1, 2), 2);
    assert_eq!(f.bound_bytes(), before);
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 2);
}
#[test]
fn h08_identical_binding_replay_preserves_file_bytes() {
    let f = Fixture::bound();
    let before = f.bound_bytes();
    let report = f.bind("n2", ISSUED, 2);
    assert!(report.rejected.is_empty());
    assert_eq!(report.applied.len(), 1);
    assert!(matches!(
        report.applied[0].action,
        TrustAction::BindNode { .. }
    ));
    assert_eq!(f.bound_bytes(), before);
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 2);
}
#[test]
fn h09_equal_timestamp_conflicting_node_is_stale() {
    let f = Fixture::bound();
    let before = f.bound_bytes();
    assert_stale(f.bind("n3", ISSUED, 2), 2);
    assert_eq!(f.bound_bytes(), before);
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 2);
}
#[test]
fn h10_revoke_log_deletes_binding() {
    let f = Fixture::bound();
    let report = apply(
        &f.store,
        USER,
        &[control_statement(
            pb::statement::Kind::RevokeDevice,
            f.device.fp,
            2,
        )],
    )
    .unwrap();
    assert_eq!(report.revoked, vec![f.device.fp]);
    assert!(f.nodes().is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&f.bound_bytes()).unwrap(),
        json!({"version": 1, "nodes": []})
    );
    assert_eq!(
        f.store.authorize_device(&f.device.fp).unwrap(),
        DeviceStatus::Revoked
    );
    assert!(open_store(&f.dir.0)
        .unwrap()
        .bound_nodes()
        .unwrap()
        .is_empty());
}
#[test]
fn h11_remove_deletes_binding() {
    let f = Fixture::bound();
    assert_eq!(
        f.store.remove(&f.device.fp).unwrap(),
        Some(f.device.clone())
    );
    assert!(f.nodes().is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&f.bound_bytes()).unwrap()["nodes"],
        json!([])
    );
    assert!(open_store(&f.dir.0)
        .unwrap()
        .bound_nodes()
        .unwrap()
        .is_empty());
}
#[test]
fn h12_crash_before_checkpoint_replays_idempotently() {
    let f = Fixture::new(Some("n1"));
    let trust_before = fs::read(f.dir.0.join("trust_state.json")).unwrap();
    let (new_key, new_device) = device(8, None);
    let approval = build_approve(USER, &new_device.cert_der, "new phone", ISSUED, [4; 16]).unwrap();
    let log = [
        f.entry("n2", ISSUED, 1),
        signed(&f.key, f.device.fp, &approval, 2),
        signed(
            &new_key,
            new_device.fp,
            &build_bind_node(USER, new_device.fp, "n4", ISSUED, [5; 16]).unwrap(),
            3,
        ),
        control_statement(pb::statement::Kind::RequireConfirm, new_device.fp, 4),
    ];
    let report = apply(&f.store, USER, &log).unwrap();
    assert_eq!(report.applied.len(), 4);
    assert!(report.rejected.is_empty());
    let crashed = Dir::new();
    for file in ["paired.json", "revoked.json", "bound_nodes.json"] {
        fs::copy(f.dir.0.join(file), crashed.0.join(file)).unwrap();
    }
    fs::write(crashed.0.join("trust_state.json"), trust_before).unwrap();
    let recovered = open_store(&crashed.0).unwrap();
    assert_eq!(recovered.trust_state().unwrap().last_applied_seq, 0);
    let report = apply(&recovered, USER, &log).unwrap();
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 2,
            reason: RejectReason::AlreadyPaired
        }]
    );
    assert_eq!(recovered.paired().unwrap(), f.store.paired().unwrap());
    assert_eq!(recovered.bound_nodes().unwrap(), f.nodes());
    assert_eq!(
        recovered.trust_state().unwrap(),
        f.store.trust_state().unwrap()
    );
    for file in [
        "paired.json",
        "revoked.json",
        "bound_nodes.json",
        "trust_state.json",
    ] {
        assert_eq!(
            fs::read(crashed.0.join(file)).unwrap(),
            fs::read(f.dir.0.join(file)).unwrap()
        );
    }
    drop(recovered);
    let recovered = open_store(&crashed.0).unwrap();
    let report = apply(&recovered, USER, &log).unwrap();
    assert!(report.applied.is_empty() && report.rejected.is_empty());
    assert_eq!(recovered.bound_nodes().unwrap(), f.nodes());
}
#[test]
fn h13_unknown_signer_binds_after_pairing() {
    let dir = Dir::new();
    let store = open_store(&dir.0).unwrap();
    let (key, device) = device(7, Some("n1"));
    let entry = signed(
        &key,
        device.fp,
        &build_bind_node(USER, device.fp, "n2", ISSUED, [3; 16]).unwrap(),
        1,
    );
    let report = apply(&store, USER, &[entry]).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 1,
            reason: RejectReason::UnknownSigner
        }]
    );
    assert_eq!(store.trust_state().unwrap().last_applied_seq, 1);
    store.upsert(device.clone()).unwrap();
    assert_bound(&store, &device.fp, Some("n2"));
    assert!(store.pending_binds().unwrap().is_empty());
}
#[test]
fn h14_legacy_store_materializes_empty_bound_file() {
    let f = Fixture::new(Some("n1"));
    drop(f.store);
    fs::remove_file(f.dir.0.join("bound_nodes.json")).unwrap();
    let store = open_store(&f.dir.0).unwrap();
    assert!(store.bound_nodes().unwrap().is_empty());
    assert_eq!(store.get(&f.device.fp).unwrap(), Some(f.device));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(f.dir.0.join("bound_nodes.json")).unwrap())
            .unwrap(),
        json!({"version": 1, "nodes": []})
    );
}
#[test]
fn h15_corrupt_bound_file_fails_open() {
    let dir = Dir::new();
    fs::write(dir.0.join("bound_nodes.json"), b"{bad json").unwrap();
    assert!(
        matches!(open_store(&dir.0), Err(StoreError::Corrupt { file }) if file == "bound_nodes.json")
    );
}
#[test]
fn invalid_bound_version_and_fingerprint_fail_open() {
    for json in [
        json!({"version": 2, "nodes": []}),
        json!({"version": 1, "nodes": [{"fp": "bad", "node": "n2", "issued_at_us": ISSUED, "source_seq": 1}]}),
    ] {
        let dir = Dir::new();
        fs::write(
            dir.0.join("bound_nodes.json"),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(open_store(&dir.0), Err(StoreError::Corrupt { file }) if file == "bound_nodes.json")
        );
    }
}
#[test]
fn repairing_same_fingerprint_preserves_binding() {
    let f = Fixture::bound();
    let before = f.bound_bytes();
    let mut device = f.device.clone();
    device.name = "repaired phone".into();
    device.whois_node = Some("n3".into());
    f.store.upsert(device).unwrap();
    assert_bound(&f.store, &f.device.fp, Some("n2"));
    assert_eq!(f.bound_bytes(), before);
}
#[test]
fn direct_store_revoke_deletes_binding() {
    let f = Fixture::bound();
    f.store.revoke(f.device.fp, 2).unwrap();
    assert!(f.nodes().is_empty());
    assert!(open_store(&f.dir.0)
        .unwrap()
        .bound_nodes()
        .unwrap()
        .is_empty());
}
#[test]
fn stale_rejections_are_sorted_with_verification_rejections() {
    let f = Fixture::bound();
    let mut invalid = f.entry("n4", ISSUED + 1, 3);
    invalid.signature.clear();
    let report = apply(
        &f.store,
        USER,
        &[
            invalid,
            f.entry("n3", ISSUED - 1, 2),
            f.entry("n5", ISSUED, 4),
        ],
    )
    .unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(
        report.rejected,
        vec![
            Rejected {
                seq: 2,
                reason: RejectReason::StaleBinding
            },
            Rejected {
                seq: 3,
                reason: RejectReason::MissingSignature
            },
            Rejected {
                seq: 4,
                reason: RejectReason::StaleBinding
            },
        ]
    );
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 4);
}
#[test]
fn failed_bound_write_rolls_back_memory_and_replays_on_reopen() {
    let f = Fixture::bound();
    let before = f.nodes();
    let temp = f.dir.0.join(".bound_nodes.json.tmp");
    fs::create_dir(&temp).unwrap();
    let entry = f.entry("n3", ISSUED + 1, 2);
    assert!(apply(&f.store, USER, std::slice::from_ref(&entry)).is_err());
    assert_eq!(f.nodes(), before);
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 1);
    fs::remove_dir(temp).unwrap();
    drop(f.store);
    let store = open_store(&f.dir.0).unwrap();
    assert!(apply(&store, USER, &[entry]).unwrap().rejected.is_empty());
    assert_bound(&store, &f.device.fp, Some("n3"));
}
#[test]
fn open_cleans_bound_temp_and_tightens_permissions() {
    let f = Fixture::bound();
    let temp = f.dir.0.join(".bound_nodes.json.tmp");
    fs::write(&temp, b"partial write").unwrap();
    let path = f.dir.0.join("bound_nodes.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    drop(f.store);
    let store = open_store(&f.dir.0).unwrap();
    assert_bound(&store, &f.device.fp, Some("n2"));
    assert!(!temp.exists());
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn assert_redacted(text: &str, entries: &[rsp_core::trust::TrustEntry]) {
    assert!(!text.contains("n2"), "StableID leaked: {text}");
    for entry in entries {
        let statement = pb::Statement::decode(entry.statement.as_slice()).unwrap();
        for bytes in [&entry.statement, &entry.signature, &statement.nonce] {
            for encoding in [
                data_encoding::HEXLOWER,
                data_encoding::BASE64,
                data_encoding::BASE64URL_NOPAD,
            ] {
                assert!(
                    !text.contains(&encoding.encode(bytes)),
                    "sensitive bytes leaked: {text}"
                );
            }
        }
    }
}

fn unpaired_fixture() -> Fixture {
    let dir = Dir::new();
    let store = Arc::new(open_store(&dir.0).unwrap());
    let (key, device) = device(7, Some("n1"));
    Fixture {
        store,
        key,
        device,
        dir,
    }
}

fn pending_entry(
    key: &SigningKey,
    device: &PairedDevice,
    node: &str,
    issued: u64,
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    signed(
        key,
        device.fp,
        &build_bind_node(USER, device.fp, node, issued, [3; 16]).unwrap(),
        seq,
    )
}

fn assert_pending(store: &Store, entry: &rsp_core::trust::TrustEntry) {
    let statement = pb::Statement::decode(entry.statement.as_slice()).unwrap();
    let pending = store.pending_binds().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].fp.as_slice(), entry.signer_fp);
    assert_eq!(pending[0].signed, entry.encode_to_vec());
    assert_eq!(pending[0].issued_at_us, statement.issued_at_us);
    assert_eq!(pending[0].source_seq, entry.seq);
}

#[test]
fn p01_pending_binding_avoids_node_mismatch_after_pairing() {
    let f = unpaired_fixture();
    let entry = f.entry("n2", ISSUED, 1);
    let report = apply(&f.store, USER, std::slice::from_ref(&entry)).unwrap();
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 1,
            reason: RejectReason::UnknownSigner
        }]
    );
    assert_pending(&f.store, &entry);
    assert!(f.nodes().is_empty());
    assert_eq!(
        f.store.authorize_device(&f.device.fp).unwrap(),
        DeviceStatus::Unknown
    );
    f.store.upsert(f.device.clone()).unwrap();
    f.assert_mismatch("n2", false);
    f.assert_mismatch("n1", false);
    f.assert_mismatch("n3", true);
    assert!(f.store.pending_binds().unwrap().is_empty());
    assert_eq!(f.nodes()[0].source_seq, 1);
}

#[test]
fn p02_later_approval_verifies_pending_binding() {
    let f = Fixture::new(Some("n1"));
    let (key, device) = device(8, Some("n3"));
    let entry = pending_entry(&key, &device, "n2", ISSUED, 1);
    let report = apply(&f.store, USER, std::slice::from_ref(&entry)).unwrap();
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 1,
            reason: RejectReason::UnknownSigner
        }]
    );
    assert_pending(&f.store, &entry);
    let approval =
        build_approve(USER, &device.cert_der, "approved phone", ISSUED, [4; 16]).unwrap();
    let report = apply(&f.store, USER, &[signed(&f.key, f.device.fp, &approval, 2)]).unwrap();
    assert!(report.rejected.is_empty());
    assert_eq!(report.applied.len(), 1);
    let paired = f.store.get(&device.fp).unwrap().unwrap();
    assert_eq!(paired.cert_der, device.cert_der);
    assert_eq!(paired.permission, Permission::Control);
    assert_bound(&f.store, &device.fp, Some("n2"));
    assert!(f.store.pending_binds().unwrap().is_empty());
    assert_eq!(f.nodes()[0].source_seq, 1);
    assert_bound(&open_store(&f.dir.0).unwrap(), &device.fp, Some("n2"));
}

#[test]
fn p03_same_batch_approval_verifies_pending_and_keeps_newest_binding() {
    // Also exercise pending versus an immediately verified binding later in
    // the batch: older, newer, equal/conflicting and equal/identical claims.
    for later in [
        None,
        Some((ISSUED + 1, "n3", "n3", 3)),
        Some((ISSUED - 1, "n3", "n2", 1)),
        Some((ISSUED, "n3", "n3", 3)),
        Some((ISSUED, "n2", "n2", 3)),
    ] {
        let f = Fixture::new(Some("n1"));
        let (key, device) = device(8, None);
        let binding = pending_entry(&key, &device, "n2", ISSUED, 1);
        let approval =
            build_approve(USER, &device.cert_der, "approved phone", ISSUED, [4; 16]).unwrap();
        let mut log = vec![signed(&f.key, f.device.fp, &approval, 2), binding];
        let (expected_node, expected_seq) =
            if let Some((issued, node, expected_node, expected_seq)) = later {
                log.push(pending_entry(&key, &device, node, issued, 3));
                (expected_node, expected_seq)
            } else {
                ("n2", 1)
            };
        let report = apply(&f.store, USER, &log).unwrap();
        assert_eq!(
            report.rejected,
            vec![Rejected {
                seq: 1,
                reason: RejectReason::UnknownSigner
            }]
        );
        assert_eq!(report.applied.len(), log.len() - 1);
        assert_bound(&f.store, &device.fp, Some(expected_node));
        assert!(f.store.pending_binds().unwrap().is_empty());
        assert_eq!(f.nodes()[0].source_seq, expected_seq);
        assert_eq!(
            f.store.trust_state().unwrap().last_applied_seq,
            log.len() as u64
        );
    }
}

#[test]
fn p04_forged_or_invalid_pending_claim_does_not_fail_pairing() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _capture = tracing::subscriber::set_default(subscriber);
    for invalid in ["signature", "cert", "subject_cert", "subject_name", "valid"] {
        let f = unpaired_fixture();
        let (other_key, _) = device(8, None);
        let mut statement = build_bind_node(USER, f.device.fp, "n2", ISSUED, [3; 16]).unwrap();
        if invalid == "subject_cert" {
            statement.subject_cert = vec![1];
        }
        if invalid == "subject_name" {
            statement.subject_name = "phone".into();
        }
        let key = if invalid == "signature" {
            &other_key
        } else {
            &f.key
        };
        let entry = signed(key, f.device.fp, &statement, 1);
        apply(&f.store, USER, std::slice::from_ref(&entry)).unwrap();
        assert_pending(&f.store, &entry);
        let mut paired = f.device.clone();
        if invalid == "cert" {
            paired.cert_der = vec![1, 2, 3];
        }
        assert!(f.store.upsert(paired.clone()).is_ok());
        assert_eq!(f.store.get(&paired.fp).unwrap(), Some(paired));
        assert_bound(
            &f.store,
            &f.device.fp,
            if invalid == "valid" { Some("n2") } else { None },
        );
        assert!(f.store.pending_binds().unwrap().is_empty());
        let logs = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert_redacted(&logs, &[entry]);
    }
    let logs = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("WARN") && logs.contains("InvalidSignature"),
        "{logs}"
    );
    assert!(
        logs.contains("INFO") && logs.contains("source_seq=1"),
        "{logs}"
    );
}

#[test]
fn p05_pending_keeps_latest_timestamp_and_first_on_ties() {
    for issued in [[1, 2], [2, 1]] {
        let f = unpaired_fixture();
        for (index, issued) in issued.into_iter().enumerate() {
            let node = if issued == 1 { "n2" } else { "n3" };
            f.bind(node, issued, index as u64 + 1);
        }
        let expected_seq = if issued[0] == 2 { 1 } else { 2 };
        let expected = f.entry("n3", 2, expected_seq);
        assert_pending(&f.store, &expected);
        // Equal timestamps, including conflicting nodes, retain the original bytes.
        f.bind("n4", 2, 3);
        f.bind("n3", 2, 4);
        assert_pending(&f.store, &expected);
        f.store.upsert(f.device.clone()).unwrap();
        assert_bound(&f.store, &f.device.fp, Some("n3"));
        assert_eq!(f.nodes()[0].source_seq, expected_seq);
        assert!(f.store.pending_binds().unwrap().is_empty());
    }
}

#[test]
fn p06_wrong_user_is_rejected_without_pending() {
    let f = unpaired_fixture();
    let statement = build_bind_node("other@", f.device.fp, "n2", ISSUED, [3; 16]).unwrap();
    let report = apply(
        &f.store,
        USER,
        &[signed(&f.key, f.device.fp, &statement, 1)],
    )
    .unwrap();
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 1,
            reason: RejectReason::UserMismatch
        }]
    );
    assert!(f.store.pending_binds().unwrap().is_empty());
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 1);
}

#[test]
fn p07_ineligible_rejections_are_never_pending() {
    let f = unpaired_fixture();
    let mut hub_binding = f.entry("n2", ISSUED, 1);
    hub_binding.signer_fp.clear();
    let mut other_signer = f.entry("n2", ISSUED, 2);
    other_signer.signer_fp = vec![8; 32];
    let mut missing_signature = f.entry("n2", ISSUED, 3);
    missing_signature.signature.clear();
    let mut bad_node = build_bind_node(USER, f.device.fp, "n2", ISSUED, [3; 16]).unwrap();
    bad_node.subject_node = "a/b".into();
    let approval = build_approve(USER, &f.device.cert_der, "phone", ISSUED, [4; 16]).unwrap();
    let report = apply(
        &f.store,
        USER,
        &[
            hub_binding,
            other_signer,
            missing_signature,
            signed(&f.key, f.device.fp, &bad_node, 4),
            signed(&f.key, f.device.fp, &approval, 5),
        ],
    )
    .unwrap();
    assert_eq!(
        report.rejected,
        vec![
            Rejected {
                seq: 1,
                reason: RejectReason::MissingSigner
            },
            Rejected {
                seq: 2,
                reason: RejectReason::BindSignerMismatch
            },
            Rejected {
                seq: 3,
                reason: RejectReason::MissingSignature
            },
            Rejected {
                seq: 4,
                reason: RejectReason::UnknownSigner
            },
            Rejected {
                seq: 5,
                reason: RejectReason::UnknownSigner
            },
        ]
    );
    assert!(f.store.pending_binds().unwrap().is_empty());
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 5);
    // A duplicate sequence that apply_log skipped must not replace its rejected entry.
    let mut invalid_first = f.entry("n2", ISSUED, 6);
    invalid_first.statement = bad_node.encode_to_vec();
    apply(
        &f.store,
        USER,
        &[invalid_first, f.entry("n3", ISSUED + 1, 6)],
    )
    .unwrap();
    assert!(f.store.pending_binds().unwrap().is_empty());
}

#[test]
fn p08_pending_capacity_evicts_lowest_source_sequences() {
    let f = unpaired_fixture();
    let log: Vec<_> = (1_u64..=300)
        .rev()
        .map(|seq| {
            let mut fp = [0; 32];
            fp[..8].copy_from_slice(&seq.to_be_bytes());
            let statement = build_bind_node(USER, fp, "n2", ISSUED, [3; 16]).unwrap();
            signed(&f.key, fp, &statement, seq)
        })
        .collect();
    let report = apply(&f.store, USER, &log).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(report.rejected.len(), 300);
    assert!(report
        .rejected
        .iter()
        .all(|r| r.reason == RejectReason::UnknownSigner));
    let pending = f.store.pending_binds().unwrap();
    assert_eq!(pending.len(), 256);
    let mut seqs: Vec<_> = pending.iter().map(|p| p.source_seq).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, (45..=300).collect::<Vec<_>>());
    assert_eq!(f.store.trust_state().unwrap().last_applied_seq, 300);
    assert_eq!(
        open_store(&f.dir.0).unwrap().pending_binds().unwrap(),
        pending
    );
}

#[test]
fn p09_revoke_or_remove_clears_unpaired_pending() {
    for mode in [
        "later_batch",
        "same_batch",
        "after_approve",
        "direct",
        "remove",
    ] {
        let f = unpaired_fixture();
        let binding = f.entry("n2", ISSUED, 1);
        let revoke = control_statement(pb::statement::Kind::RevokeDevice, f.device.fp, 2);
        if mode == "same_batch" {
            apply(&f.store, USER, &[binding, revoke]).unwrap();
        } else if mode == "after_approve" {
            let (control_key, control) = device(8, None);
            f.store.upsert(control.clone()).unwrap();
            let approval =
                build_approve(USER, &f.device.cert_der, "phone", ISSUED, [4; 16]).unwrap();
            apply(
                &f.store,
                USER,
                &[
                    binding,
                    signed(&control_key, control.fp, &approval, 2),
                    control_statement(pb::statement::Kind::RevokeDevice, f.device.fp, 3),
                ],
            )
            .unwrap();
        } else {
            apply(&f.store, USER, &[binding]).unwrap();
            assert_eq!(f.store.pending_binds().unwrap().len(), 1);
            match mode {
                "later_batch" => {
                    apply(&f.store, USER, &[revoke]).unwrap();
                }
                "direct" => f.store.revoke(f.device.fp, 2).unwrap(),
                "remove" => assert_eq!(f.store.remove(&f.device.fp).unwrap(), None),
                _ => unreachable!(),
            }
        }
        assert!(f.store.pending_binds().unwrap().is_empty());
        assert!(f.nodes().is_empty());
        let reopened = open_store(&f.dir.0).unwrap();
        assert!(reopened.pending_binds().unwrap().is_empty());
        if mode == "remove" {
            reopened.upsert(f.device.clone()).unwrap();
            assert_bound(&reopened, &f.device.fp, None);
        } else {
            assert!(matches!(
                f.store.upsert(f.device.clone()),
                Err(StoreError::Revoked)
            ));
            assert!(matches!(
                reopened.upsert(f.device.clone()),
                Err(StoreError::Revoked)
            ));
        }
    }
}

#[test]
fn p10_pending_persists_and_legacy_or_corrupt_files_are_handled() {
    let f = unpaired_fixture();
    let mut entry = f.entry("n2", ISSUED, 1);
    // Preserve signed protobuf bytes, including an unknown Statement field.
    entry.statement.extend_from_slice(&[0x98, 0x06, 0x01]);
    let signature: Signature = f
        .key
        .sign(&rsp_core::trust::signing_payload(&entry.statement));
    entry.signature = signature.to_der().as_bytes().to_vec();
    apply(&f.store, USER, std::slice::from_ref(&entry)).unwrap();
    let bytes = f.bound_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json,
        json!({"version": 1, "nodes": [], "pending": [{
            "fp": fp_b64url(&f.device.fp), "issued_at_us": ISSUED, "source_seq": 1,
            "signed": data_encoding::BASE64URL_NOPAD.encode(&entry.encode_to_vec())
        }]})
    );
    let store = open_store(&f.dir.0).unwrap();
    assert_pending(&store, &entry);
    assert_eq!(f.bound_bytes(), bytes);
    store.upsert(f.device.clone()).unwrap();
    assert_bound(&store, &f.device.fp, Some("n2"));
    assert!(store.pending_binds().unwrap().is_empty());
    drop(store);
    // H21 files have no pending key, including files with an existing binding.
    let mut legacy: Value = serde_json::from_slice(&f.bound_bytes()).unwrap();
    legacy.as_object_mut().unwrap().remove("pending");
    fs::write(
        f.dir.0.join("bound_nodes.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let store = open_store(&f.dir.0).unwrap();
    assert!(store.pending_binds().unwrap().is_empty());
    assert_bound(&store, &f.device.fp, Some("n2"));
    for (field, bad) in [
        ("fp", json!("bad")),
        ("signed", json!("!")),
        ("signed", json!("AA")),
        ("issued_at_us", json!(0)),
        ("source_seq", json!(0)),
    ] {
        let dir = Dir::new();
        let mut corrupt = json.clone();
        corrupt["pending"][0][field] = bad;
        fs::write(
            dir.0.join("bound_nodes.json"),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(open_store(&dir.0), Err(StoreError::Corrupt { file }) if file == "bound_nodes.json")
        );
    }
}

#[test]
fn p11_pending_crash_replay_converges_without_duplicates_or_loss() {
    let f = unpaired_fixture();
    let old_trust = fs::read(f.dir.0.join("trust_state.json")).unwrap();
    let entry = f.entry("n2", ISSUED, 1);
    apply(&f.store, USER, std::slice::from_ref(&entry)).unwrap();
    let pending_bytes = f.bound_bytes();
    let crashed = Dir::new();
    for file in ["paired.json", "revoked.json", "bound_nodes.json"] {
        fs::copy(f.dir.0.join(file), crashed.0.join(file)).unwrap();
    }
    fs::write(crashed.0.join("trust_state.json"), old_trust).unwrap();
    let recovered = open_store(&crashed.0).unwrap();
    assert_eq!(recovered.trust_state().unwrap().last_applied_seq, 0);
    assert_pending(&recovered, &entry);
    apply(&recovered, USER, std::slice::from_ref(&entry)).unwrap();
    assert_eq!(
        recovered.pending_binds().unwrap(),
        f.store.pending_binds().unwrap()
    );
    assert_eq!(recovered.bound_nodes().unwrap(), f.nodes());
    assert_eq!(
        recovered.trust_state().unwrap(),
        f.store.trust_state().unwrap()
    );
    assert_eq!(
        fs::read(crashed.0.join("bound_nodes.json")).unwrap(),
        pending_bytes
    );
    f.store.upsert(f.device.clone()).unwrap();
    recovered.upsert(f.device.clone()).unwrap();
    assert!(recovered.pending_binds().unwrap().is_empty());
    assert_eq!(recovered.bound_nodes().unwrap(), f.nodes());
    assert_bound(&recovered, &f.device.fp, Some("n2"));
    let report = apply(&recovered, USER, &[entry]).unwrap();
    assert!(report.applied.is_empty() && report.rejected.is_empty());
    for file in [
        "paired.json",
        "revoked.json",
        "bound_nodes.json",
        "trust_state.json",
    ] {
        assert_eq!(
            fs::read(crashed.0.join(file)).unwrap(),
            fs::read(f.dir.0.join(file)).unwrap()
        );
    }
    // A local pairing crash after paired.json but before bound_nodes.json
    // must also finish verification on reopen.
    fs::write(crashed.0.join("bound_nodes.json"), &pending_bytes).unwrap();
    drop(recovered);
    let recovered = open_store(&crashed.0).unwrap();
    assert!(recovered.pending_binds().unwrap().is_empty());
    assert_eq!(recovered.bound_nodes().unwrap(), f.nodes());

    // Likewise, a batch approval partially persisted before its checkpoint.
    let control = Fixture::new(None);
    let (key, device) = device(8, None);
    let entry = pending_entry(&key, &device, "n2", ISSUED, 1);
    apply(&control.store, USER, &[entry]).unwrap();
    let old_pending = control.bound_bytes();
    let old_trust = fs::read(control.dir.0.join("trust_state.json")).unwrap();
    let approval = build_approve(USER, &device.cert_der, "phone", ISSUED, [4; 16]).unwrap();
    let approval = signed(&control.key, control.device.fp, &approval, 2);
    apply(&control.store, USER, std::slice::from_ref(&approval)).unwrap();
    let expected_nodes = control.nodes();
    let expected_trust = control.store.trust_state().unwrap();
    fs::write(control.dir.0.join("bound_nodes.json"), old_pending).unwrap();
    fs::write(control.dir.0.join("trust_state.json"), old_trust).unwrap();
    drop(control.store);
    let recovered = open_store(&control.dir.0).unwrap();
    let report = apply(&recovered, USER, &[approval]).unwrap();
    assert_eq!(
        report.rejected,
        vec![Rejected {
            seq: 2,
            reason: RejectReason::AlreadyPaired
        }]
    );
    assert!(recovered.pending_binds().unwrap().is_empty());
    assert_eq!(recovered.bound_nodes().unwrap(), expected_nodes);
    assert_eq!(recovered.trust_state().unwrap(), expected_trust);
}
