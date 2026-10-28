//! Certificate generation, SPKI fingerprints, and device-backed signing.

use std::sync::Arc;

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyUsagePurpose,
};
use ring::{digest, rand::SecureRandom};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use thiserror::Error;
use x509_parser::{
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY},
    prelude::*,
};

/// SHA-256 of the DER-encoded SubjectPublicKeyInfo, independent of certificate metadata.
pub type Fingerprint = [u8; 32];

/// Failures while reading or creating a local identity.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("invalid DER certificate")]
    InvalidCertificate,
    #[error("invalid DER SubjectPublicKeyInfo")]
    InvalidSpki,
    #[error("external signer must use an ECDSA P-256 public key")]
    UnsupportedExternalKey,
    #[error("random number generation failed")]
    Random,
    #[error("certificate validity is out of range")]
    Validity,
    #[error("certificate generation failed: {0}")]
    Certificate(#[from] rcgen::Error),
}

/// An error reported by a device-backed signing implementation.
#[derive(Debug, Error)]
#[error("external signing failed: {0}")]
pub struct SignError(pub String);

/// A P-256 device key whose private material does not leave its signer.
///
/// Callbacks are synchronous during certificate generation and the TLS handshake.
/// Implementations must avoid blocking for long periods, including interactive prompts.
pub trait ExternalSigner: Send + Sync {
    /// 用设备私钥对 TLS 1.3 CertificateVerify 的待签数据做 ECDSA-P256-SHA256，返回 DER 编码签名。
    ///
    /// The same operation also signs certificate TBS data during self-signing.
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError>;
    /// Return the complete DER-encoded SubjectPublicKeyInfo of the signing key.
    fn spki_der(&self) -> Vec<u8>;
}

/// The certificate role determines its anonymous name and lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertKind {
    Host,
    App,
}

/// Private key material, or a callback to a device-backed private key.
pub enum KeySource {
    Software(PrivatePkcs8KeyDer<'static>),
    External(Arc<dyn ExternalSigner>),
}

/// A self-signed certificate and its matching signing key.
pub struct LocalIdentity {
    pub cert_der: CertificateDer<'static>,
    pub key: KeySource,
}

impl LocalIdentity {
    /// Generate a new software P-256 key and self-signed certificate.
    pub fn generate_software(kind: CertKind) -> Result<Self, IdentityError> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let cert = certificate_params(kind)?.self_signed(&key)?;
        Ok(Self {
            cert_der: cert.der().clone(),
            key: KeySource::Software(PrivatePkcs8KeyDer::from(key.serialize_der())),
        })
    }

    /// Self-sign a certificate using the synchronous external P-256 signer.
    pub fn generate_external(
        kind: CertKind,
        signer: Arc<dyn ExternalSigner>,
    ) -> Result<Self, IdentityError> {
        let spki_der = signer.spki_der();
        let (remaining, spki) =
            SubjectPublicKeyInfo::from_der(&spki_der).map_err(|_| IdentityError::InvalidSpki)?;
        if !remaining.is_empty() {
            return Err(IdentityError::InvalidSpki);
        }
        if !is_p256(&spki) {
            return Err(IdentityError::UnsupportedExternalKey);
        }
        let remote_key = CertificateSigner {
            signer: signer.clone(),
            public_key: spki.subject_public_key.data.to_vec(),
        };
        let cert = certificate_params(kind)?.self_signed(&remote_key)?;
        Ok(Self {
            cert_der: cert.der().clone(),
            key: KeySource::External(signer),
        })
    }

    /// Return the public key fingerprint.
    ///
    /// # Panics
    /// Panics if a manually constructed identity contains an invalid DER certificate.
    /// Both generation methods always produce a valid certificate.
    pub fn fingerprint(&self) -> Fingerprint {
        spki_fingerprint(&self.cert_der)
            .expect("LocalIdentity must contain a valid DER certificate")
    }
}

/// Compute SHA-256(SubjectPublicKeyInfo DER), rejecting malformed or trailing data.
pub fn spki_fingerprint(cert_der: &[u8]) -> Result<Fingerprint, IdentityError> {
    let cert = parse_certificate(cert_der)?;
    let mut fingerprint = [0; 32];
    fingerprint.copy_from_slice(digest::digest(&digest::SHA256, cert.public_key().raw).as_ref());
    Ok(fingerprint)
}

pub(crate) fn parse_certificate(cert_der: &[u8]) -> Result<X509Certificate<'_>, IdentityError> {
    let (remaining, cert) =
        parse_x509_certificate(cert_der).map_err(|_| IdentityError::InvalidCertificate)?;
    if !remaining.is_empty() {
        return Err(IdentityError::InvalidCertificate);
    }
    Ok(cert)
}

pub(crate) fn is_p256(spki: &SubjectPublicKeyInfo<'_>) -> bool {
    spki.algorithm.algorithm == OID_KEY_TYPE_EC_PUBLIC_KEY
        && spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            .is_some_and(|curve| curve == OID_EC_P256)
        && spki.subject_public_key.unused_bits == 0
        && spki.subject_public_key.data.len() == 65
        && spki.subject_public_key.data.first() == Some(&4)
}

fn certificate_params(kind: CertKind) -> Result<CertificateParams, IdentityError> {
    let (prefix, san, years, usage) = match kind {
        CertKind::Host => ("h-", "rs-host", 20, ExtendedKeyUsagePurpose::ServerAuth),
        CertKind::App => ("a-", "rs-app", 10, ExtendedKeyUsagePurpose::ClientAuth),
    };
    let mut params = CertificateParams::new(vec![san.to_owned()])?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, random_common_name(prefix)?);
    let now = ASN1Time::now();
    params.not_before = now.to_datetime();
    // A Julian year is 365.25 days; these long-lived identity certificates do not
    // depend on calendar-year rounding or the local time zone.
    params.not_after = ASN1Time::from_timestamp(now.timestamp() + years * 31_557_600)
        .map_err(|_| IdentityError::Validity)?
        .to_datetime();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    Ok(params)
}

fn random_common_name(prefix: &str) -> Result<String, IdentityError> {
    const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let rng = ring::rand::SystemRandom::new();
    let mut name = String::from(prefix);
    while name.len() < prefix.len() + 8 {
        let mut sample = [0; 16];
        rng.fill(&mut sample).map_err(|_| IdentityError::Random)?;
        // Reject the last four byte values to avoid a modulo bias.
        for byte in sample.into_iter().filter(|byte| *byte < 252) {
            name.push(char::from(ALPHABET[usize::from(byte % 36)]));
            if name.len() == prefix.len() + 8 {
                break;
            }
        }
    }
    Ok(name)
}

// rcgen 0.14 supports remote keys through its SigningKey/PublicKeyData traits.
struct CertificateSigner {
    signer: Arc<dyn ExternalSigner>,
    public_key: Vec<u8>,
}

impl rcgen::PublicKeyData for CertificateSigner {
    fn der_bytes(&self) -> &[u8] {
        &self.public_key
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

impl rcgen::SigningKey for CertificateSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        self.signer
            .sign(msg)
            .map_err(|_| rcgen::Error::RemoteKeyError)
    }
}
