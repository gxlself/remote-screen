use std::convert::Infallible;

use host_core::store::{DeviceStatus, PairedDevice, Store, TrustState};
use host_core::trust::apply;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use rsp_core::ids::id_from_fp;
use rsp_core::pb;
use rsp_core::trust::{build_statement, sign_with, Permission, RejectReason};

fn dir() -> std::path::PathBuf {
    let root = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = root.join(format!("rsp-host-trust-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn control_statement(
    kind: pb::statement::Kind,
    fp: [u8; 32],
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    control_for("alice", kind, fp, seq)
}
fn control_for(
    user: &str,
    kind: pb::statement::Kind,
    fp: [u8; 32],
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let (key, issuer) = journal_issuer();
    let statement = build_statement(kind, user, fp, [], "phone", 1, [1; 16]);
    let signed = sign_with(&statement, issuer.fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .unwrap();
    rsp_core::trust::TrustEntry { seq, signed }
}
fn journal_issuer() -> (SigningKey, PairedDevice) {
    use p256::pkcs8::EncodePrivateKey;
    let key = SigningKey::from_bytes((&[6; 32]).into()).unwrap();
    let native = rcgen::KeyPair::try_from(key.to_pkcs8_der().unwrap().as_bytes()).unwrap();
    let cert_der = rcgen::CertificateParams::new(vec!["journal-issuer".into()])
        .unwrap()
        .self_signed(&native)
        .unwrap()
        .der()
        .to_vec();
    let (_, mut issuer) = device(Permission::Control);
    issuer.cert_der = cert_der;
    issuer.fp = rsp_core::trust::subject_fingerprint(&issuer.cert_der).unwrap();
    issuer.device_id = id_from_fp(&issuer.fp);
    (key, issuer)
}
fn open_store(path: &std::path::Path) -> Result<Store, host_core::store::StoreError> {
    let store = Store::open(path)?;
    let (_, issuer) = journal_issuer();
    if store.get(&issuer.fp)?.is_none() {
        store.upsert(issuer)?;
    }
    Ok(store)
}

#[test]
fn foreign_revoke_is_persisted_and_blocks_later_pairing() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let fp = [8; 32];

    let report = apply(
        &store,
        "alice",
        &[control_for("bob", pb::statement::Kind::RevokeDevice, fp, 1)],
    )
    .unwrap();
    assert_eq!(report.revoked, vec![fp]);
    assert_eq!(store.trust_state().unwrap().last_applied_seq, 1);

    let result = store.upsert(PairedDevice {
        fp,
        device_id: id_from_fp(&fp),
        name: "foreign phone".into(),
        cert_der: Vec::new(),
        permission: Permission::ViewOnly,
        paired_at_us: 1,
        require_confirm: false,
        platform: Default::default(),
        paired_via: Default::default(),
        whois_user: None,
        whois_node: None,
        whois_node_name: None,
        path_permissions: Default::default(),
    });
    assert!(matches!(result, Err(host_core::store::StoreError::Revoked)));
    let _ = std::fs::remove_dir_all(path);
}

fn device(permission: Permission) -> (SigningKey, PairedDevice) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let cert_der = cert.der().to_vec();
    let fp = rsp_transport::identity::spki_fingerprint(&cert_der).unwrap();
    (
        SigningKey::from_pkcs8_der(&key.serialize_der()).unwrap(),
        PairedDevice {
            fp,
            device_id: id_from_fp(&fp),
            name: "original phone".into(),
            cert_der,
            permission,
            paired_at_us: 42,
            require_confirm: false,
            platform: Default::default(),
            paired_via: Default::default(),
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: Default::default(),
        },
    )
}

fn approve(
    key: &SigningKey,
    signer_fp: [u8; 32],
    subject: &PairedDevice,
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let statement = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject.fp,
        &subject.cert_der,
        "replacement phone",
        1,
        [1; 16],
    );
    let signed = sign_with(&statement, signer_fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .unwrap();
    rsp_core::trust::TrustEntry { seq, signed }
}

#[test]
fn apply_revoke_and_require_confirm_updates_store() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let fp = [4; 32];
    store
        .upsert(PairedDevice {
            fp,
            device_id: id_from_fp(&fp),
            name: "phone".into(),
            cert_der: Vec::new(),
            permission: Permission::Control,
            paired_at_us: 1,
            require_confirm: false,
            platform: Default::default(),
            paired_via: Default::default(),
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: Default::default(),
        })
        .unwrap();
    let report = apply(
        &store,
        "alice",
        &[control_statement(
            pb::statement::Kind::RequireConfirm,
            fp,
            1,
        )],
    )
    .unwrap();
    assert_eq!(report.applied.len(), 1);
    assert!(store.get(&fp).unwrap().unwrap().require_confirm);
    let report = apply(
        &store,
        "alice",
        &[control_statement(pb::statement::Kind::RevokeDevice, fp, 2)],
    )
    .unwrap();
    assert_eq!(report.revoked, vec![fp]);
    assert!(store.get(&fp).unwrap().is_none());
    assert_eq!(
        store.trust_state().unwrap(),
        TrustState {
            last_applied_seq: 2,
            last_trust_sync_at_us: 0
        }
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn gap_leaves_state_unchanged() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let before = store.trust_state().unwrap();
    let result = apply(
        &store,
        "alice",
        &[control_statement(
            pb::statement::Kind::RevokeDevice,
            [3; 32],
            2,
        )],
    );
    assert!(result.is_err());
    assert_eq!(store.trust_state().unwrap(), before);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn later_batch_approval_cannot_restore_revoked_subject() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let (key, signer) = device(Permission::Control);
    let (_, subject) = device(Permission::ViewOnly);
    store.upsert(signer.clone()).unwrap();
    store.upsert(subject.clone()).unwrap();
    apply(
        &store,
        "alice",
        &[control_statement(
            pb::statement::Kind::RevokeDevice,
            subject.fp,
            1,
        )],
    )
    .unwrap();
    drop(store);

    let store = open_store(&path).unwrap();
    let revoked_before = std::fs::read(path.join("revoked.json")).unwrap();
    let report = apply(&store, "alice", &[approve(&key, signer.fp, &subject, 2)]).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].seq, 2);
    assert_eq!(report.rejected[0].reason, RejectReason::SubjectRevoked);
    assert_eq!(
        store.authorize_device(&subject.fp).unwrap(),
        DeviceStatus::Revoked
    );
    assert!(store.get(&subject.fp).unwrap().is_none());
    assert_eq!(store.trust_state().unwrap().last_applied_seq, 2);
    assert_eq!(
        std::fs::read(path.join("revoked.json")).unwrap(),
        revoked_before
    );
    drop(store);
    let store = open_store(&path).unwrap();
    assert_eq!(
        store.authorize_device(&subject.fp).unwrap(),
        DeviceStatus::Revoked
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn same_batch_approval_cannot_restore_revoked_subject() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let (key, signer) = device(Permission::Control);
    let (_, subject) = device(Permission::ViewOnly);
    store.upsert(signer.clone()).unwrap();
    store.upsert(subject.clone()).unwrap();
    let report = apply(
        &store,
        "alice",
        &[
            approve(&key, signer.fp, &subject, 2),
            control_statement(pb::statement::Kind::RevokeDevice, subject.fp, 1),
        ],
    )
    .unwrap();
    assert_eq!(report.revoked, vec![subject.fp]);
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].seq, 2);
    assert_eq!(report.rejected[0].reason, RejectReason::SubjectRevoked);
    assert_eq!(
        store.authorize_device(&subject.fp).unwrap(),
        DeviceStatus::Revoked
    );
    assert!(store.get(&subject.fp).unwrap().is_none());
    assert_eq!(store.trust_state().unwrap().last_applied_seq, 2);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn approving_paired_view_only_device_keeps_all_pairing_fields() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let (key, signer) = device(Permission::Control);
    let (_, mut subject) = device(Permission::ViewOnly);
    subject.require_confirm = true;
    store.upsert(signer.clone()).unwrap();
    store.upsert(subject.clone()).unwrap();
    let paired_before = std::fs::read(path.join("paired.json")).unwrap();
    let report = apply(&store, "alice", &[approve(&key, signer.fp, &subject, 1)]).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].reason, RejectReason::AlreadyPaired);
    assert_eq!(store.get(&subject.fp).unwrap(), Some(subject.clone()));
    assert_eq!(
        store.authorize_device(&subject.fp).unwrap(),
        DeviceStatus::Paired {
            permission: Permission::ViewOnly,
            require_confirm: true,
            whois_user: None,
            whois_node: None,
            bound_node: None,
            path_permissions: Default::default(),
        }
    );
    assert_eq!(
        std::fs::read(path.join("paired.json")).unwrap(),
        paired_before
    );
    drop(store);
    let store = open_store(&path).unwrap();
    assert_eq!(store.get(&subject.fp).unwrap(), Some(subject));
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn replay_after_paired_and_revoked_persisted_converges_to_successful_apply() {
    let path = dir();
    let store = open_store(&path).unwrap();
    let (key, signer) = device(Permission::Control);
    let (_, revoked) = device(Permission::ViewOnly);
    let (_, approved) = device(Permission::ViewOnly);
    store.upsert(signer.clone()).unwrap();
    store.upsert(revoked.clone()).unwrap();
    store
        .set_trust_state(TrustState {
            last_applied_seq: 10,
            last_trust_sync_at_us: 9,
        })
        .unwrap();
    let trust_before = std::fs::read(path.join("trust_state.json")).unwrap();
    let log = [
        approve(&key, signer.fp, &approved, 11),
        control_statement(pb::statement::Kind::RequireConfirm, approved.fp, 12),
        control_statement(pb::statement::Kind::RevokeDevice, revoked.fp, 13),
        control_statement(pb::statement::Kind::RequireConfirm, [3; 32], 14),
        control_statement(pb::statement::Kind::RequireConfirm, revoked.fp, 15),
        approve(&key, signer.fp, &revoked, 16),
    ];
    let report = apply(&store, "alice", &log).unwrap();
    assert_eq!(report.applied.len(), 3);
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].reason, RejectReason::SubjectRevoked);
    let expected_paired = store.paired().unwrap();
    let expected_trust = store.trust_state().unwrap();
    let paired_bytes = std::fs::read(path.join("paired.json")).unwrap();
    let revoked_bytes = std::fs::read(path.join("revoked.json")).unwrap();
    let trust_bytes = std::fs::read(path.join("trust_state.json")).unwrap();
    drop(store);

    // Construct the on-disk state at a crash between revoked.json and the final
    // trust_state.json checkpoint: effects are durable, but seq is still ten.
    let crashed_path = dir();
    std::fs::create_dir_all(&crashed_path).unwrap();
    std::fs::write(crashed_path.join("paired.json"), &paired_bytes).unwrap();
    std::fs::write(crashed_path.join("revoked.json"), &revoked_bytes).unwrap();
    std::fs::write(crashed_path.join("trust_state.json"), trust_before).unwrap();
    let recovered = open_store(&crashed_path).unwrap();
    assert_eq!(recovered.trust_state().unwrap().last_applied_seq, 10);
    let replay = apply(&recovered, "alice", &log).unwrap();
    assert_eq!(replay.rejected[0].reason, RejectReason::AlreadyPaired);
    assert_eq!(recovered.paired().unwrap(), expected_paired);
    assert_eq!(recovered.trust_state().unwrap(), expected_trust);
    assert_eq!(
        recovered.authorize_device(&revoked.fp).unwrap(),
        DeviceStatus::Revoked
    );
    assert_eq!(
        std::fs::read(crashed_path.join("paired.json")).unwrap(),
        paired_bytes
    );
    assert_eq!(
        std::fs::read(crashed_path.join("revoked.json")).unwrap(),
        revoked_bytes
    );
    assert_eq!(
        std::fs::read(crashed_path.join("trust_state.json")).unwrap(),
        trust_bytes
    );
    drop(recovered);
    let recovered = open_store(&crashed_path).unwrap();
    let report = apply(&recovered, "alice", &log).unwrap();
    assert!(report.applied.is_empty());
    assert!(report.rejected.is_empty());
    assert_eq!(recovered.paired().unwrap(), expected_paired);
    assert_eq!(
        recovered.authorize_device(&revoked.fp).unwrap(),
        DeviceStatus::Revoked
    );
    let _ = std::fs::remove_dir_all(path);
    let _ = std::fs::remove_dir_all(crashed_path);
}
