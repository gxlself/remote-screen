use sha2::Digest;
use std::collections::HashMap;
use std::convert::Infallible;

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use prost::Message;
use rsp_core::pb;
use rsp_core::trust::{
    apply_log, build_statement, sign_with, signing_payload, verify, DeviceView, KnownDevice,
    Permission, RejectReason, TrustAction, TrustError,
};

fn signing_key(byte: u8) -> SigningKey {
    // Test-only fixed P-256 key material; never use it in production.
    SigningKey::from_bytes((&[byte; 32]).into()).expect("fixed test key is valid")
}

fn public_key_der(key: &SigningKey) -> Vec<u8> {
    key.verifying_key()
        .to_public_key_der()
        .expect("test public key encoding")
        .as_bytes()
        .to_vec()
}

fn signed(
    key: &SigningKey,
    signer_fp: [u8; 32],
    kind: pb::statement::Kind,
    user: &str,
    subject_fp: [u8; 32],
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    let statement = build_statement(kind, user, subject_fp, [0xaa], "device", 123, [0xbb; 16]);
    let result = sign_with(&statement, signer_fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    rsp_core::trust::TrustEntry {
        seq,
        signed: result,
    }
}

fn control_statement(
    kind: pb::statement::Kind,
    user: &str,
    subject_fp: [u8; 32],
    seq: u64,
) -> rsp_core::trust::TrustEntry {
    signed(&signing_key(6), [6; 32], kind, user, subject_fp, seq)
}

struct TestView {
    devices: HashMap<[u8; 32], KnownDevice>,
}

impl Default for TestView {
    fn default() -> Self {
        Self {
            devices: HashMap::from([(
                [6; 32],
                KnownDevice {
                    spki_der: public_key_der(&signing_key(6)),
                    permission: Permission::Control,
                    revoked: false,
                },
            )]),
        }
    }
}

impl DeviceView for TestView {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice> {
        self.devices.get(fp).cloned()
    }
}

fn view_for(fp: [u8; 32], key: &SigningKey, permission: Permission, revoked: bool) -> TestView {
    let mut view = TestView::default();
    view.devices.insert(
        fp,
        KnownDevice {
            spki_der: public_key_der(key),
            permission,
            revoked,
        },
    );
    view
}

#[test]
fn signing_payload_and_round_trip_verification() {
    let key = signing_key(7);
    let statement = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice@example",
        [2; 32],
        [3, 4],
        "phone",
        99,
        [5; 16],
    );
    let mut signed = sign_with(&statement, [1; 32], |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    assert_eq!(signing_payload(&signed.statement)[..13], *b"rsp-trust-v1\0");
    assert_eq!(
        verify(&signed, &public_key_der(&key)).expect("round trip"),
        statement.clone()
    );

    signed.statement[0] ^= 1;
    assert!(matches!(
        verify(&signed, &public_key_der(&key)),
        Err(TrustError::InvalidSignature)
    ));

    let mut signed = sign_with(&statement, [1; 32], |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    signed.signature[3] ^= 1;
    assert!(matches!(
        verify(&signed, &public_key_der(&key)),
        Err(TrustError::InvalidSignature)
    ));
    assert!(matches!(
        verify(&signed, &public_key_der(&signing_key(8))),
        Err(TrustError::InvalidSignature)
    ));
}

#[test]
fn fixed_signed_statement_vector_verifies() {
    let signed = pb::SignedStatement::decode(
        include_bytes!("vectors/trust/signed_statement.bin").as_slice(),
    )
    .expect("vector protobuf");
    let public_key = hex("3059301306072a8648ce3d020106082a8648ce3d030107034200041e18532fd4754c02f3041d9c75ceb33b83ffd81ac7ce4fe882ccb1c98bc5896ea46c311c4e2ff40dd96a3653e6e45445d32dfe486eced75c7a90c6a18881c0a3");
    let statement = verify(&signed, &public_key).expect("fixed vector signature");
    assert_eq!(statement.user, "vector@example");
    assert_eq!(statement.subject_fp, vec![0x22; 32]);
}

#[test]
fn unsigned_device_statements_are_rejected() {
    for kind in [
        pb::statement::Kind::RevokeDevice,
        pb::statement::Kind::RequireConfirm,
        pb::statement::Kind::ApproveDevice,
    ] {
        let mut entry = control_statement(kind, "alice", [1; 32], 1);
        entry.signer_fp.clear();
        entry.signature.clear();
        let (applied, rejected) = apply_log(&TestView::default(), 0, "alice", &[entry]).unwrap();
        assert!(applied.is_empty());
        assert_eq!(
            rejected,
            vec![rsp_core::trust::Rejected {
                seq: 1,
                reason: RejectReason::MissingSigner
            }]
        );
    }
}

#[test]
fn approvals_require_control_and_non_revoked_signer() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let statement = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        [4; 32],
        1,
    );

    let view = view_for(signer_fp, &signer_key, Permission::ViewOnly, false);
    let (_, rejected) =
        apply_log(&view, 0, "alice", std::slice::from_ref(&statement)).expect("log");
    assert_eq!(rejected[0].reason, RejectReason::InsufficientPermission);

    let view = view_for(signer_fp, &signer_key, Permission::Control, true);
    let (_, rejected) = apply_log(&view, 0, "alice", &[statement]).expect("log");
    assert_eq!(rejected[0].reason, RejectReason::SignerRevoked);
}

#[test]
fn approvals_reject_unpaired_signer() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let statement = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        [4; 32],
        1,
    );
    let (_, rejected) = apply_log(&TestView::default(), 0, "alice", &[statement]).expect("log");
    assert_eq!(rejected[0].reason, RejectReason::UnknownSigner);
}

#[test]
fn control_approval_is_applied_with_inherited_permission() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let view = view_for(signer_fp, &signer_key, Permission::Unattended, false);
    let subject_key =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("subject key");
    let subject_cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .expect("subject params")
        .self_signed(&subject_key)
        .expect("subject cert");
    let (_, parsed) = x509_parser::parse_x509_certificate(subject_cert.der()).expect("parse");
    let subject_fp: [u8; 32] = sha2::Sha256::digest(parsed.public_key().raw).into();
    let statement_data = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        subject_cert.der(),
        "device",
        123,
        [0xbb; 16],
    );
    let statement = sign_with(&statement_data, signer_fp, |payload| {
        let signature: Signature = signer_key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    let statement = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: statement,
    };
    let (applied, rejected) = apply_log(&view, 0, "alice", &[statement]).expect("log");
    assert!(rejected.is_empty());
    assert!(matches!(
        &applied[0].action,
        TrustAction::Approve {
            fp,
            cert_der,
            name,
            permission: Permission::Unattended,
        } if *fp == subject_fp && cert_der.as_slice() == subject_cert.der().as_ref() && name == "device"
    ));
}

#[test]
fn approval_rejects_mismatched_subject_certificate() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let view = view_for(signer_fp, &signer_key, Permission::Control, false);
    let subject_key =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("subject key");
    let subject_cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .expect("subject params")
        .self_signed(&subject_key)
        .expect("subject cert");
    let statement_data = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice",
        [4; 32],
        subject_cert.der(),
        "device",
        123,
        [0xbb; 16],
    );
    let statement = sign_with(&statement_data, signer_fp, |payload| {
        let signature: Signature = signer_key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    let statement = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: statement,
    };
    let (_, rejected) = apply_log(&view, 0, "alice", &[statement]).expect("log");
    assert_eq!(rejected[0].reason, RejectReason::SubjectCertMismatch);
}

#[test]
fn approval_rejects_rsa_subject_certificate() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let view = view_for(signer_fp, &signer_key, Permission::Control, false);
    let subject_cert = hex("30820303308201eba00302010202142a662bb2f5efdafa43d205cfc02acbbed0c1bbee300d06092a864886f70d01010b05003011310f300d06035504030c0672732d617070301e170d3236313032303132323532345a170d3237313032303132323532345a3011310f300d06035504030c0672732d61707030820122300d06092a864886f70d01010105000382010f003082010a0282010100ca07c17f044bd4d8026efae7ba0404050e877e2e35c71a5b209feb07552df012d68f456395679e2e877d9d5805d2cdae120c663bb7bf2339673b70e34200fb8bd89475b210b1dfdf3c28a2c497a84ce6cd94b2afa030dcb9f5233d89dc5287f88245a49bdf4ed9ecc534617ce86ec93fcc717d76e54c93a073850cec11d32fa8fac9bf251e345e2d90bac3e1968049467b79dde02c37064dde29f66578efc23e84f16f6507403442add132eb458e174e2c7265bb808fd48e383e10a729e5471ab39f0c8221794593c5b542ccaa7f5f71101b6c1985c407e78c3675e6d3406d359f24cb9bf800928397d5e54971242cc1fb440036492d101d1ca446f7df4b08ff0203010001a3533051301d0603551d0e041604147a176f8aa2cac0312648814218a6b18b0fa02529301f0603551d230418301680147a176f8aa2cac0312648814218a6b18b0fa02529300f0603551d130101ff040530030101ff300d06092a864886f70d01010b05000382010100afd07d9c569bcb875f945d1ecae5ef1737cdd515418bed6483ec325ba633e5f1f7a310a278077c771f015d10043e7bd3f19ad67788b63f6bbbe4716d27318723941d43f8c43b63e47ac32ce169a1c208e7ddc9553b76beccb433ea6684cfe8b290550770b5cc1fd07f6b0e7b6597c74570737010a13ad1b70bb5d2e57c39fdbed4ce48a173d9578de606683b747b6c3cf18c3225587230a89104ea55efc1dce8361bf1911a7f333ac973460d41a6807fe199841bd72a4cfe907bc76af65975e3ec69c00ba287343e9751913c0237d8a53d1c93222e24c89b18f15e991a9e42d7dd35d9941cacb778bcb83783de593f36c9f4eeb779d28a299370a82ec13beace");
    let (_, parsed) = x509_parser::parse_x509_certificate(&subject_cert).expect("parse");
    let subject_fp: [u8; 32] = sha2::Sha256::digest(parsed.public_key().raw).into();
    let statement_data = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        &subject_cert,
        "device",
        123,
        [0xbb; 16],
    );
    let statement = sign_with(&statement_data, signer_fp, |payload| {
        let signature: Signature = signer_key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    let statement = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: statement,
    };
    let (_, rejected) = apply_log(&view, 0, "alice", &[statement]).expect("log");
    assert_eq!(rejected[0].reason, RejectReason::UnsupportedSubjectKey);
}

#[test]
fn earlier_revoke_in_same_batch_invalidates_later_approval() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let view = view_for(signer_fp, &signer_key, Permission::Control, false);
    let revoke = control_statement(pb::statement::Kind::RevokeDevice, "alice", signer_fp, 1);
    let approve = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        [4; 32],
        2,
    );
    let (applied, rejected) = apply_log(&view, 0, "alice", &[approve, revoke]).expect("log");
    assert_eq!(applied.len(), 1);
    assert_eq!(
        rejected,
        vec![rsp_core::trust::Rejected {
            seq: 2,
            reason: RejectReason::SignerRevoked
        }]
    );
}

#[test]
fn approval_rejects_subject_revoked_in_view() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let subject_fp = [4; 32];
    let mut view = view_for(signer_fp, &signer_key, Permission::Control, false);
    view.devices.insert(
        subject_fp,
        KnownDevice {
            spki_der: Vec::new(),
            permission: Permission::ViewOnly,
            revoked: true,
        },
    );
    let approve = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        2,
    );
    let (applied, rejected) = apply_log(&view, 1, "alice", &[approve]).expect("log");
    assert!(applied.is_empty());
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].reason, RejectReason::SubjectRevoked);
}

#[test]
fn earlier_subject_revoke_in_same_batch_prevents_approval() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let subject_fp = [4; 32];
    let view = view_for(signer_fp, &signer_key, Permission::Control, false);
    let revoke = control_statement(pb::statement::Kind::RevokeDevice, "alice", subject_fp, 1);
    let approve = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        2,
    );
    let (applied, rejected) = apply_log(&view, 0, "alice", &[approve, revoke]).expect("log");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].action, TrustAction::Revoke { fp: subject_fp });
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].seq, 2);
    assert_eq!(rejected[0].reason, RejectReason::SubjectRevoked);
}

#[test]
fn approval_rejects_already_paired_view_only_subject() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let subject_fp = [4; 32];
    let mut view = view_for(signer_fp, &signer_key, Permission::Control, false);
    view.devices.insert(
        subject_fp,
        KnownDevice {
            spki_der: public_key_der(&signing_key(4)),
            permission: Permission::ViewOnly,
            revoked: false,
        },
    );
    let approve = signed(
        &signer_key,
        signer_fp,
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        1,
    );
    let (applied, rejected) = apply_log(&view, 0, "alice", &[approve]).expect("log");
    assert!(applied.is_empty());
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].reason, RejectReason::AlreadyPaired);
}

#[test]
fn duplicate_approval_in_batch_cannot_replace_permission_and_can_require_confirm() {
    let signer_fp = [9; 32];
    let signer_key = signing_key(9);
    let other_fp = [8; 32];
    let other_key = signing_key(8);
    let mut view = view_for(signer_fp, &signer_key, Permission::Control, false);
    view.devices.insert(
        other_fp,
        KnownDevice {
            spki_der: public_key_der(&other_key),
            permission: Permission::Unattended,
            revoked: false,
        },
    );
    let subject_key =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("subject key");
    let cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .expect("subject params")
        .self_signed(&subject_key)
        .expect("subject cert");
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.der()).expect("parse");
    let subject_fp = sha2::Sha256::digest(parsed.public_key().raw).into();
    let statement = build_statement(
        pb::statement::Kind::ApproveDevice,
        "alice",
        subject_fp,
        cert.der(),
        "device",
        123,
        [0xbb; 16],
    );
    let sign = |key: &SigningKey, fp, seq| {
        let signed = sign_with(&statement, fp, |payload| {
            let signature: Signature = key.sign(payload);
            Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
        })
        .expect("test signer");
        rsp_core::trust::TrustEntry { seq, signed }
    };
    let (applied, rejected) = apply_log(
        &view,
        0,
        "alice",
        &[
            sign(&signer_key, signer_fp, 1),
            sign(&other_key, other_fp, 2),
            control_statement(pb::statement::Kind::RequireConfirm, "alice", subject_fp, 3),
        ],
    )
    .expect("log");
    assert_eq!(applied.len(), 2);
    assert!(matches!(
        applied[0].action,
        TrustAction::Approve {
            permission: Permission::Control,
            ..
        }
    ));
    assert_eq!(
        applied[1].action,
        TrustAction::RequireConfirm { fp: subject_fp }
    );
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].seq, 2);
    assert_eq!(rejected[0].reason, RejectReason::AlreadyPaired);
}

#[test]
fn require_confirm_ignores_unknown_and_revoked_subjects_without_rejection() {
    let paired_fp = [9; 32];
    let revoked_fp = [4; 32];
    let mut view = view_for(paired_fp, &signing_key(9), Permission::Control, false);
    view.devices.insert(
        revoked_fp,
        KnownDevice {
            spki_der: Vec::new(),
            permission: Permission::ViewOnly,
            revoked: true,
        },
    );
    let (applied, rejected) = apply_log(
        &view,
        0,
        "alice",
        &[
            control_statement(pb::statement::Kind::RequireConfirm, "alice", [3; 32], 1),
            control_statement(pb::statement::Kind::RequireConfirm, "alice", revoked_fp, 2),
            control_statement(pb::statement::Kind::RevokeDevice, "alice", paired_fp, 3),
            control_statement(pb::statement::Kind::RequireConfirm, "alice", paired_fp, 4),
        ],
    )
    .expect("log");
    assert!(rejected.is_empty());
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].action, TrustAction::Revoke { fp: paired_fp });
}

#[test]
fn sequence_gap_duplicates_and_old_entries() {
    let view = view_for([2; 32], &signing_key(2), Permission::ViewOnly, false);
    let err = apply_log(
        &view,
        4,
        "alice",
        &[control_statement(
            pb::statement::Kind::RevokeDevice,
            "alice",
            [1; 32],
            6,
        )],
    )
    .expect_err("missing sequence five");
    assert!(matches!(
        err,
        TrustError::Gap {
            expected: 5,
            got: 6
        }
    ));

    let first = control_statement(pb::statement::Kind::RevokeDevice, "alice", [1; 32], 5);
    let duplicate = control_statement(pb::statement::Kind::RequireConfirm, "alice", [2; 32], 5);
    let old = control_statement(pb::statement::Kind::RequireConfirm, "alice", [3; 32], 4);
    let (applied, rejected) =
        apply_log(&view, 4, "alice", &[duplicate, old, first]).expect("duplicates skipped");
    assert_eq!(applied.len(), 1);
    assert!(rejected.is_empty());
}

#[test]
fn mismatched_user_is_rejected_without_stopping_log() {
    let view = TestView::default();
    let wrong = control_statement(pb::statement::Kind::RequireConfirm, "bob", [1; 32], 1);
    let right = control_statement(pb::statement::Kind::RevokeDevice, "alice", [2; 32], 2);
    let (applied, rejected) = apply_log(&view, 0, "alice", &[wrong, right]).expect("log continues");
    assert_eq!(applied.len(), 1);
    assert_eq!(rejected[0].reason, RejectReason::UserMismatch);
}

#[test]
fn revoke_from_other_user_is_applied() {
    let view = TestView::default();
    let revoke = control_statement(pb::statement::Kind::RevokeDevice, "bob", [3; 32], 1);
    let (applied, rejected) = apply_log(&view, 0, "alice", &[revoke]).expect("global revoke");
    assert!(rejected.is_empty());
    assert!(matches!(
        applied.as_slice(),
        [rsp_core::trust::Applied {
            seq: 1,
            action: TrustAction::Revoke { fp }
        }] if *fp == [3; 32]
    ));
}

#[test]
fn approve_from_other_user_is_rejected() {
    let view = TestView::default();
    let approve = control_statement(pb::statement::Kind::ApproveDevice, "bob", [4; 32], 1);
    let (applied, rejected) = apply_log(&view, 0, "alice", &[approve]).expect("user check");
    assert!(applied.is_empty());
    assert_eq!(rejected[0].reason, RejectReason::UserMismatch);
}

fn hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).expect("hex test vector"))
        .collect()
}

#[test]
fn signed_control_statements_reject_tampered_signatures() {
    for kind in [
        pb::statement::Kind::RevokeDevice,
        pb::statement::Kind::RequireConfirm,
    ] {
        let mut entry = control_statement(kind, "alice", [1; 32], 1);
        entry.signature[3] ^= 1;
        let (applied, rejected) = apply_log(&TestView::default(), 0, "alice", &[entry]).unwrap();
        assert!(applied.is_empty());
        assert_eq!(
            rejected,
            vec![rsp_core::trust::Rejected {
                seq: 1,
                reason: RejectReason::InvalidSignature
            }]
        );
    }
}

#[test]
fn signed_control_statements_require_a_known_authorized_signer() {
    for kind in [
        pb::statement::Kind::RevokeDevice,
        pb::statement::Kind::RequireConfirm,
    ] {
        for reason in [
            RejectReason::UnknownSigner,
            RejectReason::SignerRevoked,
            RejectReason::InsufficientPermission,
            RejectReason::MissingSignature,
            RejectReason::InvalidSignerFingerprint,
        ] {
            let mut view = TestView::default();
            let mut entry = control_statement(kind, "alice", [1; 32], 1);
            match reason {
                RejectReason::UnknownSigner => {
                    view.devices.remove(&[6; 32]);
                }
                RejectReason::SignerRevoked => {
                    view.devices.get_mut(&[6; 32]).unwrap().revoked = true;
                }
                RejectReason::InsufficientPermission => {
                    view.devices.get_mut(&[6; 32]).unwrap().permission = Permission::ViewOnly;
                }
                RejectReason::MissingSignature => entry.signature.clear(),
                RejectReason::InvalidSignerFingerprint => {
                    entry.signer_fp.pop();
                }
                _ => unreachable!(),
            }
            let (applied, rejected) = apply_log(&view, 0, "alice", &[entry]).unwrap();
            assert!(applied.is_empty());
            assert_eq!(rejected, vec![rsp_core::trust::Rejected { seq: 1, reason }]);
        }
    }
}
