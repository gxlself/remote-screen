use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::path::PathBuf;

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey};
use prost::Message;
use rsp_core::pb;
use rsp_core::trust::{
    apply_log, build_approve, sign_with, subject_fingerprint, valid_subject_name, verify, Applied,
    DeviceView, KnownDevice, Permission, RejectReason, Rejected, TrustAction, TrustError,
    SUBJECT_NAME_MAX_CHARS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const USER: &str = "vector@example";
const SUBJECT_NAME: &str = "小明的 iPad";
const ISSUED_AT_US: u64 = 1_792_465_134_000_000;
const NONCE: [u8; 16] = [0x44; 16];

// Same public RSA certificate as approval_rejects_rsa_subject_certificate in trust.rs.
const RSA_CERT_HEX: &str = "30820303308201eba00302010202142a662bb2f5efdafa43d205cfc02acbbed0c1bbee300d06092a864886f70d01010b05003011310f300d06035504030c0672732d617070301e170d3236313032303132323532345a170d3237313032303132323532345a3011310f300d06035504030c0672732d61707030820122300d06092a864886f70d01010105000382010f003082010a0282010100ca07c17f044bd4d8026efae7ba0404050e877e2e35c71a5b209feb07552df012d68f456395679e2e877d9d5805d2cdae120c663bb7bf2339673b70e34200fb8bd89475b210b1dfdf3c28a2c497a84ce6cd94b2afa030dcb9f5233d89dc5287f88245a49bdf4ed9ecc534617ce86ec93fcc717d76e54c93a073850cec11d32fa8fac9bf251e345e2d90bac3e1968049467b79dde02c37064dde29f66578efc23e84f16f6507403442add132eb458e174e2c7265bb808fd48e383e10a729e5471ab39f0c8221794593c5b542ccaa7f5f71101b6c1985c407e78c3675e6d3406d359f24cb9bf800928397d5e54971242cc1fb440036492d101d1ca446f7df4b08ff0203010001a3533051301d0603551d0e041604147a176f8aa2cac0312648814218a6b18b0fa02529301f0603551d230418301680147a176f8aa2cac0312648814218a6b18b0fa02529300f0603551d130101ff040530030101ff300d06092a864886f70d01010b05000382010100afd07d9c569bcb875f945d1ecae5ef1737cdd515418bed6483ec325ba633e5f1f7a310a278077c771f015d10043e7bd3f19ad67788b63f6bbbe4716d27318723941d43f8c43b63e47ac32ce169a1c208e7ddc9553b76beccb433ea6684cfe8b290550770b5cc1fd07f6b0e7b6597c74570737010a13ad1b70bb5d2e57c39fdbed4ce48a173d9578de606683b747b6c3cf18c3225587230a89104ea55efc1dce8361bf1911a7f333ac973460d41a6807fe199841bd72a4cfe907bc76af65975e3ec69c00ba287343e9751913c0237d8a53d1c93222e24c89b18f15e991a9e42d7dd35d9941cacb778bcb83783de593f36c9f4eeb779d28a299370a82ec13beace";

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

fn certificate(key: &rcgen::KeyPair, common_name: &str) -> Vec<u8> {
    let mut params = rcgen::CertificateParams::new(Vec::new()).expect("certificate params");
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params.serial_number = Some(rcgen::SerialNumber::from(7u64));
    params
        .self_signed(key)
        .expect("self-signed certificate")
        .der()
        .to_vec()
}

fn p256_certificate(byte: u8, common_name: &str) -> Vec<u8> {
    let pkcs8 = signing_key(byte).to_pkcs8_der().expect("test PKCS#8 key");
    let key = rcgen::KeyPair::try_from(pkcs8.as_bytes()).expect("import test P-256 key");
    certificate(&key, common_name)
}

fn certificate_spki(cert_der: &[u8]) -> Vec<u8> {
    let (rest, cert) = x509_parser::parse_x509_certificate(cert_der).expect("test certificate");
    assert!(rest.is_empty());
    cert.public_key().raw.to_vec()
}

fn invalid_certificates() -> Vec<(Vec<u8>, RejectReason)> {
    let mut trailing = p256_certificate(8, "approve-subject");
    trailing.push(0);
    vec![
        (Vec::new(), RejectReason::MissingCertificate),
        (trailing, RejectReason::SubjectCertMismatch),
        (vec![0xa5; 100], RejectReason::SubjectCertMismatch),
        (unhex(RSA_CERT_HEX), RejectReason::UnsupportedSubjectKey),
    ]
}

fn signed(statement: &pb::Statement, seq: u64) -> rsp_core::trust::TrustEntry {
    let key = signing_key(7);
    let signer_fp = Sha256::digest(public_key_der(&key)).into();
    let signed = sign_with(statement, signer_fp, |payload| {
        let signature: Signature = key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("test signer");
    rsp_core::trust::TrustEntry { seq, signed }
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

fn control_view(spki_der: Vec<u8>) -> TestView {
    let fp = Sha256::digest(&spki_der).into();
    TestView {
        devices: HashMap::from([(
            fp,
            KnownDevice {
                spki_der,
                permission: Permission::Control,
                revoked: false,
            },
        )]),
    }
}

#[test]
fn subject_names_use_trimmed_unicode_scalar_count_and_reject_original_controls() {
    assert_eq!(SUBJECT_NAME_MAX_CHARS, 64);
    for name in [
        SUBJECT_NAME,
        " 小明的 iPad ",
        " 汉 ",
        "\u{2003}iPad\u{2003}",
        &"汉".repeat(64),
        &format!(" {}", "汉".repeat(64)),
        &format!(" {} ", "汉".repeat(64)),
    ] {
        assert!(valid_subject_name(name), "{name:?}");
    }
    for name in [
        "",
        "   ",
        "\u{2003}",
        "a\nb",
        "a\u{7}",
        "a\u{0}b",
        "a\rb",
        "a\tb",
        "\niPad",
        "iPad\u{7f}",
        "a\u{85}b",
        &"汉".repeat(65),
        &format!(" {} ", "汉".repeat(65)),
    ] {
        assert!(!valid_subject_name(name), "{name:?}");
    }
}

#[test]
fn subject_fingerprint_accepts_p256_and_ed25519_spki() {
    let p256 = p256_certificate(8, "approve-subject");
    let ed25519_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("Ed25519 key");
    let ed25519 = certificate(&ed25519_key, "approve-ed25519-subject");
    for cert_der in [p256, ed25519] {
        let expected: [u8; 32] = Sha256::digest(certificate_spki(&cert_der)).into();
        assert_eq!(subject_fingerprint(&cert_der), Ok(expected));
    }
}

#[test]
fn subject_fingerprint_rejects_missing_malformed_trailing_and_rsa_certificates() {
    for (cert_der, reason) in invalid_certificates() {
        assert_eq!(subject_fingerprint(&cert_der), Err(reason));
    }
}

#[test]
fn build_approve_derives_fingerprint_and_preserves_certificate_and_name() {
    let cert_der = p256_certificate(8, "approve-subject");
    let name = format!(" {SUBJECT_NAME} ");
    let statement =
        build_approve(USER, &cert_der, &name, ISSUED_AT_US, NONCE).expect("valid approval");
    assert_eq!(statement.kind, pb::statement::Kind::ApproveDevice as i32);
    assert_eq!(statement.user, USER);
    assert_eq!(
        statement.subject_fp,
        Sha256::digest(certificate_spki(&cert_der)).as_slice()
    );
    assert_eq!(statement.subject_cert, cert_der);
    assert_eq!(statement.subject_name, name);
    assert_eq!(statement.issued_at_us, ISSUED_AT_US);
    assert_eq!(statement.nonce, NONCE);
    assert_eq!(
        build_approve(USER.to_owned(), &cert_der, &name, ISSUED_AT_US, NONCE)
            .expect("owned user approval"),
        statement
    );
}

#[test]
fn build_approve_checks_certificate_before_name_and_propagates_rejection() {
    let cert_der = p256_certificate(8, "approve-subject");
    for name in ["", "   ", "a\u{0}b", &"汉".repeat(65)] {
        assert_eq!(
            build_approve(USER, &cert_der, name, ISSUED_AT_US, NONCE),
            Err(RejectReason::InvalidStatement)
        );
    }
    for (cert_der, reason) in invalid_certificates() {
        for name in [SUBJECT_NAME, ""] {
            assert_eq!(
                build_approve(USER, &cert_der, name, ISSUED_AT_US, NONCE),
                Err(reason.clone())
            );
        }
    }
}

#[test]
fn invalid_approval_name_does_not_block_later_64_character_approval() {
    let cert_der = p256_certificate(8, "approve-subject");
    let name = "汉".repeat(64);
    let valid =
        build_approve(USER, &cert_der, &name, ISSUED_AT_US, NONCE).expect("64-character approval");
    let view = control_view(public_key_der(&signing_key(7)));
    for invalid_name in ["a\u{0}b", "", "   ", &"汉".repeat(65)] {
        let mut invalid = valid.clone();
        invalid.subject_name = invalid_name.into();
        let (applied, rejected) =
            apply_log(&view, 0, USER, &[signed(&invalid, 1), signed(&valid, 2)])
                .expect("batch continues");
        assert_eq!(
            rejected,
            vec![Rejected {
                seq: 1,
                reason: RejectReason::InvalidStatement
            }]
        );
        assert_eq!(
            applied,
            vec![Applied {
                seq: 2,
                action: TrustAction::Approve {
                    fp: subject_fingerprint(&cert_der).expect("subject fingerprint"),
                    cert_der: cert_der.clone(),
                    name: name.clone(),
                    permission: Permission::Control,
                },
            }]
        );
    }
}

#[test]
fn approval_checks_signature_then_name_then_subject_then_certificate() {
    let cert_der = p256_certificate(8, "approve-subject");
    let mut statement =
        build_approve(USER, &cert_der, SUBJECT_NAME, ISSUED_AT_US, NONCE).expect("approval");
    let fp = subject_fingerprint(&cert_der).expect("fingerprint");
    let mut view = control_view(public_key_der(&signing_key(7)));
    view.devices.insert(
        fp,
        KnownDevice {
            spki_der: certificate_spki(&cert_der),
            permission: Permission::ViewOnly,
            revoked: true,
        },
    );
    statement.subject_name = "a\u{0}b".into();
    statement.subject_cert.clear();
    let mut bad_signature = signed(&statement, 1);
    bad_signature.signature[0] ^= 1;
    let (applied, rejected) = apply_log(&view, 0, USER, &[bad_signature, signed(&statement, 2)])
        .expect("invalid approvals");
    assert!(applied.is_empty());
    assert_eq!(
        rejected,
        vec![
            Rejected {
                seq: 1,
                reason: RejectReason::InvalidSignature
            },
            Rejected {
                seq: 2,
                reason: RejectReason::InvalidStatement
            },
        ]
    );

    statement.subject_name = SUBJECT_NAME.into();
    for reason in [
        RejectReason::SubjectRevoked,
        RejectReason::AlreadyPaired,
        RejectReason::MissingCertificate,
    ] {
        let (applied, rejected) =
            apply_log(&view, 0, USER, &[signed(&statement, 1)]).expect("subject check");
        assert!(applied.is_empty());
        assert_eq!(rejected, vec![Rejected { seq: 1, reason }]);
        if let Some(subject) = view.devices.get_mut(&fp) {
            if subject.revoked {
                subject.revoked = false;
            } else {
                view.devices.remove(&fp);
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct ApproveFlow {
    description: String,
    user: String,
    subject_name: String,
    issued_at_us: u64,
    nonce_hex: String,
    signer_cert_der_hex: String,
    signer_fp_hex: String,
    subject_cert_der_hex: String,
    subject_fp_hex: String,
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
    // rcgen certificate signatures are randomized: reuse the committed DER even
    // when regenerating the statement/signature, and only create certificates once.
    let existing: Option<ApproveFlow> = match fs::read(dir.join("approve_flow.json")) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).expect("existing approve-flow JSON")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("read approve_flow.json: {error}"),
    };
    let (signer_cert, subject_cert) = match existing {
        Some(vector) => (
            unhex(&vector.signer_cert_der_hex),
            unhex(&vector.subject_cert_der_hex),
        ),
        None => (
            p256_certificate(7, "approve-flow-signer"),
            p256_certificate(8, "approve-flow-subject"),
        ),
    };
    let statement = build_approve(USER, &subject_cert, SUBJECT_NAME, ISSUED_AT_US, NONCE)
        .expect("vector approval");
    let signed = signed(&statement, 0);
    let signer_fp = subject_fingerprint(&signer_cert).expect("signer certificate fingerprint");
    assert_eq!(signed.signer_fp, signer_fp);
    let vector = ApproveFlow {
        description: "APPROVE_DEVICE signed deterministically (RFC 6979) with test-only P-256 private key [0x07; 32]; subject private key [0x08; 32] is also test-only. Never use these keys in production. Self-signed certificates were generated once with rcgen and their DER is reused on regeneration. Sign rsp-trust-v1 followed by NUL and the original statement bytes.".into(),
        user: USER.into(),
        subject_name: SUBJECT_NAME.into(),
        issued_at_us: ISSUED_AT_US,
        nonce_hex: hex(&NONCE),
        signer_cert_der_hex: hex(&signer_cert),
        signer_fp_hex: hex(&signer_fp),
        subject_cert_der_hex: hex(&subject_cert),
        subject_fp_hex: hex(&statement.subject_fp),
        statement_hex: hex(&signed.statement),
        signature_der_hex: hex(&signed.signature),
        signed_hex: hex(&signed.encode_to_vec()),
    };
    fs::create_dir_all(&dir).expect("create trust vector directory");
    fs::write(dir.join("approve_flow.bin"), signed.encode_to_vec())
        .expect("write approve-flow binary");
    let mut pretty = serde_json::to_vec_pretty(&vector).expect("serialize approve-flow JSON");
    pretty.push(b'\n');
    fs::write(dir.join("approve_flow.json"), pretty).expect("write approve-flow JSON");
}

#[test]
fn approve_flow_vector_verifies_resigns_applies_and_rejects_tampering() {
    ensure_vector_files();
    let dir = vector_dir();
    let vector: ApproveFlow = serde_json::from_slice(
        &fs::read(dir.join("approve_flow.json")).expect("read approve-flow JSON"),
    )
    .expect("parse approve-flow JSON");
    let bytes = fs::read(dir.join("approve_flow.bin")).expect("read approve-flow binary");
    let signed = pb::SignedStatement::decode(bytes.as_slice()).expect("approve-flow protobuf");
    let signer_cert = unhex(&vector.signer_cert_der_hex);
    let subject_cert = unhex(&vector.subject_cert_der_hex);
    let signer_spki = certificate_spki(&signer_cert);
    assert_eq!(signer_spki, public_key_der(&signing_key(7)));
    assert_eq!(
        certificate_spki(&subject_cert),
        public_key_der(&signing_key(8))
    );
    let (_, parsed_signer) =
        x509_parser::parse_x509_certificate(&signer_cert).expect("signer certificate");
    assert_eq!(
        parsed_signer
            .subject()
            .iter_common_name()
            .next()
            .expect("signer CN")
            .as_str()
            .expect("CN string"),
        "approve-flow-signer"
    );
    let signer_fp: [u8; 32] = Sha256::digest(&signer_spki).into();
    let subject_fp: [u8; 32] = Sha256::digest(certificate_spki(&subject_cert)).into();
    assert_eq!(vector.user, USER);
    assert_eq!(vector.subject_name, SUBJECT_NAME);
    assert_eq!(vector.issued_at_us, ISSUED_AT_US);
    assert_eq!(vector.nonce_hex, hex(&NONCE));
    assert_eq!(vector.signer_fp_hex, hex(&signer_fp));
    assert_eq!(vector.subject_fp_hex, hex(&subject_fp));
    assert_eq!(signed.signer_fp, signer_fp);
    assert_ne!(signed.signer_fp, [0x07; 32]);
    assert_eq!(vector.statement_hex, hex(&signed.statement));
    assert_eq!(vector.signature_der_hex, hex(&signed.signature));
    assert_eq!(vector.signed_hex, hex(&bytes));
    assert!(vector.description.contains("test-only"));

    let statement =
        verify(&signed, &signer_spki).expect("verify using enrolled signer certificate");
    let expected = build_approve(USER, &subject_cert, SUBJECT_NAME, ISSUED_AT_US, NONCE)
        .expect("build vector statement");
    assert_eq!(statement, expected);
    assert_eq!(statement.subject_fp, subject_fp);
    let resigned = self::signed(&statement, 0);
    assert_eq!(hex(&resigned.signature), vector.signature_der_hex);
    assert_eq!(resigned.encode_to_vec(), bytes);

    let view = control_view(signer_spki.clone());
    let logged = signed.clone();
    let logged = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: logged,
    };
    let (applied, rejected) = apply_log(&view, 0, USER, &[logged]).expect("apply vector");
    assert!(rejected.is_empty());
    assert_eq!(
        applied,
        vec![Applied {
            seq: 1,
            action: TrustAction::Approve {
                fp: subject_fp,
                cert_der: subject_cert,
                name: SUBJECT_NAME.into(),
                permission: Permission::Control,
            },
        }]
    );

    let mut tampered = bytes;
    tampered[20] ^= 1;
    let tampered = pb::SignedStatement::decode(tampered.as_slice()).expect("tampered envelope");
    assert!(matches!(
        verify(&tampered, &signer_spki),
        Err(TrustError::InvalidSignature)
    ));
    let tampered = rsp_core::trust::TrustEntry {
        seq: 1,
        signed: tampered,
    };
    let (applied, rejected) =
        apply_log(&view, 0, USER, &[tampered]).expect("reject tampered vector");
    assert!(applied.is_empty());
    // Byte 20 changes statement.user, which is checked before the signature.
    assert_eq!(
        rejected,
        vec![Rejected {
            seq: 1,
            reason: RejectReason::UserMismatch
        }]
    );
}
