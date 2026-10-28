use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use prost::Message;
use rsp_core::pb;
use rsp_core::trust::{
    apply_log, build_statement, sign_with, verify, DeviceView, KnownDevice, Permission,
    RejectReason, TrustAction,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SUBJECT_PKCS8_DER: [u8; 138] = [
    0x30, 0x81, 0x87, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02,
    0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x6d, 0x30, 0x6b, 0x02,
    0x01, 0x01, 0x04, 0x20, 0xd9, 0x80, 0x0a, 0x62, 0xde, 0xd6, 0xbc, 0x1f, 0xb5, 0x80, 0xec, 0x7f,
    0x5b, 0xa2, 0x00, 0xf6, 0x30, 0xe6, 0x6b, 0x46, 0xe6, 0x27, 0x0e, 0xd4, 0x70, 0x67, 0xb1, 0x94,
    0x9b, 0xb6, 0x50, 0x28, 0xa1, 0x44, 0x03, 0x42, 0x00, 0x04, 0xf6, 0x18, 0x13, 0x71, 0x49, 0x41,
    0xb7, 0x61, 0x9b, 0x15, 0xb5, 0xa6, 0x76, 0x4f, 0x14, 0x9f, 0x67, 0x2f, 0x0c, 0x13, 0xa0, 0xe9,
    0x3a, 0x21, 0xf6, 0xbf, 0x72, 0xc4, 0xb1, 0x12, 0x49, 0x4e, 0x57, 0x43, 0xbd, 0xc4, 0x78, 0xf6,
    0x5a, 0x93, 0xde, 0x45, 0xfa, 0x3e, 0x9b, 0xcf, 0xe2, 0x7f, 0x4d, 0x82, 0x75, 0x73, 0xf6, 0xef,
    0xbf, 0x7f, 0x3c, 0x29, 0x30, 0x34, 0x6a, 0x46, 0x57, 0x8f,
];

const VECTOR_NAMES: [&str; 1] = ["approve_signed_p256"];
const USER: &str = "vector@example";
const SIGNER_FP_BYTE: u8 = 0x07;
const SUBJECT_NAME: &str = "Approved Phone";

static REGEN_ONCE: Once = Once::new();

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/trust")
}

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "hex test vector has an odd length");
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).expect("hex test vector"))
        .collect()
}

fn statement_json(statement: &pb::Statement) -> Value {
    let kind = pb::statement::Kind::try_from(statement.kind).expect("vector statement kind");
    json!({
        "kind": kind.as_str_name(),
        "user": statement.user,
        "subject_fp": hex(&statement.subject_fp),
        "subject_cert": hex(&statement.subject_cert),
        "subject_name": statement.subject_name,
        "issued_at_us": statement.issued_at_us,
        "nonce": hex(&statement.nonce),
    })
}

fn signed_fields_json(signed: &pb::SignedStatement) -> Value {
    json!({
        "signer_fp": hex(&signed.signer_fp),
        "signature_der": hex(&signed.signature),
    })
}

fn subject_certificate() -> Vec<u8> {
    let key = rcgen::KeyPair::try_from(SUBJECT_PKCS8_DER.as_slice())
        .expect("fixed subject PKCS#8 key imports");
    let mut params = rcgen::CertificateParams::new(vec!["rs-app".into()]).expect("subject params");
    params.serial_number = Some(rcgen::SerialNumber::from(7u64));
    params
        .self_signed(&key)
        .expect("fixed subject certificate")
        .der()
        .to_vec()
}

fn subject_fingerprint(cert_der: &[u8]) -> [u8; 32] {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der).expect("subject certificate");
    Sha256::digest(cert.public_key().raw).into()
}

fn approved_statement(subject_fp: [u8; 32], subject_cert: &[u8]) -> pb::SignedStatement {
    let signer = signing_key(SIGNER_FP_BYTE);
    let statement = build_statement(
        pb::statement::Kind::ApproveDevice,
        USER,
        subject_fp,
        subject_cert,
        SUBJECT_NAME,
        1_735_689_600_654_321,
        [0x44; 16],
    );
    sign_with(&statement, [SIGNER_FP_BYTE; 32], |payload| {
        let signature: Signature = signer.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer")
}

fn regen_cases() -> Vec<(&'static str, pb::SignedStatement, Value)> {
    let subject_cert = subject_certificate();
    let subject_fp = subject_fingerprint(&subject_cert);
    let signer_fp = [SIGNER_FP_BYTE; 32];

    let approve = approved_statement(subject_fp, &subject_cert);
    vec![(
        "approve_signed_p256",
        approve.clone(),
        json!({
            "name": "approve_signed_p256",
            "statement": statement_json(&pb::Statement::decode(approve.statement.as_slice()).expect("approve statement")),
            "signer_fp": hex(&signer_fp),
            "signature_der": hex(&approve.signature),
            "apply": {
                "last_applied_seq": 0,
                "user": USER,
                "view": {
                    "devices": {
                        hex(&signer_fp): {
                            "permission": "Control",
                            "revoked": false,
                            "spki_der": hex(&public_key_der(&signing_key(SIGNER_FP_BYTE)))
                        }
                    }
                },
                "expected_applied": [format!(
                    "Approve{{fp: {}, permission: Control, name: {SUBJECT_NAME}}}",
                    hex(&subject_fp)
                )],
                "expected_rejected": []
            },
            "description": "APPROVE_DEVICE signed by test-only P-256 key [0x07; 32]. The subject is a self-signed P-256 certificate generated by rcgen from a fixed PKCS#8 key; the certificate DER changes when the vector is regenerated because ECDSA signatures are randomized.",
            "private_key": "test-only fixed P-256 key (0x07 repeated 32 times); never use in production",
            "subject_private_key": "test-only fixed PKCS#8 P-256 key; never use in production"
        }),
    )]
}

fn is_regen() -> bool {
    std::env::var_os("RSP_REGEN_VECTORS").is_some_and(|value| value == "1")
}

fn write_vector_files(dir: &Path, cases: &[(&str, pb::SignedStatement, Value)]) {
    fs::create_dir_all(dir).expect("create trust vector directory");
    for (name, signed, description) in cases {
        fs::write(dir.join(format!("{name}.bin")), signed.encode_to_vec())
            .expect("write trust vector binary");
        let mut pretty = serde_json::to_vec_pretty(description).expect("serialize vector json");
        pretty.push(b'\n');
        fs::write(dir.join(format!("{name}.json")), pretty).expect("write trust vector json");
    }
}

fn ensure_vector_files() {
    if is_regen() {
        REGEN_ONCE.call_once(|| {
            write_vector_files(&vector_dir(), &regen_cases());
        });
    }
}

#[derive(Default)]
struct TestView {
    devices: HashMap<[u8; 32], KnownDevice>,
}

impl DeviceView for TestView {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice> {
        self.devices.get(fp).cloned()
    }
}

fn view_from_json(apply: &Value) -> TestView {
    let mut view = TestView::default();
    let devices = apply["view"]["devices"]
        .as_object()
        .expect("apply.view.devices object");
    for (fingerprint, device) in devices {
        let fp: [u8; 32] = unhex(fingerprint)
            .try_into()
            .expect("32-byte device fingerprint");
        let permission = match device["permission"].as_str().expect("device permission") {
            "ViewOnly" => Permission::ViewOnly,
            "Control" => Permission::Control,
            "Unattended" => Permission::Unattended,
            other => panic!("unknown test permission {other}"),
        };
        let revoked = device["revoked"].as_bool().expect("device revoked flag");
        let spki_der = unhex(device["spki_der"].as_str().expect("device spki_der"));
        view.devices.insert(
            fp,
            KnownDevice {
                spki_der,
                permission,
                revoked,
            },
        );
    }
    view
}

fn action_summary(action: &TrustAction) -> String {
    match action {
        TrustAction::BindNode { fp, node, .. } => {
            format!("BindNode{{fp: {}, node: {node}}}", hex(fp))
        }
        TrustAction::Revoke { fp } => format!("Revoke{{fp: {}}}", hex(fp)),
        TrustAction::RequireConfirm { fp } => format!("RequireConfirm{{fp: {}}}", hex(fp)),
        TrustAction::Approve {
            fp,
            permission,
            name,
            ..
        } => format!(
            "Approve{{fp: {}, permission: {permission:?}, name: {name}}}",
            hex(fp)
        ),
    }
}

fn rejection_summary(reason: &RejectReason) -> String {
    format!("{reason:?}")
}

fn read_vector(name: &str) -> (pb::SignedStatement, Value) {
    let dir = vector_dir();
    let json: Value = serde_json::from_slice(
        &fs::read(dir.join(format!("{name}.json")))
            .unwrap_or_else(|error| panic!("read {name}.json: {error}")),
    )
    .unwrap_or_else(|error| panic!("parse {name}.json: {error}"));
    let signed = pb::SignedStatement::decode(
        fs::read(dir.join(format!("{name}.bin")))
            .unwrap_or_else(|error| panic!("read {name}.bin: {error}"))
            .as_slice(),
    )
    .unwrap_or_else(|error| panic!("decode {name}.bin: {error}"));
    (signed, json)
}

fn assert_apply(name: &str, signed: &pb::SignedStatement, json: &Value) {
    let apply = &json["apply"];
    let last_applied_seq = apply["last_applied_seq"]
        .as_u64()
        .expect("last_applied_seq");
    let user = apply["user"].as_str().expect("apply.user");
    let view = view_from_json(apply);
    let (applied, rejected) = apply_log(
        &view,
        last_applied_seq,
        user,
        &[rsp_core::trust::TrustEntry {
            seq: last_applied_seq + 1,
            signed: signed.clone(),
        }],
    )
    .unwrap_or_else(|error| panic!("apply {name}: {error}"));
    let applied: Vec<String> = applied
        .iter()
        .map(|entry| action_summary(&entry.action))
        .collect();
    let rejected: Vec<String> = rejected
        .iter()
        .map(|entry| rejection_summary(&entry.reason))
        .collect();
    let expected_applied: Vec<String> = serde_json::from_value(apply["expected_applied"].clone())
        .expect("expected_applied string array");
    let expected_rejected: Vec<String> = serde_json::from_value(apply["expected_rejected"].clone())
        .expect("expected_rejected string array");
    assert_eq!(applied, expected_applied, "{name} applied actions");
    assert_eq!(rejected, expected_rejected, "{name} rejections");
}

#[test]
fn trust_vectors_decode_and_apply() {
    ensure_vector_files();

    for name in VECTOR_NAMES {
        let (signed, json) = read_vector(name);
        assert_eq!(json["name"], name);
        assert_eq!(json["signer_fp"], signed_fields_json(&signed)["signer_fp"]);
        assert_eq!(
            json["signature_der"],
            signed_fields_json(&signed)["signature_der"]
        );
        let statement = pb::Statement::decode(signed.statement.as_slice())
            .unwrap_or_else(|error| panic!("decode {name} statement: {error}"));
        assert_eq!(
            json["statement"],
            statement_json(&statement),
            "{name} statement fields"
        );
        assert_apply(name, &signed, &json);
    }
}

#[test]
fn approve_vector_signature_and_certificate_verify() {
    ensure_vector_files();
    let (signed, _) = read_vector("approve_signed_p256");
    let statement =
        verify(&signed, &public_key_der(&signing_key(SIGNER_FP_BYTE))).expect("approve signature");
    assert_eq!(statement.kind, pb::statement::Kind::ApproveDevice as i32);
    assert_eq!(statement.user, USER);
    assert_eq!(
        subject_fingerprint(&statement.subject_cert),
        statement.subject_fp.as_slice(),
        "subject_fp must be SHA-256 of the certificate SPKI"
    );
}

#[test]
fn approve_vector_rejects_view_only_signer() {
    ensure_vector_files();
    let (signed, json) = read_vector("approve_signed_p256");
    let mut view = view_from_json(&json["apply"]);
    view.devices
        .get_mut(&[SIGNER_FP_BYTE; 32])
        .expect("signer present")
        .permission = Permission::ViewOnly;
    let (applied, rejected) = apply_log(
        &view,
        0,
        USER,
        &[rsp_core::trust::TrustEntry { seq: 1, signed }],
    )
    .expect("apply approve vector");
    assert!(applied.is_empty());
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].reason, RejectReason::InsufficientPermission);
}
