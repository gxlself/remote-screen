use std::{
    io::Cursor,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use rcgen::PublicKeyData;
use ring::{rand::SystemRandom, signature};
use rsp_transport::{
    identity::{CertKind, ExternalSigner, KeySource, LocalIdentity, SignError},
    tls::{
        classify, client_config, peer_fingerprint, server_config, HandshakeFailure, ServerPin,
        ALPN_RSP1,
    },
};
use rustls::{
    client::ResolvesClientCert,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    sign::CertifiedKey,
    ClientConfig, ClientConnection, Error, ServerConfig, ServerConnection,
};

#[test]
fn software_mutual_tls_checks_pins_and_extracts_both_fingerprints() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    assert_success(&host, &app, ServerPin::Fingerprint(host.fingerprint()));
}

#[test]
fn external_p256_signer_works_in_both_tls_roles() {
    for external_host in [false, true] {
        let signer = Arc::new(RingSigner::new());
        let external = LocalIdentity::generate_external(
            if external_host {
                CertKind::Host
            } else {
                CertKind::App
            },
            signer.clone(),
        )
        .unwrap();
        assert_eq!(signer.calls.load(Ordering::SeqCst), 1);
        let software = LocalIdentity::generate_software(if external_host {
            CertKind::App
        } else {
            CertKind::Host
        })
        .unwrap();
        let (host, app) = if external_host {
            (&external, &software)
        } else {
            (&software, &external)
        };
        assert_success(host, app, ServerPin::Fingerprint(host.fingerprint()));
        assert_eq!(signer.calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn wrong_pin_is_classified_with_expected_and_actual_fingerprints() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let mut wrong = host.fingerprint();
    wrong[0] ^= 1;
    let err = handshake(
        client_config(&app, ServerPin::Fingerprint(wrong)).unwrap(),
        server_config(&host).unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        classify(&err),
        HandshakeFailure::FingerprintMismatch {
            expected: wrong,
            got: host.fingerprint(),
        }
    );
    assert_eq!(classify(&err.clone()), classify(&err));
}

#[test]
fn six_digit_pairing_accepts_an_unknown_server_key() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    assert_success(&host, &app, ServerPin::Any);
}

#[test]
fn host_requires_a_client_certificate() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let mut config = client_config(&app, ServerPin::Fingerprint(host.fingerprint())).unwrap();
    config.client_auth_cert_resolver = Arc::new(NoCertificate);
    let err = handshake(config, server_config(&host).unwrap()).unwrap_err();
    assert_eq!(classify(&err), HandshakeFailure::NoPeerCertificate);
    assert_eq!(
        classify(&Error::AlertReceived(
            rustls::AlertDescription::CertificateRequired
        )),
        HandshakeFailure::NoPeerCertificate
    );
}

#[test]
fn host_accepts_an_ed25519_client_key() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = software_identity(&rcgen::PKCS_ED25519);
    assert_success(&host, &app, ServerPin::Fingerprint(host.fingerprint()));
}

#[test]
fn host_rejects_a_p384_client_key() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = software_identity(&rcgen::PKCS_ECDSA_P384_SHA384);
    assert!(handshake(
        client_config(&app, ServerPin::Any).unwrap(),
        server_config(&host).unwrap(),
    )
    .is_err());
}

#[test]
fn invalid_certificate_verify_signatures_fail_in_both_directions() {
    for external_host in [false, true] {
        let signer = Arc::new(RingSigner::new());
        let external = LocalIdentity::generate_external(
            if external_host {
                CertKind::Host
            } else {
                CertKind::App
            },
            signer.clone(),
        )
        .unwrap();
        signer.corrupt.store(true, Ordering::SeqCst);
        let software = LocalIdentity::generate_software(if external_host {
            CertKind::App
        } else {
            CertKind::Host
        })
        .unwrap();
        let (host, app) = if external_host {
            (&external, &software)
        } else {
            (&software, &external)
        };
        // Any still verifies the server's CertificateVerify proof of possession.
        let err = handshake(
            client_config(app, ServerPin::Any).unwrap(),
            server_config(host).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidCertificate(_)), "{err:?}");
        assert_eq!(classify(&err), HandshakeFailure::Other);
        assert_eq!(signer.calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn mismatched_local_certificates_and_keys_are_rejected() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let mut other = LocalIdentity::generate_software(CertKind::Host).unwrap();
    other.cert_der = host.cert_der.clone();
    assert!(server_config(&other).is_err());
    let signer = Arc::new(RingSigner::new());
    let mut external = LocalIdentity::generate_external(CertKind::App, signer).unwrap();
    external.cert_der = host.cert_der;
    assert!(client_config(&external, ServerPin::Any).is_err());
}

#[test]
fn peer_fingerprint_rejects_missing_and_malformed_leaf_certificates() {
    assert_eq!(peer_fingerprint(&[]), None);
    assert_eq!(peer_fingerprint(&[CertificateDer::from(vec![0])]), None);
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    assert_eq!(
        peer_fingerprint(&[host.cert_der.clone(), CertificateDer::from(vec![0])]),
        Some(host.fingerprint())
    );
}

#[test]
fn tls12_and_wrong_alpn_are_rejected() {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let mut client = client_config(&app, ServerPin::Any).unwrap();
    client.alpn_protocols = vec![b"unrelated/1".to_vec()];
    assert!(matches!(
        handshake(client, server_config(&host).unwrap()),
        Err(Error::NoApplicationProtocol)
    ));

    let KeySource::Software(key) = app.key else {
        panic!("software key")
    };
    let mut roots = rustls::RootCertStore::empty();
    roots.add(host.cert_der.clone()).unwrap();
    let mut client =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_root_certificates(roots)
            .with_client_auth_cert(vec![app.cert_der], key.into())
            .unwrap();
    client.alpn_protocols = vec![ALPN_RSP1.to_vec()];
    assert!(handshake(client, server_config(&host).unwrap()).is_err());
}

fn assert_success(host: &LocalIdentity, app: &LocalIdentity, pin: ServerPin) {
    let (client, server) = handshake(
        client_config(app, pin).unwrap(),
        server_config(host).unwrap(),
    )
    .unwrap();
    assert_eq!(
        client.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(
        server.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(client.alpn_protocol(), Some(ALPN_RSP1));
    assert_eq!(server.alpn_protocol(), Some(ALPN_RSP1));
    assert_eq!(
        peer_fingerprint(client.peer_certificates().unwrap()),
        Some(host.fingerprint())
    );
    assert_eq!(
        peer_fingerprint(server.peer_certificates().unwrap()),
        Some(app.fingerprint())
    );
}

fn handshake(
    client: ClientConfig,
    server: ServerConfig,
) -> Result<(ClientConnection, ServerConnection), Error> {
    // Deliberately unrelated to the SAN: identity comes from SPKI, not DNS.
    let name = "unrelated.example.com".try_into().unwrap();
    let mut client = ClientConnection::new(Arc::new(client), name)?;
    let mut server = ServerConnection::new(Arc::new(server))?;
    for _ in 0..16 {
        if client.wants_write() {
            let mut wire = Vec::new();
            client.write_tls(&mut wire).unwrap();
            server.read_tls(&mut Cursor::new(wire)).unwrap();
            server.process_new_packets()?;
        }
        if server.wants_write() {
            let mut wire = Vec::new();
            server.write_tls(&mut wire).unwrap();
            client.read_tls(&mut Cursor::new(wire)).unwrap();
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() && !client.wants_write() {
            return Ok((client, server));
        }
    }
    panic!("in-memory TLS handshake did not finish");
}

fn software_identity(algorithm: &'static rcgen::SignatureAlgorithm) -> LocalIdentity {
    let key = rcgen::KeyPair::generate_for(algorithm).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["rs-app".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    LocalIdentity {
        cert_der: cert.der().clone(),
        key: KeySource::Software(PrivatePkcs8KeyDer::from(key.serialize_der())),
    }
}

struct RingSigner {
    key: signature::EcdsaKeyPair,
    spki: Vec<u8>,
    calls: AtomicUsize,
    corrupt: AtomicBool,
}

impl RingSigner {
    fn new() -> Self {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        Self {
            key: signature::EcdsaKeyPair::from_pkcs8(
                &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                key.serialized_der(),
                &SystemRandom::new(),
            )
            .unwrap(),
            spki: key.subject_public_key_info(),
            calls: AtomicUsize::new(0),
            corrupt: AtomicBool::new(false),
        }
    }
}

impl ExternalSigner for RingSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let signature = self
            .key
            .sign(&SystemRandom::new(), msg)
            .map_err(|_| SignError("ring signing failed".into()))?;
        let mut signature = signature.as_ref().to_vec();
        if self.corrupt.load(Ordering::SeqCst) {
            // Preserve DER structure while invalidating the signature scalar.
            *signature.last_mut().unwrap() ^= 1;
        }
        Ok(signature)
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

#[derive(Debug)]
struct NoCertificate;

impl ResolvesClientCert for NoCertificate {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}
