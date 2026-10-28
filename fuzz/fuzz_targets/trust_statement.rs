#![no_main]

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use prost::Message;
use rcgen::PublicKeyData;
use rsp_core::pb;
use rsp_core::trust::{
    apply_log, build_statement, signing_payload, verify, DeviceView, KnownDevice, Permission,
    TrustAction, TrustEntry, TrustError,
};
use sha2::Digest;

const SIGNER_BYTES: [u8; 32] = [0x07; 32];
const SIGNER_FP: [u8; 32] = [
    0xce, 0x97, 0xec, 0x20, 0xd4, 0x36, 0x61, 0xa1, 0x9b, 0x21, 0x5c, 0xb2, 0xe3, 0x76, 0xa6, 0x98,
    0xd0, 0xb4, 0xaa, 0x2a, 0xa5, 0xb4, 0xc7, 0x85, 0x4e, 0x91, 0x6b, 0xd0, 0x50, 0x82, 0xfc, 0xee,
];
const SUBJECT_FP: [u8; 32] = [
    0x67, 0xfb, 0xb7, 0x5b, 0x4b, 0x14, 0x3a, 0x9c, 0x9b, 0xfa, 0xa3, 0x2d, 0x4a, 0x95, 0xfc, 0x41,
    0xbf, 0xf3, 0xe9, 0x3e, 0xb5, 0x9a, 0xd2, 0x21, 0x75, 0x63, 0x52, 0xc9, 0x2c, 0x71, 0x1c, 0xfe,
];
const SUBJECT_PKCS8_DER: &[u8] = &[
    0x30, 0x81, 0x87, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02,
    0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x6d, 0x30, 0x6b, 0x02,
    0x01, 0x01, 0x04, 0x20, 0x16, 0xa0, 0x4b, 0xc9, 0x0e, 0x2a, 0x99, 0xb4, 0x6c, 0x3f, 0xa4, 0xc7,
    0x55, 0x8c, 0xc0, 0x95, 0xe7, 0x09, 0x1d, 0x37, 0x1c, 0xd3, 0xf7, 0xe0, 0x6c, 0x6d, 0x17, 0x8b,
    0x0f, 0xcc, 0x53, 0xda, 0xa1, 0x44, 0x03, 0x42, 0x00, 0x04, 0xf8, 0x93, 0x9f, 0xf4, 0x0f, 0x09,
    0x56, 0x43, 0x62, 0xe5, 0x2e, 0xd6, 0x00, 0x06, 0x02, 0x33, 0x7f, 0x4c, 0xbf, 0x48, 0x16, 0x11,
    0x93, 0xa1, 0x4c, 0x76, 0x0b, 0x30, 0xa5, 0xcf, 0x9e, 0x9e, 0xa9, 0x31, 0x34, 0xa2, 0x3d, 0x6e,
    0x6c, 0xbc, 0x7f, 0x3e, 0x74, 0xd4, 0xef, 0xb2, 0xaa, 0x11, 0xbf, 0xbb, 0xc3, 0x18, 0xb8, 0x80,
    0x03, 0x1e, 0x26, 0xaa, 0x12, 0x60, 0x9d, 0x61, 0xfb, 0x3a,
];
const USERS: [&str; 2] = ["alice", "bob"];
const PERMISSIONS: [Permission; 3] = [
    Permission::ViewOnly,
    Permission::Control,
    Permission::Unattended,
];

struct Materials {
    signer_key: SigningKey,
    signer_spki: Vec<u8>,
    subject_cert: Vec<u8>,
}

/// The certificate signature bytes are randomized by rcgen on every process
/// start, but the certificate's SPKI comes from the fixed private key below.
/// `SUBJECT_FP` is therefore stable, so fuzz results stay reproducible.
fn materials() -> &'static Materials {
    static MATERIALS: OnceLock<Materials> = OnceLock::new();
    MATERIALS.get_or_init(|| {
        let signer_key =
            SigningKey::from_bytes((&SIGNER_BYTES).into()).expect("fixed signer key is valid");
        let signer_spki = signer_key
            .verifying_key()
            .to_public_key_der()
            .expect("fixed public key encoding")
            .as_bytes()
            .to_vec();
        assert_eq!(
            sha2::Sha256::digest(&signer_spki).as_slice(),
            SIGNER_FP,
            "signer fingerprint must be SHA-256(SPKI)"
        );

        let subject_key =
            rcgen::KeyPair::try_from(SUBJECT_PKCS8_DER).expect("fixed subject PKCS#8 key is valid");
        let subject_spki = subject_key.subject_public_key_info();
        assert_eq!(
            sha2::Sha256::digest(&subject_spki).as_slice(),
            SUBJECT_FP,
            "subject fingerprint must be SHA-256(SPKI)"
        );
        let subject_cert = rcgen::CertificateParams::new(vec!["rs-app".to_owned()])
            .expect("fixed subject certificate params")
            .self_signed(&subject_key)
            .expect("fixed subject certificate")
            .der()
            .to_vec();

        Materials {
            signer_key,
            signer_spki,
            subject_cert,
        }
    })
}

#[derive(Default)]
struct View {
    devices: HashMap<[u8; 32], KnownDevice>,
}

impl DeviceView for View {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice> {
        self.devices.get(fp).cloned()
    }
}

fn pick_permission(input: &mut Unstructured<'_>) -> Permission {
    let index = usize::from(input.arbitrary::<u8>().unwrap_or(0)) % PERMISSIONS.len();
    PERMISSIONS[index]
}

fn random_bytes(input: &mut Unstructured<'_>, max_len: usize) -> Vec<u8> {
    let len = usize::from(input.arbitrary::<u8>().unwrap_or(0)) % (max_len + 1);
    input.bytes(len).map(<[u8]>::to_vec).unwrap_or_default()
}

fn random_text(input: &mut Unstructured<'_>) -> String {
    let len = usize::from(input.arbitrary::<u8>().unwrap_or(0)) % 24;
    let bytes = input.bytes(len).unwrap_or(&[]);
    bytes
        .iter()
        .map(|byte| char::from(b'a' + byte % 26))
        .collect()
}

fn statement_bytes(input: &mut Unstructured<'_>, round_user: &str) -> Vec<u8> {
    if input.arbitrary::<bool>().unwrap_or(false) {
        return random_bytes(input, 96);
    }

    let kind = match input.arbitrary::<u8>().unwrap_or(0) % 5 {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    };
    let user = match input.arbitrary::<u8>().unwrap_or(0) % 4 {
        0 => round_user.to_owned(),
        1 => USERS[usize::from(input.arbitrary::<u8>().unwrap_or(0)) % USERS.len()].to_owned(),
        2 => String::new(),
        _ => random_text(input),
    };
    let subject_fp = if input.arbitrary::<bool>().unwrap_or(false) {
        SUBJECT_FP
    } else {
        input.arbitrary::<[u8; 32]>().unwrap_or([0x22; 32])
    };
    let cert = match input.arbitrary::<u8>().unwrap_or(0) % 3 {
        0 => materials().subject_cert.clone(),
        1 => Vec::new(),
        _ => random_bytes(input, 96),
    };
    let name = match input.arbitrary::<u8>().unwrap_or(0) % 3 {
        0 => "device".to_owned(),
        1 => String::new(),
        _ => random_text(input),
    };
    let issued_at_us = input.arbitrary::<u64>().unwrap_or(0);
    let nonce = input.arbitrary::<[u8; 16]>().unwrap_or([0x33; 16]);

    let mut statement = build_statement(
        pb::statement::Kind::Unspecified,
        user,
        subject_fp,
        cert,
        name,
        issued_at_us,
        nonce,
    );
    statement.kind = kind;
    statement.encode_to_vec()
}

fn signer_fp(input: &mut Unstructured<'_>) -> Vec<u8> {
    match input.arbitrary::<u8>().unwrap_or(0) % 4 {
        0 => Vec::new(),
        1 => SIGNER_FP.to_vec(),
        2 => random_bytes(input, 32),
        _ => input.arbitrary::<[u8; 32]>().unwrap_or([0x44; 32]).to_vec(),
    }
}

fn signature(input: &mut Unstructured<'_>, statement: &[u8]) -> Vec<u8> {
    match input.arbitrary::<u8>().unwrap_or(0) % 3 {
        0 => {
            let signature: Signature = materials().signer_key.sign(&signing_payload(statement));
            signature.to_der().as_bytes().to_vec()
        }
        1 => Vec::new(),
        _ => random_bytes(input, 80),
    }
}

fn check_success_invariants(
    sorted_entries: &[&TrustEntry],
    last_applied_seq: u64,
    signer_present: bool,
    signer_revoked: bool,
    signer_permission: Permission,
    applied: &[rsp_core::trust::Applied],
    rejected: &[rsp_core::trust::Rejected],
) {
    let mut previous = last_applied_seq;
    for entry in applied {
        assert!(
            entry.seq > previous,
            "applied sequences must strictly increase"
        );
        previous = entry.seq;
    }
    previous = last_applied_seq;
    for entry in rejected {
        assert!(
            entry.seq > previous,
            "rejected sequences must strictly increase"
        );
        previous = entry.seq;
    }

    let applied_seqs: HashSet<u64> = applied.iter().map(|entry| entry.seq).collect();
    let rejected_seqs: HashSet<u64> = rejected.iter().map(|entry| entry.seq).collect();
    assert!(
        applied_seqs.is_disjoint(&rejected_seqs),
        "one sequence was both applied and rejected"
    );

    for entry in applied {
        let TrustAction::Approve { fp, cert_der, .. } = &entry.action else {
            continue;
        };
        let source = sorted_entries
            .iter()
            .find(|source| source.seq == entry.seq)
            .expect("an applied sequence must exist in the input");
        assert!(
            !source.signer_fp.is_empty(),
            "an empty signer fingerprint cannot be approved"
        );
        assert_eq!(
            source.signer_fp, SIGNER_FP,
            "approval must be signed by the fixed signer"
        );
        assert!(signer_present, "the approval signer must be known");
        assert!(!signer_revoked, "the approval signer must not be revoked");
        assert!(
            signer_permission >= Permission::Control,
            "the approval signer needs at least Control permission"
        );
        assert!(
            verify(source, &materials().signer_spki).is_ok(),
            "an approved input must verify"
        );

        let statement = pb::Statement::decode(source.statement.as_slice())
            .expect("an approved input must contain a valid statement");
        assert_eq!(statement.subject_cert, materials().subject_cert);
        assert_eq!(statement.subject_fp.as_slice(), SUBJECT_FP);
        assert_eq!(fp, &SUBJECT_FP);
        assert_eq!(cert_der, &materials().subject_cert);
    }
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    let materials = materials();
    let last_applied_seq = u64::from(input.arbitrary::<u8>().unwrap_or(0) % 9);
    let user = USERS[usize::from(input.arbitrary::<u8>().unwrap_or(0)) % USERS.len()];

    let signer_present = input.arbitrary::<bool>().unwrap_or(true);
    let signer_revoked = input.arbitrary::<bool>().unwrap_or(false);
    let signer_permission = pick_permission(&mut input);
    let subject_present = input.arbitrary::<bool>().unwrap_or(false);
    let subject_revoked = input.arbitrary::<bool>().unwrap_or(false);

    let mut view = View::default();
    if signer_present {
        view.devices.insert(
            SIGNER_FP,
            KnownDevice {
                spki_der: materials.signer_spki.clone(),
                permission: signer_permission,
                revoked: signer_revoked,
            },
        );
    }
    if subject_present {
        view.devices.insert(
            SUBJECT_FP,
            KnownDevice {
                spki_der: Vec::new(),
                permission: pick_permission(&mut input),
                revoked: subject_revoked,
            },
        );
    }

    let entry_count = usize::from(input.arbitrary::<u8>().unwrap_or(0) % 17);
    let mut entries = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let statement = statement_bytes(&mut input, user);
        let entry = TrustEntry {
            signed: pb::SignedStatement {
                signer_fp: signer_fp(&mut input),
                signature: signature(&mut input, &statement),
                statement,
            },
            seq: if input.arbitrary::<bool>().unwrap_or(true) {
                last_applied_seq + 1 + index as u64
            } else {
                u64::from(input.arbitrary::<u8>().unwrap_or(0) % 16)
            },
        };

        let _ = verify(&entry, &materials.signer_spki);
        let random_key = random_bytes(&mut input, 96);
        let _ = verify(&entry, &random_key);
        entries.push(entry);
    }

    let result = apply_log(&view, last_applied_seq, user, &entries);
    if let Err(TrustError::Gap { expected, got }) = &result {
        assert!(got > expected, "a gap must move the sequence forward");
        assert!(
            entries.iter().any(|entry| entry.seq == *got),
            "the gap sequence must exist in the input"
        );
        assert!(
            !entries.iter().any(|entry| entry.seq == *expected),
            "the expected sequence must be missing from the input"
        );
    }

    if let Ok((applied, rejected)) = &result {
        let mut sorted_entries: Vec<&TrustEntry> = entries.iter().collect();
        sorted_entries.sort_by_key(|entry| entry.seq);
        check_success_invariants(
            &sorted_entries,
            last_applied_seq,
            signer_present,
            signer_revoked,
            signer_permission,
            applied,
            rejected,
        );
    }
});
