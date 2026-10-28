//! 主机密钥、证书与指纹。见 docs/dev/04-identity-security.md 密钥与证书。

use p256::ecdsa::SigningKey;
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
};
use ring::{digest, rand::SecureRandom};
use rsp_transport::identity::{spki_fingerprint, CertKind, KeySource, LocalIdentity};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use thiserror::Error;

use crate::platform::{KeyStore, PlatformError};

const KEY_NAME: &str = "host-key-v1";
const CERT_NAME: &str = "host-cert-v1";

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("host key is not a valid PKCS#8 P-256 private key")]
    InvalidKey,
    #[error("host certificate generation failed: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("host certificate is invalid")]
    InvalidCertificate,
    #[error("host identity transport error: {0}")]
    Transport(#[from] rsp_transport::identity::IdentityError),
    #[error("random number generation failed")]
    Random,
}

/// The host's stable identity. The private key is intentionally omitted from
/// `Debug` and is only exposed through `local_identity` for TLS setup.
pub struct HostIdentity {
    key_der: Vec<u8>,
    cert_der: Vec<u8>,
    fp: [u8; 32],
}

impl HostIdentity {
    pub fn load_or_create(ks: &dyn KeyStore, now_us: u64) -> Result<Self, IdentityError> {
        let _ = now_us;
        let key = ks.load(KEY_NAME)?;
        let cert = ks.load(CERT_NAME)?;
        match (key, cert) {
            (None, None) => {
                let generated = LocalIdentity::generate_software(CertKind::Host)?;
                let KeySource::Software(key) = generated.key else {
                    return Err(IdentityError::InvalidKey);
                };
                let key_der = key.secret_pkcs8_der().to_vec();
                let cert_der = generated.cert_der.as_ref().to_vec();
                ks.store(KEY_NAME, &key_der)?;
                ks.store(CERT_NAME, &cert_der)?;
                Self::from_parts(key_der, cert_der)
            }
            (None, Some(_)) => Err(IdentityError::InvalidKey),
            (Some(key_der), cert_der) => {
                validate_key(&key_der)?;
                let cert_der = match cert_der {
                    Some(cert) if certificate_matches_key(&cert, &key_der) => cert,
                    _ => {
                        let cert = certificate_for_key(&key_der)?;
                        ks.store(CERT_NAME, &cert)?;
                        cert
                    }
                };
                Self::from_parts(key_der, cert_der)
            }
        }
    }

    pub fn fp(&self) -> [u8; 32] {
        self.fp
    }

    pub fn host_id(&self) -> String {
        rsp_core::ids::id_from_fp(&self.fp)
    }

    pub fn fp_display(&self) -> String {
        rsp_core::ids::fp_display(&self.fp)
    }

    pub fn local_identity(&self) -> LocalIdentity {
        LocalIdentity {
            cert_der: CertificateDer::from(self.cert_der.clone()),
            key: KeySource::Software(PrivatePkcs8KeyDer::from(self.key_der.clone())),
        }
    }

    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    fn from_parts(key_der: Vec<u8>, cert_der: Vec<u8>) -> Result<Self, IdentityError> {
        let fp = spki_fingerprint(&cert_der)?;
        Ok(Self {
            key_der,
            cert_der,
            fp,
        })
    }
}

impl std::fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostIdentity")
            .field("fp", &self.fp)
            .field("cert_der_len", &self.cert_der.len())
            .finish()
    }
}

fn validate_key(key_der: &[u8]) -> Result<(), IdentityError> {
    SigningKey::from_pkcs8_der(key_der).map_err(|_| IdentityError::InvalidKey)?;
    Ok(())
}

fn certificate_matches_key(cert_der: &[u8], key_der: &[u8]) -> bool {
    let key = match SigningKey::from_pkcs8_der(key_der) {
        Ok(key) => key,
        Err(_) => return false,
    };
    let mut fp = [0u8; 32];
    let spki = match key.verifying_key().to_public_key_der() {
        Ok(spki) => spki,
        Err(_) => return false,
    };
    fp.copy_from_slice(digest::digest(&digest::SHA256, spki.as_bytes()).as_ref());
    spki_fingerprint(cert_der).is_ok_and(|cert_fp| cert_fp == fp)
}

fn certificate_for_key(key_der: &[u8]) -> Result<Vec<u8>, IdentityError> {
    let key = KeyPair::try_from(key_der).map_err(IdentityError::Certificate)?;
    let mut params = CertificateParams::new(vec!["rs-host".to_owned()])?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, random_common_name()?);
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(365 * 20);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    Ok(params.self_signed(&key)?.der().as_ref().to_vec())
}

fn random_common_name() -> Result<String, IdentityError> {
    const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut result = String::from("h-");
    let rng = ring::rand::SystemRandom::new();
    while result.len() < 10 {
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes).map_err(|_| IdentityError::Random)?;
        for byte in bytes.into_iter().filter(|byte| *byte < 252) {
            result.push(char::from(ALPHABET[usize::from(byte % 36)]));
            if result.len() == 10 {
                return Ok(result);
            }
        }
    }
    Ok(result)
}
