#![no_main]

use std::convert::Infallible;
use std::sync::OnceLock;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use prost::Message;
use rsp_core::pb;
use rsp_core::trust::{build_bind_node, check_bind_node, sign_with, verify};
use sha2::Digest;

const SIGNER_BYTES: [u8; 32] = [0x07; 32];
const USER: &str = "vector@example";
const SUBJECT_NODE: &str = "nK7m2q4xCNTRL";
const ISSUED_AT_US: u64 = 1_792_465_134_000_000;
const NONCE: [u8; 16] = [0x55; 16];

struct TestSigner {
    key: SigningKey,
    spki: Vec<u8>,
    fp: [u8; 32],
}

fn signer() -> &'static TestSigner {
    static SIGNER: OnceLock<TestSigner> = OnceLock::new();
    SIGNER.get_or_init(|| {
        let key =
            SigningKey::from_bytes((&SIGNER_BYTES).into()).expect("fixed P-256 test key is valid");
        let spki = key
            .verifying_key()
            .to_public_key_der()
            .expect("fixed public key encoding")
            .as_bytes()
            .to_vec();
        let fp: [u8; 32] = sha2::Sha256::digest(&spki).into();
        TestSigner { key, spki, fp }
    })
}

fn sign(statement: &pb::Statement) -> pb::SignedStatement {
    sign_with(statement, signer().fp, |payload| {
        let signature: Signature = signer().key.sign(payload);
        Ok::<_, Infallible>(signature.to_der().as_bytes().to_vec())
    })
    .expect("infallible test signature")
}

fn valid_signed() -> pb::SignedStatement {
    let statement = build_bind_node(USER, signer().fp, SUBJECT_NODE, ISSUED_AT_US, NONCE)
        .expect("fixed BIND_NODE statement");
    sign(&statement)
}

fn decode_statement(signed: &pb::SignedStatement) -> Option<pb::Statement> {
    pb::Statement::decode(signed.statement.as_slice()).ok()
}

fn tamper(mut signed: pb::SignedStatement, input: &mut Unstructured<'_>) -> pb::SignedStatement {
    match input.arbitrary::<u8>().unwrap_or(0) % 7 {
        0 => {
            if !signed.signature.is_empty() {
                let index =
                    usize::from(input.arbitrary::<u8>().unwrap_or(0)) % signed.signature.len();
                signed.signature[index] ^= 1;
            }
        }
        1 => {
            if let Some(mut statement) = decode_statement(&signed) {
                statement.user.push('x');
                signed.statement = statement.encode_to_vec();
            }
        }
        2 => {
            if let Some(mut statement) = decode_statement(&signed) {
                statement.subject_node.push('/');
                signed.statement = statement.encode_to_vec();
            }
        }
        3 => {
            if let Some(mut statement) = decode_statement(&signed) {
                statement.kind = pb::statement::Kind::ApproveDevice as i32;
                signed.statement = statement.encode_to_vec();
            }
        }
        4 => {
            if let Some(mut statement) = decode_statement(&signed) {
                if input.arbitrary::<bool>().unwrap_or(false) {
                    statement.subject_cert = b"not a certificate".to_vec();
                } else {
                    statement.subject_name = "not-empty".to_owned();
                }
                signed.statement = statement.encode_to_vec();
            }
        }
        5 => {
            // Mutate a valid statement and re-sign it, exercising the paths
            // that are cryptographically valid but violate semantic rules.
            if let Some(mut statement) = decode_statement(&signed) {
                statement.subject_node.clear();
                signed = sign(&statement);
            }
        }
        _ => {
            if !signed.signer_fp.is_empty() {
                let index =
                    usize::from(input.arbitrary::<u8>().unwrap_or(0)) % signed.signer_fp.len();
                signed.signer_fp[index] ^= 1;
            }
        }
    }
    signed
}

fuzz_target!(|input: &[u8]| {
    let Some((&mode, rest)) = input.split_first() else {
        return;
    };
    let mut unstructured = Unstructured::new(rest);

    let signed = if mode & 1 == 0 {
        match pb::SignedStatement::decode(rest) {
            Ok(signed) => signed,
            Err(_) => return,
        }
    } else {
        tamper(valid_signed(), &mut unstructured)
    };
    let expected_user = if mode & 2 == 0 { Some(USER) } else { None };

    let result = check_bind_node(&signed, &signer().spki, expected_user);
    let statement = decode_statement(&signed);
    let kind = statement
        .as_ref()
        .and_then(|statement| pb::statement::Kind::try_from(statement.kind).ok());
    let independently_verified = verify(&signed, &signer().spki).is_ok();

    if let Ok(claim) = result {
        assert_eq!(
            kind,
            Some(pb::statement::Kind::BindNode),
            "check_bind_node accepted a non-BIND_NODE statement"
        );
        assert!(
            independently_verified,
            "check_bind_node accepted a statement whose signature does not verify"
        );
        let statement = statement.expect("an accepted statement must decode");
        assert_eq!(
            claim.fp.as_slice(),
            statement.subject_fp.as_slice(),
            "the accepted claim fingerprint differs from the statement"
        );
        assert_eq!(claim.node, statement.subject_node);
        assert_eq!(claim.issued_at_us, statement.issued_at_us);
        if let Some(user) = expected_user {
            assert_eq!(
                statement.user, user,
                "check_bind_node accepted a different user"
            );
        }
    }
});
