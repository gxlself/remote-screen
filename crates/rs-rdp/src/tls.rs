//! TLS upgrade with trust-on-first-use.

use std::net::IpAddr;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::client::TlsStream;

use crate::types::{CertCheck, RdpError};

#[derive(Debug)]
struct TofuVerifier(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // The leaf is checked asynchronously, before any RDP credentials are sent.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Use ring explicitly, deferring certificate trust to [`CertCheck`] while
/// retaining TLS handshake signature verification.
pub fn client_config() -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("ring supports the rustls default TLS protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TofuVerifier(provider)))
            .with_no_client_auth(),
    )
}

/// Complete TLS and obtain trust approval before returning the upgraded stream.
/// The public key is the SPKI subjectPublicKey BIT STRING, as required by CredSSP.
pub async fn upgrade<S>(
    stream: S,
    server_name: &str,
    cert: &dyn CertCheck,
) -> Result<(TlsStream<S>, Vec<u8>, [u8; 32]), RdpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let name = match server_name.parse::<IpAddr>() {
        Ok(ip) => ServerName::IpAddress(ip.into()),
        Err(_) => ServerName::try_from(server_name.to_owned())
            .map_err(|_| RdpError::Tls("bad server name".into()))?,
    };
    let stream = tokio_rustls::TlsConnector::from(client_config())
        .connect(name, stream)
        .await
        .map_err(|error| RdpError::Tls(error.to_string()))?;
    let leaf = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or_else(|| RdpError::Tls("no certificate".into()))?;
    let fingerprint = Sha256::digest(leaf.as_ref()).into();
    if !cert.check(fingerprint).await {
        return Err(RdpError::CertRejected);
    }
    let public_key = server_public_key(leaf.as_ref())?;
    Ok((stream, public_key, fingerprint))
}

/// Lowercase hexadecimal SHA-256 fingerprint, without separators.
pub fn fingerprint_hex(fp: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in fp {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// Extract the key bytes, without the SubjectPublicKeyInfo DER wrapper.
pub fn server_public_key(cert_der: &[u8]) -> Result<Vec<u8>, RdpError> {
    let (remaining, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|_| RdpError::Tls("bad certificate".into()))?;
    let key = &cert.public_key().subject_public_key;
    if !remaining.is_empty() || key.unused_bits != 0 || key.data.is_empty() {
        return Err(RdpError::Tls("bad certificate".into()));
    }
    Ok(key.data.to_vec())
}
