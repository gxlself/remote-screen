//! 信任声明的签名和执行规则。
//!
//! 这里不做 I/O，也不保存信任表；`DeviceView` 提供调用方当前已知的设备，
//! `apply_log` 只产生需要由调用方持久化的动作。

use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use prost::Message;
use sha2::Digest;
use thiserror::Error;
use x509_parser::{
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ED25519},
    prelude::parse_x509_certificate,
};

use crate::pb;

const SIGNING_PREFIX: &[u8] = b"rsp-trust-v1\0";

/// Maximum number of Unicode scalar values in an approved device's trimmed name.
pub const SUBJECT_NAME_MAX_CHARS: usize = 64;

/// Maximum byte length of the tailnet node StableID in a BIND_NODE statement.
pub const SUBJECT_NODE_MAX_LEN: usize = 64;

/// A BIND_NODE `subject_node`: 1–64 bytes of ASCII letters, digits, '.', '_' or '-'.
pub fn valid_subject_node(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= SUBJECT_NODE_MAX_LEN
        && node
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Check the trimmed name's length and reject controls anywhere in the original.
pub fn valid_subject_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty()
        && t.chars().count() <= SUBJECT_NAME_MAX_CHARS
        && !name.chars().any(char::is_control)
}

/// Validate an approval subject certificate and hash its SPKI DER.
pub fn subject_fingerprint(cert_der: &[u8]) -> Result<[u8; 32], RejectReason> {
    if cert_der.is_empty() {
        return Err(RejectReason::MissingCertificate);
    }
    let (_, cert) = match parse_x509_certificate(cert_der) {
        Ok(parsed) if parsed.0.is_empty() => parsed,
        _ => return Err(RejectReason::SubjectCertMismatch),
    };
    let spki = cert.public_key();
    let is_p256 = spki.algorithm.algorithm == OID_KEY_TYPE_EC_PUBLIC_KEY
        && spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            .is_some_and(|curve| curve == OID_EC_P256)
        && spki.subject_public_key.unused_bits == 0
        && spki.subject_public_key.data.len() == 65
        && spki.subject_public_key.data.first() == Some(&4);
    let is_ed25519 = spki.algorithm.algorithm == OID_SIG_ED25519
        && spki.algorithm.parameters.is_none()
        && spki.subject_public_key.unused_bits == 0
        && spki.subject_public_key.data.len() == 32;
    if !is_p256 && !is_ed25519 {
        return Err(RejectReason::UnsupportedSubjectKey);
    }
    Ok(sha2::Sha256::digest(spki.raw).into())
}

/// Extract the exact SubjectPublicKeyInfo DER from a certificate.
pub fn spki_der_from_cert(cert_der: &[u8]) -> Result<Vec<u8>, RejectReason> {
    let (_, cert) = match parse_x509_certificate(cert_der) {
        Ok(parsed) if parsed.0.is_empty() => parsed,
        _ => return Err(RejectReason::SubjectCertMismatch),
    };
    Ok(cert.public_key().raw.to_vec())
}

/// Build an approval using the fingerprint derived from the subject certificate.
pub fn build_approve(
    user: impl Into<String>,
    subject_cert_der: &[u8],
    subject_name: &str,
    issued_at_us: u64,
    nonce: [u8; 16],
) -> Result<pb::Statement, RejectReason> {
    let subject_fp = subject_fingerprint(subject_cert_der)?;
    if !valid_subject_name(subject_name) {
        return Err(RejectReason::InvalidStatement);
    }
    Ok(build_statement(
        pb::statement::Kind::ApproveDevice,
        user,
        subject_fp,
        subject_cert_der,
        subject_name,
        issued_at_us,
        nonce,
    ))
}

/// Build a BIND_NODE statement: device `subject_fp` declares `subject_node` as
/// its second tailnet node. The device signs it with its own key
/// (signer_fp = subject_fp); subject_cert and subject_name stay empty.
pub fn build_bind_node(
    user: impl Into<String>,
    subject_fp: [u8; 32],
    subject_node: &str,
    issued_at_us: u64,
    nonce: [u8; 16],
) -> Result<pb::Statement, RejectReason> {
    if !valid_subject_node(subject_node) {
        return Err(RejectReason::InvalidStatement);
    }
    let mut statement = build_statement(
        pb::statement::Kind::BindNode,
        user,
        subject_fp,
        [],
        "",
        issued_at_us,
        nonce,
    );
    statement.subject_node = subject_node.to_owned();
    Ok(statement)
}

/// Build the bytes covered by a trust statement signature.
pub fn signing_payload(statement_bytes: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(SIGNING_PREFIX.len() + statement_bytes.len());
    payload.extend_from_slice(SIGNING_PREFIX);
    payload.extend_from_slice(statement_bytes);
    payload
}

/// Construct a protobuf trust statement.
pub fn build_statement(
    kind: pb::statement::Kind,
    user: impl Into<String>,
    subject_fp: [u8; 32],
    subject_cert: impl AsRef<[u8]>,
    subject_name: impl Into<String>,
    issued_at_us: u64,
    nonce: [u8; 16],
) -> pb::Statement {
    pb::Statement {
        kind: kind as i32,
        user: user.into(),
        subject_fp: subject_fp.to_vec(),
        subject_cert: subject_cert.as_ref().to_vec(),
        subject_name: subject_name.into(),
        issued_at_us,
        nonce: nonce.to_vec(),
        subject_node: String::new(),
    }
}

/// Encode and sign a statement. The signing operation is deliberately supplied
/// by the caller so that private keys can remain in a platform security chip.
pub fn sign_with<E>(
    statement: &pb::Statement,
    signer_fp: [u8; 32],
    sign: impl FnOnce(&[u8]) -> Result<Vec<u8>, E>,
) -> Result<pb::SignedStatement, E> {
    let statement_bytes = statement.encode_to_vec();
    let signature = sign(&signing_payload(&statement_bytes))?;
    Ok(pb::SignedStatement {
        statement: statement_bytes,
        signer_fp: signer_fp.to_vec(),
        signature,
    })
}

/// A device statement with a local journal sequence. The sequence is not sent
/// over the wire and is not part of the signed statement.
#[derive(Clone, Debug)]
pub struct TrustEntry {
    pub seq: u64,
    pub signed: pb::SignedStatement,
}

impl std::ops::Deref for TrustEntry {
    type Target = pb::SignedStatement;

    fn deref(&self) -> &Self::Target {
        &self.signed
    }
}

impl std::ops::DerefMut for TrustEntry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.signed
    }
}

/// Errors encountered while decoding and verifying a signed statement, or
/// while checking the sequence of a trust log.
#[derive(Debug, Error)]
pub enum TrustError {
    #[error("invalid signer public key")]
    InvalidPublicKey,
    #[error("invalid ECDSA signature")]
    InvalidSignature,
    #[error("invalid statement protobuf: {0}")]
    InvalidStatement(#[source] prost::DecodeError),
    #[error("trust log sequence gap: expected {expected}, got {got}")]
    Gap { expected: u64, got: u64 },
}

/// Decode a statement only after verifying its ECDSA-P256-SHA256 signature.
pub fn verify(
    signed: &pb::SignedStatement,
    signer_spki_der: &[u8],
) -> Result<pb::Statement, TrustError> {
    let key = VerifyingKey::from_public_key_der(signer_spki_der)
        .map_err(|_| TrustError::InvalidPublicKey)?;
    let signature =
        Signature::from_der(&signed.signature).map_err(|_| TrustError::InvalidSignature)?;
    key.verify(&signing_payload(&signed.statement), &signature)
        .map_err(|_| TrustError::InvalidSignature)?;
    pb::Statement::decode(signed.statement.as_slice()).map_err(TrustError::InvalidStatement)
}

/// Permission inherited by a newly approved device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    ViewOnly,
    Control,
    Unattended,
}

/// Device data needed to authenticate an approval signer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownDevice {
    pub spki_der: Vec<u8>,
    pub permission: Permission,
    pub revoked: bool,
}

/// Read-only view of devices known by the host.
pub trait DeviceView {
    fn device(&self, fp: &[u8; 32]) -> Option<KnownDevice>;
}

/// A trust statement that passed all checks and is ready for host persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustAction {
    Revoke {
        fp: [u8; 32],
    },
    RequireConfirm {
        fp: [u8; 32],
    },
    Approve {
        fp: [u8; 32],
        cert_der: Vec<u8>,
        name: String,
        permission: Permission,
    },
    BindNode {
        fp: [u8; 32],
        node: String,
        issued_at_us: u64,
    },
}

/// One accepted log entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub seq: u64,
    pub action: TrustAction,
}

/// Why one log entry was rejected while processing continued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    InvalidStatement,
    InvalidSubjectFingerprint,
    InvalidSignerFingerprint,
    InvalidSignature,
    InvalidSignerKey,
    MissingSignature,
    UnknownSigner,
    SignerRevoked,
    InsufficientPermission,
    UserMismatch,
    MissingSigner,
    UnknownKind,
    MissingCertificate,
    SubjectCertMismatch,
    UnsupportedSubjectKey,
    SubjectRevoked,
    AlreadyPaired,
    BindSignerMismatch,
    StaleBinding,
}

/// One rejected log entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub seq: u64,
    pub reason: RejectReason,
}

/// A verified device declaration of its second tailnet node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindNodeClaim {
    pub fp: [u8; 32],
    pub node: String,
    pub issued_at_us: u64,
}

/// Validate a self-signed BIND_NODE with the same rejection precedence as
/// `apply_log`. The caller supplies the trusted signer's public key and decides
/// whether that signer is paired and unrevoked.
pub fn check_bind_node(
    signed: &pb::SignedStatement,
    signer_spki_der: &[u8],
    expected_user: Option<&str>,
) -> Result<BindNodeClaim, RejectReason> {
    let statement = pb::Statement::decode(signed.statement.as_slice())
        .map_err(|_| RejectReason::InvalidStatement)?;
    let kind =
        pb::statement::Kind::try_from(statement.kind).map_err(|_| RejectReason::UnknownKind)?;
    if kind != pb::statement::Kind::BindNode {
        return Err(RejectReason::InvalidStatement);
    }
    if expected_user.is_some_and(|user| statement.user != user) {
        return Err(RejectReason::UserMismatch);
    }
    let fp = to_fp(&statement.subject_fp).ok_or(RejectReason::InvalidSubjectFingerprint)?;
    if signed.signer_fp.is_empty() {
        return Err(RejectReason::MissingSigner);
    }
    let signer_fp = to_fp(&signed.signer_fp).ok_or(RejectReason::InvalidSignerFingerprint)?;
    if signer_fp != fp {
        return Err(RejectReason::BindSignerMismatch);
    }
    if signed.signature.is_empty() {
        return Err(RejectReason::MissingSignature);
    }
    verify(signed, signer_spki_der).map_err(|error| match error {
        TrustError::InvalidPublicKey => RejectReason::InvalidSignerKey,
        _ => RejectReason::InvalidSignature,
    })?;
    if !valid_subject_node(&statement.subject_node)
        || !statement.subject_cert.is_empty()
        || !statement.subject_name.is_empty()
    {
        return Err(RejectReason::InvalidStatement);
    }
    Ok(BindNodeClaim {
        fp,
        node: statement.subject_node,
        issued_at_us: statement.issued_at_us,
    })
}

/// Apply a locally sequenced journal of authenticated device statements.
pub fn apply_log(
    view: &dyn DeviceView,
    last_applied_seq: u64,
    user: &str,
    log: &[TrustEntry],
) -> Result<(Vec<Applied>, Vec<Rejected>), TrustError> {
    // Sorting references keeps the caller's log untouched and makes duplicate
    // sequence handling deterministic (sort_by is stable).
    let mut entries: Vec<&TrustEntry> = log.iter().collect();
    entries.sort_by_key(|entry| entry.seq);

    let mut expected = last_applied_seq.saturating_add(1);
    let mut applied = Vec::new();
    let mut rejected = Vec::new();
    let mut revoked_in_batch = std::collections::HashSet::<[u8; 32]>::new();
    let mut approved_in_batch = std::collections::HashMap::<[u8; 32], Vec<u8>>::new();

    for signed in entries {
        let seq = signed.seq;
        if seq <= last_applied_seq || seq < expected {
            continue;
        }
        if seq > expected {
            return Err(TrustError::Gap { expected, got: seq });
        }
        expected = expected.saturating_add(1);

        let statement = match pb::Statement::decode(signed.statement.as_slice()) {
            Ok(statement) => statement,
            Err(_) => {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::InvalidStatement,
                });
                continue;
            }
        };

        let kind = match pb::statement::Kind::try_from(statement.kind) {
            Ok(kind) => kind,
            Err(_) => {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::UnknownKind,
                });
                continue;
            }
        };

        // REVOKE_DEVICE is a global safety fact for every Host; only other
        // statement kinds are scoped to the authenticated user's trust view.
        // See 04 · 信任声明.
        if kind != pb::statement::Kind::RevokeDevice && statement.user != user {
            rejected.push(Rejected {
                seq,
                reason: RejectReason::UserMismatch,
            });
            continue;
        }

        let subject_fp = match to_fp(&statement.subject_fp) {
            Some(fp) => fp,
            None => {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::InvalidSubjectFingerprint,
                });
                continue;
            }
        };

        let Some(signer_fp) = to_fp(&signed.signer_fp) else {
            rejected.push(Rejected {
                seq,
                reason: if signed.signer_fp.is_empty() {
                    RejectReason::MissingSigner
                } else {
                    RejectReason::InvalidSignerFingerprint
                },
            });
            continue;
        };

        if kind == pb::statement::Kind::ApproveDevice {
            if signed.signature.is_empty() {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::MissingSignature,
                });
                continue;
            }
            if revoked_in_batch.contains(&signer_fp) {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SignerRevoked,
                });
                continue;
            }
            let signer = match view.device(&signer_fp) {
                Some(device) => device,
                None => {
                    rejected.push(Rejected {
                        seq,
                        reason: RejectReason::UnknownSigner,
                    });
                    continue;
                }
            };
            if signer.revoked {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SignerRevoked,
                });
                continue;
            }
            if signer.permission < Permission::Control {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::InsufficientPermission,
                });
                continue;
            }
            if verify(signed, &signer.spki_der).is_err() {
                let reason = match VerifyingKey::from_public_key_der(&signer.spki_der) {
                    Ok(_) => RejectReason::InvalidSignature,
                    Err(_) => RejectReason::InvalidSignerKey,
                };
                rejected.push(Rejected { seq, reason });
                continue;
            }
            if !valid_subject_name(&statement.subject_name) {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::InvalidStatement,
                });
                continue;
            }
            let subject = view.device(&subject_fp);
            if revoked_in_batch.contains(&subject_fp)
                || subject.as_ref().is_some_and(|device| device.revoked)
            {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SubjectRevoked,
                });
                continue;
            }
            // Approval only adds a new pairing. Repeating it must never replace
            // an existing device's permission, name or certificate.
            if subject.is_some() || approved_in_batch.contains_key(&subject_fp) {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::AlreadyPaired,
                });
                continue;
            }
            let certificate_fp = match subject_fingerprint(&statement.subject_cert) {
                Ok(fp) => fp,
                Err(reason) => {
                    rejected.push(Rejected { seq, reason });
                    continue;
                }
            };
            if certificate_fp != subject_fp {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SubjectCertMismatch,
                });
                continue;
            }
            let spki = match spki_der_from_cert(&statement.subject_cert) {
                Ok(spki) => spki,
                Err(reason) => {
                    rejected.push(Rejected { seq, reason });
                    continue;
                }
            };
            approved_in_batch.insert(subject_fp, spki);
            applied.push(Applied {
                seq,
                action: TrustAction::Approve {
                    fp: subject_fp,
                    cert_der: statement.subject_cert,
                    name: statement.subject_name,
                    permission: signer.permission,
                },
            });
            continue;
        }

        if kind == pb::statement::Kind::BindNode {
            if signer_fp != subject_fp {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::BindSignerMismatch,
                });
                continue;
            }
            if signed.signature.is_empty() {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::MissingSignature,
                });
                continue;
            }
            if revoked_in_batch.contains(&signer_fp) {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SignerRevoked,
                });
                continue;
            }
            let spki = match view.device(&signer_fp) {
                Some(device) if device.revoked => {
                    rejected.push(Rejected {
                        seq,
                        reason: RejectReason::SignerRevoked,
                    });
                    continue;
                }
                Some(device) => device.spki_der,
                None => match approved_in_batch.get(&signer_fp) {
                    Some(spki) => spki.clone(),
                    None => {
                        rejected.push(Rejected {
                            seq,
                            reason: RejectReason::UnknownSigner,
                        });
                        continue;
                    }
                },
            };
            let claim = match check_bind_node(signed, &spki, None) {
                Ok(claim) => claim,
                Err(reason) => {
                    rejected.push(Rejected { seq, reason });
                    continue;
                }
            };
            applied.push(Applied {
                seq,
                action: TrustAction::BindNode {
                    fp: claim.fp,
                    node: claim.node,
                    issued_at_us: claim.issued_at_us,
                },
            });
            continue;
        }

        let signer = match view.device(&signer_fp) {
            Some(device) if !device.revoked && !revoked_in_batch.contains(&signer_fp) => device,
            Some(_) => {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::SignerRevoked,
                });
                continue;
            }
            None => {
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::UnknownSigner,
                });
                continue;
            }
        };
        if signed.signature.is_empty() {
            rejected.push(Rejected {
                seq,
                reason: RejectReason::MissingSignature,
            });
            continue;
        }
        if signer.permission < Permission::Control {
            rejected.push(Rejected {
                seq,
                reason: RejectReason::InsufficientPermission,
            });
            continue;
        }
        if verify(signed, &signer.spki_der).is_err() {
            rejected.push(Rejected {
                seq,
                reason: RejectReason::InvalidSignature,
            });
            continue;
        }

        match kind {
            pb::statement::Kind::RevokeDevice => {
                revoked_in_batch.insert(subject_fp);
                applied.push(Applied {
                    seq,
                    action: TrustAction::Revoke { fp: subject_fp },
                });
            }
            pb::statement::Kind::RequireConfirm => {
                if !revoked_in_batch.contains(&subject_fp)
                    && (approved_in_batch.contains_key(&subject_fp)
                        || view
                            .device(&subject_fp)
                            .is_some_and(|device| !device.revoked))
                {
                    applied.push(Applied {
                        seq,
                        action: TrustAction::RequireConfirm { fp: subject_fp },
                    });
                }
            }
            pb::statement::Kind::Unspecified | pb::statement::Kind::ApproveDevice => {
                // APPROVE_DEVICE was handled above; UNSPECIFIED was handled by
                // the enum conversion branch only for known values.
                rejected.push(Rejected {
                    seq,
                    reason: RejectReason::UnknownKind,
                });
            }
            // BIND_NODE, like APPROVE_DEVICE, was handled above.
            pb::statement::Kind::BindNode => unreachable!("BIND_NODE was handled above"),
        }
    }

    Ok((applied, rejected))
}

fn to_fp(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}
