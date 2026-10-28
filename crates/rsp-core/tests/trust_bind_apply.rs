//! BIND_NODE execution, rejection precedence and approval-batch compatibility.
use std::{collections::HashMap, convert::Infallible};

use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::{EncodePrivateKey, EncodePublicKey},
};
use prost::Message;
use rsp_core::{
    pb,
    trust::{
        apply_log, build_approve, build_bind_node, build_statement, sign_with, subject_fingerprint,
        Applied, DeviceView, KnownDevice, Permission, RejectReason, Rejected, TrustAction,
    },
};

const USER: &str = "vector@example";
const FP: [u8; 32] = [7; 32];
const ISSUED: u64 = 1_000_000;

#[derive(Default)]
struct View(HashMap<[u8; 32], KnownDevice>);
impl DeviceView for View {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice> {
        self.0.get(fp).cloned()
    }
}
fn signing_key(byte: u8) -> SigningKey {
    // Test-only fixed key material, never used outside these tests.
    SigningKey::from_bytes((&[byte; 32]).into()).unwrap()
}
fn public_key_der(key: &SigningKey) -> Vec<u8> {
    key.verifying_key()
        .to_public_key_der()
        .unwrap()
        .as_bytes()
        .to_vec()
}
fn view(permission: Permission) -> View {
    View(HashMap::from([(
        FP,
        KnownDevice {
            spki_der: public_key_der(&signing_key(7)),
            permission,
            revoked: false,
        },
    )]))
}
fn statement() -> pb::Statement {
    build_bind_node(USER, FP, "n2", ISSUED, [3; 16]).unwrap()
}
fn signed(
    statement: &pb::Statement,
    fp: [u8; 32],
    byte: u8,
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let signed = sign_with(statement, fp, |payload| {
        let signature: Signature = signing_key(byte).sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .unwrap();
    rsp_core::trust::TrustEntry { seq, signed }
}
fn bind(seq: u64) -> rsp_core::trust::TrustEntry {
    signed(&statement(), FP, 7, seq)
}
fn assert_rejected(view: &View, entry: rsp_core::trust::TrustEntry, reason: RejectReason) {
    let seq = entry.seq;
    let (applied, rejected) = apply_log(view, 0, USER, &[entry]).unwrap();
    assert!(applied.is_empty());
    assert_eq!(rejected, vec![Rejected { seq, reason }]);
}
fn assert_bound(view: &View) {
    let (applied, rejected) = apply_log(view, 0, USER, &[bind(1)]).unwrap();
    assert!(rejected.is_empty());
    assert_eq!(
        applied,
        vec![Applied {
            seq: 1,
            action: TrustAction::BindNode {
                fp: FP,
                node: "n2".into(),
                issued_at_us: ISSUED
            },
        }]
    );
}

#[test]
fn a01_control_device_self_signs() {
    assert_bound(&view(Permission::Control));
}
#[test]
fn a02_contract_vector_view_only() {
    let vector: serde_json::Value =
        serde_json::from_str(include_str!("vectors/trust/bind_node.json")).unwrap();
    let cert = data_encoding::HEXLOWER
        .decode(vector["device_cert_der_hex"].as_str().unwrap().as_bytes())
        .unwrap();
    let (_, parsed) = x509_parser::parse_x509_certificate(&cert).unwrap();
    let fp = subject_fingerprint(&cert).unwrap();
    let view = View(HashMap::from([(
        fp,
        KnownDevice {
            spki_der: parsed.public_key().raw.to_vec(),
            permission: Permission::ViewOnly,
            revoked: false,
        },
    )]));
    let entry =
        pb::SignedStatement::decode(include_bytes!("vectors/trust/bind_node.bin").as_slice())
            .unwrap();
    let encoded = entry.encode_to_vec();
    let entry = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: pb::SignedStatement::decode(encoded.as_slice()).unwrap(),
    };
    let (applied, rejected) = apply_log(&view, 0, USER, &[entry]).unwrap();
    assert!(rejected.is_empty());
    assert_eq!(
        applied,
        vec![Applied {
            seq: 1,
            action: TrustAction::BindNode {
                fp,
                node: "nK7m2q4xCNTRL".into(),
                issued_at_us: vector["issued_at_us"].as_u64().unwrap(),
            },
        }]
    );
}
#[test]
fn a03_view_only_device_self_signs() {
    assert_bound(&view(Permission::ViewOnly));
}
#[test]
fn a04_unknown_signer() {
    assert_rejected(&View::default(), bind(1), RejectReason::UnknownSigner);
}
#[test]
fn a05_revoked_signer() {
    let mut view = view(Permission::Control);
    view.0.get_mut(&FP).unwrap().revoked = true;
    assert_rejected(&view, bind(1), RejectReason::SignerRevoked);
}
#[test]
fn a06_revoked_earlier_in_batch() {
    let revoke = signed(
        &build_statement(
            pb::statement::Kind::RevokeDevice,
            USER,
            FP,
            [],
            "",
            ISSUED,
            [4; 16],
        ),
        FP,
        7,
        1,
    );
    let (applied, rejected) =
        apply_log(&view(Permission::Control), 0, USER, &[revoke, bind(2)]).unwrap();
    assert_eq!(
        applied,
        vec![Applied {
            seq: 1,
            action: TrustAction::Revoke { fp: FP }
        }]
    );
    assert_eq!(
        rejected,
        vec![Rejected {
            seq: 2,
            reason: RejectReason::SignerRevoked
        }]
    );
}
#[test]
fn a07_unsigned_binding_forbidden() {
    let mut entry = bind(1);
    entry.signer_fp.clear();
    entry.signature.clear();
    assert_rejected(
        &view(Permission::Control),
        entry,
        RejectReason::MissingSigner,
    );
}
#[test]
fn a08_other_device_cannot_bind() {
    let mut view = view(Permission::Control);
    view.0.insert(
        [8; 32],
        KnownDevice {
            spki_der: public_key_der(&signing_key(8)),
            permission: Permission::Control,
            revoked: false,
        },
    );
    assert_rejected(
        &view,
        signed(&statement(), [8; 32], 8, 1),
        RejectReason::BindSignerMismatch,
    );
}
#[test]
fn a09_user_mismatch() {
    let mut statement = statement();
    statement.user = "other@example".into();
    assert_rejected(
        &view(Permission::Control),
        signed(&statement, FP, 7, 1),
        RejectReason::UserMismatch,
    );
}
#[test]
fn a10_tampered_statement() {
    let mut entry = bind(1);
    // Last byte is the final ASCII byte of subject_node: protobuf stays valid.
    *entry.statement.last_mut().unwrap() ^= 1;
    assert_rejected(
        &view(Permission::Control),
        entry,
        RejectReason::InvalidSignature,
    );
}
#[test]
fn a11_missing_signature() {
    let mut entry = bind(1);
    entry.signature.clear();
    assert_rejected(
        &view(Permission::Control),
        entry,
        RejectReason::MissingSignature,
    );
}
#[test]
fn a12_invalid_binding_fields() {
    let mut statements = Vec::new();
    for node in ["".into(), "a/b".into(), "a".repeat(65)] {
        let mut statement = statement();
        statement.subject_node = node;
        statements.push(statement);
    }
    let mut with_cert = statement();
    with_cert.subject_cert = vec![1];
    statements.push(with_cert);
    let mut with_name = statement();
    with_name.subject_name = "phone".into();
    statements.push(with_name);
    for statement in statements {
        assert_rejected(
            &view(Permission::Control),
            signed(&statement, FP, 7, 1),
            RejectReason::InvalidStatement,
        );
    }
}
#[test]
fn a13_invalid_subject_fingerprint() {
    let mut statement = statement();
    statement.subject_fp.pop();
    assert_rejected(
        &view(Permission::Control),
        signed(&statement, FP, 7, 1),
        RejectReason::InvalidSubjectFingerprint,
    );
}
fn approval() -> (rsp_core::trust::TrustEntry, [u8; 32]) {
    let key = signing_key(8).to_pkcs8_der().unwrap();
    let key = rcgen::KeyPair::try_from(key.as_bytes()).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let statement = build_approve(USER, cert.der(), "new phone", ISSUED, [3; 16]).unwrap();
    let fp = subject_fingerprint(cert.der()).unwrap();
    (signed(&statement, FP, 7, 1), fp)
}
#[test]
fn a14_approve_then_bind_in_same_batch() {
    let (approve, fp) = approval();
    let bind = signed(
        &build_bind_node(USER, fp, "n2", ISSUED, [4; 16]).unwrap(),
        fp,
        8,
        2,
    );
    let (applied, rejected) =
        apply_log(&view(Permission::Control), 0, USER, &[approve, bind]).unwrap();
    assert!(rejected.is_empty());
    assert_eq!(applied.len(), 2);
    assert!(
        matches!(&applied[0].action, TrustAction::Approve { fp: actual, permission: Permission::Control, .. } if actual == &fp)
    );
    assert_eq!(
        applied[1],
        Applied {
            seq: 2,
            action: TrustAction::BindNode {
                fp,
                node: "n2".into(),
                issued_at_us: ISSUED
            }
        }
    );
}
#[test]
fn a15_duplicate_approval_still_already_paired() {
    let (approve, _) = approval();
    let mut duplicate = approve.clone();
    duplicate.seq = 2;
    let (applied, rejected) =
        apply_log(&view(Permission::Control), 0, USER, &[approve, duplicate]).unwrap();
    assert_eq!(applied.len(), 1);
    assert!(matches!(applied[0].action, TrustAction::Approve { .. }));
    assert_eq!(
        rejected,
        vec![Rejected {
            seq: 2,
            reason: RejectReason::AlreadyPaired
        }]
    );
}
#[test]
fn invalid_signer_key_is_distinct_from_signature() {
    let mut view = view(Permission::Control);
    view.0.get_mut(&FP).unwrap().spki_der = vec![1, 2, 3];
    assert_rejected(&view, bind(1), RejectReason::InvalidSignerKey);
}
#[test]
fn rejection_precedence_is_preserved() {
    let mut entry = bind(1);
    entry.signer_fp = vec![8; 32];
    entry.signature.clear();
    assert_rejected(
        &View::default(),
        entry.clone(),
        RejectReason::BindSignerMismatch,
    );
    entry.signer_fp = FP.to_vec();
    let mut view = view(Permission::ViewOnly);
    view.0.get_mut(&FP).unwrap().revoked = true;
    assert_rejected(&view, entry, RejectReason::MissingSignature);
}

#[test]
fn b01_check_bind_node_returns_verified_claim() {
    let claim =
        rsp_core::trust::check_bind_node(&bind(1), &public_key_der(&signing_key(7)), Some(USER))
            .unwrap();
    assert_eq!(claim.fp, FP);
    assert_eq!(claim.node, "n2");
    assert_eq!(claim.issued_at_us, ISSUED);
}

#[test]
fn b02_check_bind_node_rejects_forged_signature() {
    assert_eq!(
        rsp_core::trust::check_bind_node(
            &signed(&statement(), FP, 8, 1),
            &public_key_der(&signing_key(7)),
            Some(USER),
        ),
        Err(RejectReason::InvalidSignature)
    );
}

#[test]
fn b03_check_bind_node_user_is_optional() {
    let spki = public_key_der(&signing_key(7));
    assert_eq!(
        rsp_core::trust::check_bind_node(&bind(1), &spki, Some("other@")),
        Err(RejectReason::UserMismatch)
    );
    assert!(rsp_core::trust::check_bind_node(&bind(1), &spki, None).is_ok());
}

#[test]
fn b04_check_bind_node_preserves_field_rejections() {
    let spki = public_key_der(&signing_key(7));
    let mut bad_node = statement();
    bad_node.subject_node = "a/b".into();
    let mut with_cert = statement();
    with_cert.subject_cert = vec![1];
    let mut with_name = statement();
    with_name.subject_name = "phone".into();
    for statement in [bad_node, with_cert, with_name] {
        assert_eq!(
            rsp_core::trust::check_bind_node(&signed(&statement, FP, 7, 1), &spki, None),
            Err(RejectReason::InvalidStatement)
        );
    }
    let mut bad_subject = statement();
    bad_subject.subject_fp.pop();
    let mut bad_signer = bind(1);
    bad_signer.signer_fp.pop();
    let mut unsigned = bind(1);
    unsigned.signer_fp.clear();
    let mut missing_signature = bind(1);
    missing_signature.signature.clear();
    for (entry, reason) in [
        (
            signed(&bad_subject, FP, 7, 1),
            RejectReason::InvalidSubjectFingerprint,
        ),
        (bad_signer, RejectReason::InvalidSignerFingerprint),
        (unsigned, RejectReason::MissingSigner),
        (missing_signature, RejectReason::MissingSignature),
        (
            signed(&statement(), [8; 32], 8, 1),
            RejectReason::BindSignerMismatch,
        ),
    ] {
        assert_eq!(
            rsp_core::trust::check_bind_node(&entry, &spki, None),
            Err(reason)
        );
    }
    assert_eq!(
        rsp_core::trust::check_bind_node(&bind(1), &[1, 2, 3], None),
        Err(RejectReason::InvalidSignerKey)
    );
    let mut wrong_kind = statement();
    wrong_kind.kind = pb::statement::Kind::ApproveDevice as i32;
    assert_eq!(
        rsp_core::trust::check_bind_node(&signed(&wrong_kind, FP, 7, 1), &spki, None),
        Err(RejectReason::InvalidStatement)
    );
}

#[test]
fn b05_certificate_spki_hash_matches_trust_vector_fingerprint() {
    use sha2::{Digest, Sha256};
    let signed = pb::SignedStatement::decode(
        include_bytes!("vectors/trust/approve_signed_p256.bin").as_slice(),
    )
    .unwrap();
    let statement = pb::Statement::decode(signed.statement.as_slice()).unwrap();
    let spki = rsp_core::trust::spki_der_from_cert(&statement.subject_cert).unwrap();
    let fp: [u8; 32] = Sha256::digest(&spki).into();
    assert_eq!(fp, subject_fingerprint(&statement.subject_cert).unwrap());
    assert_eq!(fp.as_slice(), statement.subject_fp);
    assert_eq!(
        rsp_core::trust::spki_der_from_cert(&[1, 2, 3]),
        Err(RejectReason::SubjectCertMismatch)
    );
    let mut trailing = statement.subject_cert;
    trailing.push(0);
    assert_eq!(
        rsp_core::trust::spki_der_from_cert(&trailing),
        Err(RejectReason::SubjectCertMismatch)
    );
}
