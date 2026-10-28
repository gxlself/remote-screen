mod support;

use std::sync::Arc;

use data_encoding::BASE64URL_NOPAD;
use rs_mobile::rscore::api_identity::ensure;
use rs_mobile::rscore::error::CoreError;
use rsp_transport::identity::{spki_fingerprint, ExternalSigner, SignError};
use serde_json::json;

use support::{test_core, test_core_with, TestSigner};

#[tokio::test]
pub async fn ensure_requires_a_registered_signer() {
    let core = test_core();
    assert_eq!(
        ensure(&core, "小明的 iPhone").await,
        Err(CoreError::internal("device signer not registered"))
    );
    assert!(core.store().identity().unwrap().is_none());
}

#[tokio::test]
pub async fn ensure_signs_once_and_reuses_certificate_while_updating_name() {
    let core = test_core_with(|_, opts| opts.clock = Some(Arc::new(|| 123_456)));
    let signer = TestSigner::new();
    core.set_signer(Some(signer.clone()));
    let first = ensure(&core, "小明的 iPhone").await.unwrap();
    assert_eq!(first.device_id.len(), 16);
    assert_eq!(first.fp.len(), 43);
    let signed = signer.sign_count();
    assert!(
        signed >= 1,
        "certificate must be signed by the native signer"
    );
    let stored = core.store().identity().unwrap().unwrap();
    assert_eq!(stored.device_name, "小明的 iPhone");
    assert_eq!(stored.created_at_us, 123_456);
    assert_eq!(stored.registered_at_us, None);
    assert_eq!(
        BASE64URL_NOPAD.decode(first.fp.as_bytes()).unwrap(),
        spki_fingerprint(&stored.cert_der).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        json!({"deviceId": first.device_id, "fp": first.fp})
    );
    assert_eq!(ensure(&core, "小明的 iPhone").await.unwrap(), first);
    core.store().mark_registered(123_457).unwrap();
    assert_eq!(ensure(&core, "新名字").await.unwrap(), first);
    assert_eq!(signer.sign_count(), signed);
    let renamed = core.store().identity().unwrap().unwrap();
    assert_eq!(renamed.device_name, "新名字");
    assert_eq!(renamed.cert_der, stored.cert_der);
    assert_eq!(renamed.created_at_us, stored.created_at_us);
    assert_eq!(renamed.registered_at_us, Some(123_457));

    core.set_signer(None);
    assert_eq!(
        ensure(&core, "新名字").await,
        Err(CoreError::internal("device signer not registered"))
    );
}

#[tokio::test]
pub async fn changed_signer_replaces_identity_and_invalidates_tls_cache() {
    let core = test_core();
    core.set_signer(Some(TestSigner::new()));
    let old = ensure(&core, "旧设备").await.unwrap();
    // Persisted rows keep their timestamp until replaced.
    core.store().mark_registered(1).unwrap();
    let old_tls = core.local_identity().unwrap();
    assert!(core
        .store()
        .identity()
        .unwrap()
        .unwrap()
        .registered_at_us
        .is_some());

    let signer = TestSigner::new();
    core.set_signer(Some(signer.clone()));
    let new = ensure(&core, "重装后的设备").await.unwrap();
    assert_ne!(new.device_id, old.device_id);
    assert_ne!(new.fp, old.fp);
    assert!(signer.sign_count() >= 1);
    assert_eq!(
        core.store().identity().unwrap().unwrap().registered_at_us,
        None
    );
    assert_ne!(
        core.local_identity().unwrap().fingerprint(),
        old_tls.fingerprint()
    );
}

#[tokio::test]
pub async fn concurrent_ensure_calls_share_one_certificate() {
    let core = test_core();
    let signer = TestSigner::new();
    core.set_signer(Some(signer.clone()));
    let (first, second) = tokio::join!(ensure(&core, "手机"), ensure(&core, "手机"));
    assert_eq!(first.unwrap(), second.unwrap());
    let signed = signer.sign_count();
    assert_eq!(signed, 1, "concurrent creation must sign one certificate");
    ensure(&core, "手机").await.unwrap();
    assert_eq!(signer.sign_count(), signed);
}

pub struct FailingSigner(pub Arc<TestSigner>);

impl ExternalSigner for FailingSigner {
    fn sign(&self, _: &[u8]) -> Result<Vec<u8>, SignError> {
        Err(SignError("signer unavailable".to_owned()))
    }

    fn spki_der(&self) -> Vec<u8> {
        self.0.spki_der()
    }
}

#[tokio::test]
pub async fn failed_native_signing_preserves_the_stored_identity() {
    let core = test_core();
    core.set_signer(Some(TestSigner::new()));
    ensure(&core, "手机").await.unwrap();
    let stored = core.store().identity().unwrap().unwrap();
    core.set_signer(Some(Arc::new(FailingSigner(TestSigner::new()))));
    assert!(matches!(
        ensure(&core, "新手机").await,
        Err(CoreError::Internal { .. })
    ));
    assert_eq!(core.store().identity().unwrap().unwrap(), stored);
}
