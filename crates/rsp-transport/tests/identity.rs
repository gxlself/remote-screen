use std::sync::Arc;

use rcgen::PublicKeyData;
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};
use rsp_transport::identity::{
    spki_fingerprint, CertKind, ExternalSigner, IdentityError, KeySource, LocalIdentity, SignError,
};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use x509_parser::prelude::*;

const FIXED_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBkjCCATigAwIBAgIUZKq5g40Ir4H/cRJ81vtpHtIS3NMwCgYIKoZIzj0EAwIw
FTETMBEGA1UEAwwKYS1maXh0dXJlMTAeFw0yNjEwMjAxMTE3NTZaFw0zNjEwMTcx
MTE3NTZaMBUxEzARBgNVBAMMCmEtZml4dHVyZTEwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAASSw3xnfG9zhMyLqhptSN532AcIFHxiLN1fzbuo8KmBWEzSh+IAby9Y
5CRt7POmFz4G3qwXAKjbgohk9wVGjbI8o2YwZDAdBgNVHQ4EFgQU5aBYwmi1tc4o
i44g8nD0Sjd/rTIwHwYDVR0jBBgwFoAU5aBYwmi1tc4oi44g8nD0Sjd/rTIwDwYD
VR0TAQH/BAUwAwEB/zARBgNVHREECjAIggZycy1hcHAwCgYIKoZIzj0EAwIDSAAw
RQIgVJgxrM1MNc4lX24P59jdusm95EedjbTTRU8hnyRMzFQCIQC1e+raIKknMq8r
Fmm51KZVOdC8emFu28T6C42bbjfhHA==
-----END CERTIFICATE-----";

#[test]
fn fixed_certificate_has_known_spki_fingerprint() {
    let der = CertificateDer::from_pem_slice(FIXED_CERT.as_bytes()).unwrap();
    // Independently computed with openssl x509 -pubkey | openssl pkey
    // -pubin -outform DER | openssl dgst -sha256.
    let expected = [
        0x00, 0xdd, 0xd3, 0xc7, 0x26, 0x8f, 0x5a, 0x36, 0xf3, 0x72, 0x8f, 0x81, 0x11, 0x55, 0x66,
        0xd2, 0x0c, 0xd0, 0xd7, 0xed, 0x85, 0x7f, 0x9d, 0x8c, 0x4a, 0x09, 0xdc, 0x31, 0x34, 0x69,
        0x9b, 0x68,
    ];
    assert_eq!(spki_fingerprint(&der).unwrap(), expected);
}

#[test]
fn malformed_and_trailing_certificate_data_are_rejected() {
    assert!(matches!(
        spki_fingerprint(b"invalid certificate"),
        Err(IdentityError::InvalidCertificate)
    ));
    let der = CertificateDer::from_pem_slice(FIXED_CERT.as_bytes()).unwrap();
    assert!(spki_fingerprint(&der[..der.len() - 1]).is_err());
    let mut trailing = der.to_vec();
    trailing.push(0);
    assert!(spki_fingerprint(&trailing).is_err());
}

#[test]
fn software_certificate_fields_and_self_signatures_match_each_role() {
    for (kind, prefix, san, years) in [
        (CertKind::Host, "h-", "rs-host", 20),
        (CertKind::App, "a-", "rs-app", 10),
    ] {
        let identity = LocalIdentity::generate_software(kind).unwrap();
        assert!(matches!(identity.key, KeySource::Software(_)));
        assert_certificate(&identity, prefix, san, years);
        assert_eq!(
            identity.fingerprint(),
            spki_fingerprint(&identity.cert_der).unwrap()
        );
    }
}

#[test]
fn renewed_external_certificates_keep_the_key_fingerprint() {
    let signer = Arc::new(TestSigner(
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap(),
    ));
    let first = LocalIdentity::generate_external(CertKind::Host, signer.clone()).unwrap();
    let renewed = LocalIdentity::generate_external(CertKind::App, signer).unwrap();
    assert!(matches!(first.key, KeySource::External(_)));
    assert_ne!(first.cert_der, renewed.cert_der);
    assert_eq!(first.fingerprint(), renewed.fingerprint());
    assert_certificate(&first, "h-", "rs-host", 20);
    assert_certificate(&renewed, "a-", "rs-app", 10);
}

#[test]
fn external_certificate_generation_rejects_wrong_key_types_and_invalid_spki() {
    let ed25519 = Arc::new(TestSigner(
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap(),
    ));
    assert!(matches!(
        LocalIdentity::generate_external(CertKind::App, ed25519),
        Err(IdentityError::UnsupportedExternalKey)
    ));
    let invalid = Arc::new(InvalidSigner);
    assert!(matches!(
        LocalIdentity::generate_external(CertKind::App, invalid),
        Err(IdentityError::InvalidSpki)
    ));
}

#[test]
fn external_certificate_generation_propagates_signing_failure() {
    let signer = Arc::new(FailingSigner(
        rcgen::KeyPair::generate()
            .unwrap()
            .subject_public_key_info(),
    ));
    assert!(matches!(
        LocalIdentity::generate_external(CertKind::App, signer),
        Err(IdentityError::Certificate(rcgen::Error::RemoteKeyError))
    ));
}

fn assert_certificate(identity: &LocalIdentity, prefix: &str, san: &str, years: i64) {
    let (remaining, cert) = parse_x509_certificate(&identity.cert_der).unwrap();
    assert!(remaining.is_empty());
    assert_eq!(cert.subject(), cert.issuer());
    assert_eq!(cert.subject().iter().count(), 1);
    let common_names: Vec<_> = cert.subject().iter_common_name().collect();
    assert_eq!(common_names.len(), 1);
    let name = common_names[0].as_str().unwrap();
    assert!(name.starts_with(prefix));
    assert_eq!(name.len(), prefix.len() + 8);
    assert!(name[prefix.len()..]
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()));
    assert_eq!(
        cert.subject_alternative_name()
            .unwrap()
            .unwrap()
            .value
            .general_names,
        vec![GeneralName::DNSName(san)]
    );
    let lifetime = cert.validity().not_after.timestamp() - cert.validity().not_before.timestamp();
    assert!((lifetime - years * 31_557_600).abs() < 86_400);
    assert!((cert.validity().not_before.timestamp() - ASN1Time::now().timestamp()).abs() < 60);
    assert_eq!(
        cert.public_key().algorithm.algorithm,
        x509_parser::oid_registry::OID_KEY_TYPE_EC_PUBLIC_KEY
    );
    assert_eq!(
        cert.public_key()
            .algorithm
            .parameters
            .as_ref()
            .unwrap()
            .as_oid()
            .unwrap(),
        x509_parser::oid_registry::OID_EC_P256
    );
    UnparsedPublicKey::new(
        &ECDSA_P256_SHA256_ASN1,
        cert.public_key().subject_public_key.data.as_ref(),
    )
    .verify(cert.tbs_certificate.as_ref(), &cert.signature_value.data)
    .unwrap();
}

struct TestSigner(rcgen::KeyPair);

impl ExternalSigner for TestSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        rcgen::SigningKey::sign(&self.0, msg).map_err(|err| SignError(err.to_string()))
    }

    fn spki_der(&self) -> Vec<u8> {
        self.0.subject_public_key_info()
    }
}

struct InvalidSigner;

impl ExternalSigner for InvalidSigner {
    fn sign(&self, _msg: &[u8]) -> Result<Vec<u8>, SignError> {
        panic!("invalid SPKI must be rejected before signing")
    }

    fn spki_der(&self) -> Vec<u8> {
        b"invalid SPKI".to_vec()
    }
}

struct FailingSigner(Vec<u8>);

impl ExternalSigner for FailingSigner {
    fn sign(&self, _msg: &[u8]) -> Result<Vec<u8>, SignError> {
        Err(SignError("test signer unavailable".into()))
    }

    fn spki_der(&self) -> Vec<u8> {
        self.0.clone()
    }
}
