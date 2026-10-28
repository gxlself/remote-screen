#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rcgen::PublicKeyData;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use rs_mobile::rscore::dto_core::{CoreConfig, DevicePlatform};
use rs_mobile::rscore::runtime::{Core, CoreOptions};
use rs_mobile::rscore::store::StoredIdentity;
use rs_mobile::uniffi_api::EventSink;
use rsp_transport::identity::{CertKind, ExternalSigner, LocalIdentity, SignError};
use serde_json::Value;

static TEMP_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn temp_dir(tag: &str) -> PathBuf {
    let root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp"));
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let count = TEMP_COUNT.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!("rs-{tag}-{}-{count}-{time}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Software signing material exists only in this test fixture, never rs.db.
pub struct TestSigner {
    key: EcdsaKeyPair,
    spki: Vec<u8>,
    count: AtomicUsize,
}

impl TestSigner {
    pub fn new() -> Arc<TestSigner> {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &pair.serialize_der(),
            &SystemRandom::new(),
        )
        .unwrap();
        Arc::new(TestSigner {
            key,
            spki: pair.subject_public_key_info(),
            count: AtomicUsize::new(0),
        })
    }

    pub fn sign_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

impl ExternalSigner for TestSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.key
            .sign(&SystemRandom::new(), msg)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|error| SignError(error.to_string()))
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

pub fn test_core() -> Arc<Core> {
    test_core_with(|_, _| {})
}

pub fn test_core_with(f: impl FnOnce(&mut CoreConfig, &mut CoreOptions)) -> Arc<Core> {
    let mut cfg = CoreConfig {
        files_dir: None,
        data_dir: temp_dir("core").to_str().unwrap().to_owned(),
        app_version: "test".to_owned(),
        platform: DevicePlatform::Ios,
    };
    let mut opts = CoreOptions {
        init_logging: false,
        ..CoreOptions::default()
    };
    f(&mut cfg, &mut opts);
    Core::new(cfg, opts).unwrap()
}

pub fn with_identity(core: &Arc<Core>, name: &str) -> Arc<TestSigner> {
    let signer = TestSigner::new();
    core.set_signer(Some(signer.clone()));
    let identity = LocalIdentity::generate_external(CertKind::App, signer.clone()).unwrap();
    core.store()
        .set_identity(&StoredIdentity {
            cert_der: identity.cert_der.to_vec(),
            device_name: name.to_owned(),
            created_at_us: core.now_us(),
            registered_at_us: None,
        })
        .unwrap();
    core.identity_changed();
    signer
}

#[derive(Default)]
pub struct EventCollector {
    collected: Mutex<Vec<(u64, Value)>>,
}

impl EventCollector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn events(&self) -> Vec<(u64, Value)> {
        self.collected.lock().unwrap().clone()
    }

    pub fn of_type(&self, event_type: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .map(|(_, value)| value)
            .filter(|value| value["type"] == event_type)
            .collect()
    }

    pub fn wait_for(&self, predicate: impl Fn(&[Value]) -> bool, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            let events: Vec<_> = self.events().into_iter().map(|(_, value)| value).collect();
            if predicate(&events) {
                return true;
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl EventSink for EventCollector {
    fn on_event(&self, seq: u64, payload: String) {
        self.collected
            .lock()
            .unwrap()
            .push((seq, serde_json::from_str(&payload).unwrap()));
    }
}
