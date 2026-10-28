#![cfg(windows)]

use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    pkcs8::DecodePublicKey,
    PublicKey,
};
use rs_desktop::win_signer::{
    self, Backend, WinSigner, KEYSTORE_ENV, KEY_NAME_PREFIX, SOFTWARE_PROVIDER,
};
use rsp_transport::identity::ExternalSigner;
use serde_json::Value;
use sha2::Digest;
use std::{fs, path::PathBuf, sync::Arc, thread};
use windows::{
    core::PCWSTR,
    Win32::{Foundation::NTE_BAD_KEYSET, Security::Cryptography::*},
};

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).expect("test randomness");
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("rs-desktop-win-signer-{suffix}"));
        fs::create_dir_all(&path).expect("test directory");
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = win_signer::delete_key(&self.path);
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn verify(signer: &WinSigner, message: &[u8]) {
    let public = PublicKey::from_public_key_der(&signer.spki_der()).expect("SPKI");
    let verifying = VerifyingKey::from(public);
    let signature = Signature::from_der(&signer.sign(message).expect("signature")).expect("DER");
    verifying
        .verify(message, &signature)
        .expect("valid signature");
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_cng_key(name: &str, provider_name: &str) -> (NCRYPT_PROV_HANDLE, NCRYPT_KEY_HANDLE) {
    let provider_name = wide(provider_name);
    let key_name = wide(name);
    let mut provider = NCRYPT_PROV_HANDLE::default();
    // SAFETY: the UTF-16 strings are NUL terminated and provider points to a
    // writable output slot for the synchronous NCrypt call.
    unsafe { NCryptOpenStorageProvider(&mut provider, PCWSTR(provider_name.as_ptr()), 0) }
        .expect("provider");
    let mut key = NCRYPT_KEY_HANDLE::default();
    // SAFETY: provider is live, key_name is NUL terminated, and key is an
    // output slot valid for the duration of the call.
    unsafe {
        NCryptOpenKey(
            provider,
            &mut key,
            PCWSTR(key_name.as_ptr()),
            CERT_KEY_SPEC(0),
            NCRYPT_SILENT_FLAG,
        )
    }
    .expect("key");
    (provider, key)
}

fn free_cng_handles(provider: NCRYPT_PROV_HANDLE, key: NCRYPT_KEY_HANDLE) {
    // SAFETY: both handles were returned by NCrypt and are exclusively owned
    // by this test helper.
    unsafe {
        let _ = NCryptFreeObject(key.into());
        let _ = NCryptFreeObject(provider.into());
    }
}

fn delete_cng_key(name: &str, provider_name: &str) {
    let (provider, key) = open_cng_key(name, provider_name);
    // SAFETY: key is a valid persisted key handle owned by this test.
    unsafe { NCryptDeleteKey(key, 0) }.expect("delete CNG key");
    // NCryptDeleteKey consumes the key handle; only the provider remains.
    // SAFETY: provider is still live and exclusively owned here.
    unsafe {
        let _ = NCryptFreeObject(provider.into());
    }
}

fn remove_keystore_env() {
    std::env::remove_var(KEYSTORE_ENV);
}

#[test]
fn file_backend_roundtrip() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (first, created) =
        win_signer::open_with(&directory.path, Some(Backend::File)).expect("open");
    assert!(created);
    assert_eq!(first.backend(), Backend::File);
    assert!(first.key_name().is_none());
    verify(&first, b"hello");
    let first_spki = first.spki_der();

    let (second, created) =
        win_signer::open_with(&directory.path, Some(Backend::File)).expect("reopen");
    assert!(!created);
    assert_eq!(second.spki_der(), first_spki);
}

#[test]
fn software_ksp_roundtrip() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (first, created) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("open");
    assert!(created);
    assert_eq!(first.backend(), Backend::SoftwareKsp);
    let key_name = first.key_name().expect("CNG key name");
    assert!(key_name.starts_with(KEY_NAME_PREFIX));
    assert_eq!(key_name.len(), KEY_NAME_PREFIX.len() + 16);
    verify(&first, b"hello");
    let first_spki = first.spki_der();

    let (second, created) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("reopen");
    assert!(!created);
    assert_eq!(second.spki_der(), first_spki);
}

#[test]
fn software_ksp_not_exportable() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (signer, _) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("open");
    let name = signer.key_name().expect("key name");
    let (provider, key) = open_cng_key(name, SOFTWARE_PROVIDER);

    let mut exported_size = 0u32;
    // SAFETY: this is a size query against a live key; the output length slot
    // is writable and no private buffer is supplied.
    let export = unsafe {
        NCryptExportKey(
            key,
            None,
            BCRYPT_ECCPRIVATE_BLOB,
            None,
            None,
            &mut exported_size,
            NCRYPT_SILENT_FLAG,
        )
    };
    assert!(export.is_err(), "private export unexpectedly succeeded");

    let mut policy = [0u8; 4];
    let mut policy_len = 0u32;
    // SAFETY: policy is a writable four-byte property buffer and key is live.
    unsafe {
        NCryptGetProperty(
            key.into(),
            NCRYPT_EXPORT_POLICY_PROPERTY,
            Some(&mut policy),
            &mut policy_len,
            windows::Win32::Security::OBJECT_SECURITY_INFORMATION(0),
        )
    }
    .expect("export policy");
    assert_eq!(policy_len, 4);
    assert_eq!(u32::from_ne_bytes(policy), 0);
    free_cng_handles(provider, key);
}

#[test]
fn default_prefers_tpm() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (signer, _) = win_signer::open_with(&directory.path, None).expect("open");
    let expected = if win_signer::tpm_available() {
        Backend::Tpm
    } else {
        Backend::SoftwareKsp
    };
    assert_eq!(signer.backend(), expected);
    verify(&signer, b"hello");
    eprintln!("backend={:?} last_sign_us={}", signer.backend(), {
        let _ = signer.sign(b"timing");
        signer.last_sign_us()
    });
}

#[test]
fn missing_key_regenerates() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (first, _) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("open");
    let old_name = first.key_name().expect("old key name").to_owned();
    let old_spki = first.spki_der();
    delete_cng_key(&old_name, SOFTWARE_PROVIDER);

    let (second, created) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("regenerate");
    assert!(created);
    assert_ne!(second.key_name(), Some(old_name.as_str()));
    assert_ne!(second.spki_der(), old_spki);
}

#[test]
fn metadata_records_spki_hash() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (signer, _) = win_signer::open_with(&directory.path, Some(Backend::File)).expect("open");
    let metadata: Value = serde_json::from_slice(
        &fs::read(directory.path.join("device_key.json")).expect("metadata"),
    )
    .expect("JSON");
    let digest = sha2::Sha256::digest(signer.spki_der());
    let expected: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(metadata["spkiSha256"], expected);
    assert_eq!(metadata["backend"], "file");
    assert!(metadata.get("keyName").is_none());
}

#[test]
fn sign_is_der() {
    remove_keystore_env();
    for backend in [Backend::File, Backend::SoftwareKsp] {
        let directory = TestDirectory::new();
        let (signer, _) = win_signer::open_with(&directory.path, Some(backend)).expect("open");
        let signature = signer.sign(b"DER").expect("signature");
        assert_eq!(signature.first().copied(), Some(0x30));
        assert!(signature.len() <= 72);
    }
}

#[test]
fn concurrent_sign() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (signer, _) = win_signer::open_with(&directory.path, Some(Backend::File)).expect("open");
    let verifying =
        VerifyingKey::from(PublicKey::from_public_key_der(&signer.spki_der()).expect("SPKI"));
    let mut workers = Vec::new();
    for worker in 0..4 {
        let signer = Arc::clone(&signer);
        workers.push(thread::spawn(move || {
            for iteration in 0..10 {
                let message = format!("worker-{worker}-{iteration}");
                let signature =
                    Signature::from_der(&signer.sign(message.as_bytes()).expect("sign"))
                        .expect("DER");
                verifying
                    .verify(message.as_bytes(), &signature)
                    .expect("verify");
            }
        }));
    }
    for worker in workers {
        worker.join().expect("worker");
    }
}

#[test]
fn delete_key_removes_everything() {
    remove_keystore_env();
    let directory = TestDirectory::new();
    let (signer, _) =
        win_signer::open_with(&directory.path, Some(Backend::SoftwareKsp)).expect("open");
    let name = signer.key_name().expect("key name").to_owned();
    win_signer::delete_key(&directory.path).expect("delete");
    assert!(!directory.path.join("device_key.json").exists());
    assert!(!directory.path.join("device_key.p8").exists());

    let (provider, key_name) = {
        let provider_name = wide(SOFTWARE_PROVIDER);
        let key_name = wide(&name);
        let mut provider = NCRYPT_PROV_HANDLE::default();
        // SAFETY: provider_name is NUL terminated and provider is an output slot.
        unsafe { NCryptOpenStorageProvider(&mut provider, PCWSTR(provider_name.as_ptr()), 0) }
            .expect("provider");
        (provider, key_name)
    };
    let mut key = NCRYPT_KEY_HANDLE::default();
    // SAFETY: provider is live, key_name is NUL terminated, and key is an
    // output slot for this synchronous call.
    let result = unsafe {
        NCryptOpenKey(
            provider,
            &mut key,
            PCWSTR(key_name.as_ptr()),
            CERT_KEY_SPEC(0),
            NCRYPT_SILENT_FLAG,
        )
    };
    assert_eq!(
        result.expect_err("deleted key should not open").code(),
        NTE_BAD_KEYSET
    );
    // SAFETY: provider is live and owned by this test.
    unsafe {
        let _ = NCryptFreeObject(provider.into());
    }
}
