//! TLS 1.3 mutual authentication with SPKI pinning and device-backed signing.
//!
//! Certificate verification establishes possession of a key, not authorization.
//! The Host must pass the peer fingerprint to the application authorization layer.

use std::{fmt, sync::Arc};

use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    sign::{CertifiedKey, Signer, SigningKey, SingleCertAndKey},
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, OtherError,
    ServerConfig, SignatureAlgorithm, SignatureScheme,
};
use thiserror::Error;
use x509_parser::oid_registry::OID_SIG_ED25519;

use crate::identity::{
    is_p256, parse_certificate, spki_fingerprint, ExternalSigner, Fingerprint, IdentityError,
    KeySource, LocalIdentity,
};

pub const ALPN_RSP1: &[u8] = b"rsp/1";

/// The expected Host public key, or the temporary six-digit pairing mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerPin {
    Fingerprint(Fingerprint),
    /// Only for six-digit pairing, where SPAKE2 must bind the TLS fingerprints
    /// into its transcript before the connection is trusted.
    Any,
}

/// Whether a server asks the peer for a client certificate.  `Required` is
/// the protocol default; `Optional` exists only for the host's explicit
/// development mode where pre-P2 clients do not have an identity yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientAuth {
    Required,
    Optional,
}

/// Failures while configuring a local TLS identity.
#[derive(Debug, Error)]
pub enum TlsError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Rustls(#[from] Error),
}

/// Handshake failures relevant to the dialer's user-facing error classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeFailure {
    FingerprintMismatch {
        expected: Fingerprint,
        got: Fingerprint,
    },
    NoPeerCertificate,
    Other,
}

/// Configure the Host to require a client certificate and verify possession of
/// its P-256 or Ed25519 key. Pairing/authorization is performed after TLS.
pub fn server_config(local: &LocalIdentity) -> Result<ServerConfig, TlsError> {
    server_config_with_client_auth(local, ClientAuth::Required)
}

/// Configure a server certificate and device certificate verifier.
pub fn server_config_with_client_auth(
    local: &LocalIdentity,
    client_auth: ClientAuth,
) -> Result<ServerConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = certified_key(local, &provider)?;
    let mut config = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(DeviceClientVerifier {
            provider,
            mandatory: matches!(client_auth, ClientAuth::Required),
        }))
        .with_cert_resolver(Arc::new(SingleCertAndKey::from(key)));
    config.alpn_protocols = vec![ALPN_RSP1.to_vec()];
    Ok(config)
}

/// Configure a client certificate and Host SPKI pin, without DNS or CA checks.
pub fn client_config(local: &LocalIdentity, pin: ServerPin) -> Result<ClientConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = certified_key(local, &provider)?;
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerVerifier { pin, provider }))
        .with_client_cert_resolver(Arc::new(SingleCertAndKey::from(key)));
    config.alpn_protocols = vec![ALPN_RSP1.to_vec()];
    Ok(config)
}

/// Extract the leaf SPKI fingerprint from an authenticated peer certificate chain.
/// This accepts both quinn's peer identity and rustls's peer certificates.
pub fn peer_fingerprint(certs: &[CertificateDer<'_>]) -> Option<Fingerprint> {
    certs.first().and_then(|cert| spki_fingerprint(cert).ok())
}

/// Classify a local rustls handshake error. A pin mismatch retains both fingerprints
/// so the dialer can distinguish RS-E150 without parsing error strings.
pub fn classify(err: &Error) -> HandshakeFailure {
    match err {
        Error::InvalidCertificate(CertificateError::Other(other)) => {
            match other.0.downcast_ref::<FingerprintMismatch>() {
                Some(mismatch) => HandshakeFailure::FingerprintMismatch {
                    expected: mismatch.expected,
                    got: mismatch.got,
                },
                None => HandshakeFailure::Other,
            }
        }
        Error::NoCertificatesPresented
        | Error::AlertReceived(rustls::AlertDescription::CertificateRequired) => {
            HandshakeFailure::NoPeerCertificate
        }
        _ => HandshakeFailure::Other,
    }
}

fn certified_key(
    local: &LocalIdentity,
    provider: &CryptoProvider,
) -> Result<CertifiedKey, TlsError> {
    parse_certificate(&local.cert_der)?;
    let key = match &local.key {
        KeySource::Software(key) => CertifiedKey::from_der(
            vec![local.cert_der.clone()],
            PrivateKeyDer::Pkcs8(key.clone_key()),
            provider,
        )?,
        KeySource::External(signer) => {
            let signing_key = ExternalSigningKey {
                signer: signer.clone(),
                spki: signer.spki_der(),
            };
            let key = CertifiedKey::new(vec![local.cert_der.clone()], Arc::new(signing_key));
            key.keys_match()?;
            key
        }
    };
    Ok(key)
}

#[derive(Debug, Error)]
#[error("Host SPKI fingerprint mismatch")]
struct FingerprintMismatch {
    expected: Fingerprint,
    got: Fingerprint,
}

#[derive(Debug)]
struct PinnedServerVerifier {
    pin: ServerPin,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let got = spki_fingerprint(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if let ServerPin::Fingerprint(expected) = self.pin {
            if expected != got {
                return Err(Error::InvalidCertificate(CertificateError::Other(
                    OtherError(Arc::new(FingerprintMismatch { expected, got })),
                )));
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("RSP requires TLS 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct DeviceClientVerifier {
    provider: Arc<CryptoProvider>,
    mandatory: bool,
}

impl ClientCertVerifier for DeviceClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        let cert = parse_certificate(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        let spki = cert.public_key();
        let is_ed25519 = spki.algorithm.algorithm == OID_SIG_ED25519
            && spki.algorithm.parameters.is_none()
            && spki.subject_public_key.unused_bits == 0
            && spki.subject_public_key.data.len() == 32;
        if !is_p256(spki) && !is_ed25519 {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("RSP requires TLS 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        if !self.supported_verify_schemes().contains(&dss.scheme) {
            return Err(rustls::PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme.into());
        }
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
        ]
    }
}

struct ExternalSigningKey {
    signer: Arc<dyn ExternalSigner>,
    spki: Vec<u8>,
}

impl fmt::Debug for ExternalSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalSigningKey").finish_non_exhaustive()
    }
}

impl SigningKey for ExternalSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        offered
            .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
            .then(|| Box::new(ExternalTlsSigner(self.signer.clone())) as Box<dyn Signer>)
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        Some(SubjectPublicKeyInfoDer::from(self.spki.as_slice()))
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }
}

struct ExternalTlsSigner(Arc<dyn ExternalSigner>);

impl fmt::Debug for ExternalTlsSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalTlsSigner").finish_non_exhaustive()
    }
}

impl Signer for ExternalTlsSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        self.0
            .sign(message)
            .map_err(|err| Error::Other(OtherError(Arc::new(err))))
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}
