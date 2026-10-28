#![cfg(target_os = "macos")]

use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    pkcs8::DecodePublicKey,
    PublicKey,
};
use rs_desktop::mac_signer::{self, Backend, SecretStore};
use rsp_transport::identity::ExternalSigner;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default)]
struct MemoryKeychain {
    value: Arc<Mutex<Option<Vec<u8>>>>,
}

impl SecretStore for MemoryKeychain {
    fn get(&self) -> Result<Option<Vec<u8>>, rs_desktop::platform::PlatformError> {
        Ok(self.value.lock().expect("memory keychain lock").clone())
    }

    fn set(&self, value: &[u8]) -> Result<(), rs_desktop::platform::PlatformError> {
        *self.value.lock().expect("memory keychain lock") = Some(value.to_vec());
        Ok(())
    }
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rs-desktop-mac-signer-{}-{now}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn verifying_key(signer: &dyn ExternalSigner) -> VerifyingKey {
    let public = PublicKey::from_public_key_der(&signer.spki_der()).expect("valid SPKI");
    VerifyingKey::from_sec1_bytes(public.to_sec1_bytes().as_ref()).expect("valid P-256 public key")
}

fn assert_signs(signer: &dyn ExternalSigner, message: &[u8]) {
    let signature =
        Signature::from_der(&signer.sign(message).expect("sign")).expect("DER signature");
    verifying_key(signer)
        .verify(message, &signature)
        .expect("signature verifies");
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

#[test]
fn file_backend_roundtrip() {
    let dir = TestDir::new();
    let keychain = Arc::new(MemoryKeychain::default());
    let (first, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::File), keychain.clone(), false)
            .expect("create file signer");
    assert_eq!(backend, Backend::File);
    assert!(created);
    let first_spki = first.spki_der();
    let public = PublicKey::from_public_key_der(&first_spki).expect("SPKI parses");
    assert_eq!(public.to_sec1_bytes().len(), 65);
    assert_signs(first.as_ref(), b"hello");

    let (second, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::File), keychain, false)
            .expect("open file signer");
    assert_eq!(backend, Backend::File);
    assert!(!created);
    assert_eq!(first_spki, second.spki_der());
    assert_eq!(mode(&dir.path().join("device_key.p8")), 0o600);
    assert_eq!(mode(&dir.path().join("device_key.json")), 0o600);
}

#[test]
fn keychain_backend_with_fake_store() {
    let dir = TestDir::new();
    let keychain = Arc::new(MemoryKeychain::default());
    let (first, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::Keychain), keychain.clone(), false)
            .expect("create keychain signer");
    assert_eq!(backend, Backend::Keychain);
    assert!(created);
    assert_eq!(
        keychain
            .value
            .lock()
            .expect("memory keychain lock")
            .as_ref()
            .map(Vec::len),
        Some(32)
    );
    assert_signs(first.as_ref(), b"hello");
    let first_spki = first.spki_der();

    let (second, _, created) =
        mac_signer::open_with(dir.path(), Some(Backend::Keychain), keychain, false)
            .expect("open keychain signer");
    assert!(!created);
    assert_eq!(first_spki, second.spki_der());
}

#[test]
fn keychain_missing_regenerates() {
    let dir = TestDir::new();
    let populated = Arc::new(MemoryKeychain::default());
    let (first, _, _) =
        mac_signer::open_with(dir.path(), Some(Backend::Keychain), populated, false)
            .expect("create keychain signer");
    let first_spki = first.spki_der();
    let empty = Arc::new(MemoryKeychain::default());
    let (second, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::Keychain), empty, false)
            .expect("regenerate keychain signer");
    assert_eq!(backend, Backend::Keychain);
    assert!(created);
    assert_ne!(first_spki, second.spki_der());
}

#[test]
fn metadata_records_spki_hash() {
    let dir = TestDir::new();
    let signer = mac_signer::open_with(
        dir.path(),
        Some(Backend::File),
        Arc::new(MemoryKeychain::default()),
        false,
    )
    .expect("create file signer")
    .0;
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join("device_key.json")).expect("metadata"))
            .expect("metadata JSON");
    let expected = Sha256::digest(signer.spki_der());
    let expected = expected
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(json["spkiSha256"].as_str(), Some(expected.as_str()));
}

#[cfg(target_arch = "aarch64")]
#[test]
fn secure_enclave_roundtrip() {
    assert!(mac_signer::se_available());
    let dir = TestDir::new();
    let keychain = Arc::new(MemoryKeychain::default());
    let (first, backend, created) = mac_signer::open_with(
        dir.path(),
        Some(Backend::SecureEnclave),
        keychain.clone(),
        true,
    )
    .expect("create Secure Enclave signer");
    assert_eq!(backend, Backend::SecureEnclave);
    assert!(created);
    assert_signs(first.as_ref(), b"hello");
    let first_spki = first.spki_der();
    let (second, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::SecureEnclave), keychain, true)
            .expect("open Secure Enclave signer");
    assert_eq!(backend, Backend::SecureEnclave);
    assert!(!created);
    assert_eq!(first_spki, second.spki_der());
    assert_eq!(mode(&dir.path().join("device_key.se")), 0o600);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn secure_enclave_corrupt_blob_regenerates() {
    let dir = TestDir::new();
    let keychain = Arc::new(MemoryKeychain::default());
    let (first, _, _) = mac_signer::open_with(
        dir.path(),
        Some(Backend::SecureEnclave),
        keychain.clone(),
        true,
    )
    .expect("create Secure Enclave signer");
    let first_spki = first.spki_der();
    fs::write(dir.path().join("device_key.se"), [0x55_u8; 32]).expect("corrupt blob");
    let (second, backend, created) =
        mac_signer::open_with(dir.path(), Some(Backend::SecureEnclave), keychain, true)
            .expect("regenerate Secure Enclave signer");
    assert_eq!(backend, Backend::SecureEnclave);
    assert!(created);
    assert_ne!(first_spki, second.spki_der());
}

#[test]
fn sign_is_der() {
    let dir = TestDir::new();
    let signer = mac_signer::open_with(
        dir.path(),
        Some(Backend::File),
        Arc::new(MemoryKeychain::default()),
        false,
    )
    .expect("create signer")
    .0;
    let signature = signer.sign(b"DER").expect("sign");
    assert_eq!(signature.first(), Some(&0x30));
    assert!(signature.len() <= 72);
}

#[test]
fn concurrent_sign() {
    let dir = TestDir::new();
    let signer = mac_signer::open_with(
        dir.path(),
        Some(Backend::File),
        Arc::new(MemoryKeychain::default()),
        false,
    )
    .expect("create signer")
    .0;
    let verifying = verifying_key(signer.as_ref());
    let mut workers = Vec::new();
    for thread in 0..4 {
        let signer = signer.clone();
        workers.push(std::thread::spawn(move || {
            for index in 0..50 {
                let message = format!("thread-{thread}-{index}");
                let signature =
                    Signature::from_der(&signer.sign(message.as_bytes()).expect("sign"))
                        .expect("DER signature");
                verifying
                    .verify(message.as_bytes(), &signature)
                    .expect("verify");
            }
        }));
    }
    for worker in workers {
        worker.join().expect("worker completed");
    }
}
