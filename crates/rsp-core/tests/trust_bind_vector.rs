use std::convert::Infallible;
use std::fs;
use std::path::PathBuf;

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use prost::Message;
use rsp_core::pb;
use rsp_core::trust::{
    build_bind_node, sign_with, valid_subject_node, verify, RejectReason, TrustError,
    SUBJECT_NODE_MAX_LEN,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const USER: &str = "vector@example";
const SUBJECT_NODE: &str = "nK7m2q4xCNTRL";
const ISSUED_AT_US: u64 = 1_792_465_134_000_000;
const NONCE: [u8; 16] = [0x55; 16];

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

fn certificate_spki(cert_der: &[u8]) -> Vec<u8> {
    let (rest, cert) = x509_parser::parse_x509_certificate(cert_der).expect("test certificate");
    assert!(rest.is_empty());
    cert.public_key().raw.to_vec()
}

fn sign_statement(statement: &pb::Statement, device_fp: [u8; 32]) -> pb::SignedStatement {
    let key = signing_key(7);
    let signed = sign_with(statement, device_fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    });
    signed.expect("test signer")
}

#[derive(Deserialize)]
struct ApproveFlowCertificate {
    signer_cert_der_hex: String,
}

#[derive(Deserialize, Serialize)]
struct BindNodeVector {
    description: String,
    user: String,
    subject_node: String,
    issued_at_us: u64,
    nonce_hex: String,
    device_cert_der_hex: String,
    device_fp_hex: String,
    statement_hex: String,
    signature_der_hex: String,
    signed_hex: String,
}

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/trust")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "hex test vector has an odd length");
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).expect("hex test vector"))
        .collect()
}

fn ensure_vector_files() {
    if std::env::var_os("RSP_REGEN_VECTORS").is_none_or(|value| value != "1") {
        return;
    }
    let dir = vector_dir();
    let approve: ApproveFlowCertificate = serde_json::from_slice(
        &fs::read(dir.join("approve_flow.json")).expect("read approve-flow JSON"),
    )
    .expect("parse approve-flow JSON");
    let device_cert = unhex(&approve.signer_cert_der_hex);
    let device_spki = certificate_spki(&device_cert);
    assert_eq!(device_spki, public_key_der(&signing_key(7)));
    let device_fp: [u8; 32] = Sha256::digest(&device_spki).into();
    let statement = build_bind_node(USER, device_fp, SUBJECT_NODE, ISSUED_AT_US, NONCE)
        .expect("vector BIND_NODE statement");
    let signed = sign_statement(&statement, device_fp);
    let vector = BindNodeVector {
        description: "BIND_NODE signed deterministically (RFC 6979) with test-only P-256 private key [0x07; 32]; device certificate reused from approve_flow.json; signer_fp = subject_fp. Never use these keys in production.".into(),
        user: USER.into(),
        subject_node: SUBJECT_NODE.into(),
        issued_at_us: ISSUED_AT_US,
        nonce_hex: hex(&NONCE),
        device_cert_der_hex: hex(&device_cert),
        device_fp_hex: hex(&device_fp),
        statement_hex: hex(&signed.statement),
        signature_der_hex: hex(&signed.signature),
        signed_hex: hex(&signed.encode_to_vec()),
    };
    fs::create_dir_all(&dir).expect("create trust vector directory");
    fs::write(dir.join("bind_node.bin"), signed.encode_to_vec()).expect("write BIND_NODE binary");
    let mut pretty = serde_json::to_vec_pretty(&vector).expect("serialize BIND_NODE JSON");
    pretty.push(b'\n');
    fs::write(dir.join("bind_node.json"), pretty).expect("write BIND_NODE JSON");
}

#[test]
fn bind_node_vector_matches_builder_and_verifies() {
    ensure_vector_files();
    let dir = vector_dir();
    let vector: BindNodeVector =
        serde_json::from_slice(&fs::read(dir.join("bind_node.json")).expect("read BIND_NODE JSON"))
            .expect("parse BIND_NODE JSON");
    let bytes = fs::read(dir.join("bind_node.bin")).expect("read BIND_NODE binary");
    let signed = pb::SignedStatement::decode(bytes.as_slice()).expect("BIND_NODE protobuf");
    let device_cert = unhex(&vector.device_cert_der_hex);
    let device_spki = certificate_spki(&device_cert);
    assert_eq!(device_spki, public_key_der(&signing_key(7)));
    let device_fp: [u8; 32] = Sha256::digest(&device_spki).into();
    assert_eq!(vector.user, USER);
    assert_eq!(vector.subject_node, SUBJECT_NODE);
    assert_eq!(vector.issued_at_us, ISSUED_AT_US);
    assert_eq!(vector.nonce_hex, hex(&NONCE));
    assert_eq!(vector.device_fp_hex, hex(&device_fp));
    assert_eq!(signed.signer_fp, device_fp);
    assert_eq!(vector.statement_hex, hex(&signed.statement));
    assert_eq!(vector.signature_der_hex, hex(&signed.signature));
    assert_eq!(vector.signed_hex, hex(&bytes));
    assert!(vector.description.contains("test-only"));

    let expected = build_bind_node(USER, device_fp, SUBJECT_NODE, ISSUED_AT_US, NONCE)
        .expect("build vector statement");
    assert_eq!(vector.statement_hex, hex(&expected.encode_to_vec()));
    assert_eq!(
        verify(&signed, &device_spki).expect("verify BIND_NODE"),
        expected.clone()
    );
    assert_eq!(
        sign_statement(&expected, device_fp).signature,
        signed.signature
    );

    assert_eq!(
        pb::statement::Kind::try_from(expected.kind),
        Ok(pb::statement::Kind::BindNode)
    );
    assert_eq!(expected.subject_node, SUBJECT_NODE);
    assert_eq!(expected.subject_fp, device_fp);
    assert_eq!(signed.signer_fp, expected.subject_fp);
    assert!(expected.subject_cert.is_empty());
    assert!(expected.subject_name.is_empty());

    let mut tampered = signed.clone();
    tampered.statement[0] ^= 1;
    assert!(matches!(
        verify(&tampered, &device_spki),
        Err(TrustError::InvalidSignature)
    ));
}

#[test]
fn valid_subject_node_table() {
    assert_eq!(SUBJECT_NODE_MAX_LEN, 64);
    for node in ["nK7m2q4xCNTRL", "12", "a.b_c-d"] {
        assert!(valid_subject_node(node), "{node:?}");
    }
    assert!(valid_subject_node(&"a".repeat(64)));
    for node in ["", "n K", "a/b", "节点", "a\n"] {
        assert!(!valid_subject_node(node), "{node:?}");
    }
    assert!(!valid_subject_node(&"a".repeat(65)));
}

#[test]
fn build_bind_node_rejects_bad_node() {
    let fp = [0x07; 32];
    for node in ["", &"a".repeat(65)] {
        assert_eq!(
            build_bind_node(USER, fp, node, ISSUED_AT_US, NONCE),
            Err(RejectReason::InvalidStatement)
        );
    }
}
