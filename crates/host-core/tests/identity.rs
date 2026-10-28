use std::collections::HashMap;
use std::sync::Mutex;

use host_core::identity::HostIdentity;
use host_core::platform::{KeyStore, PlatformError, Result};

#[derive(Default)]
struct Keys(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for Keys {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| PlatformError::Platform("poisoned".into()))?
            .insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[test]
fn identity_survives_restart_and_repairs_certificate() {
    let keys = Keys::default();
    let first = HostIdentity::load_or_create(&keys, 1).expect("identity");
    let fp = first.fp();
    let id = first.host_id();
    let cert = first.cert_der().to_vec();
    let second = HostIdentity::load_or_create(&keys, 2).expect("restart");
    assert_eq!(second.fp(), fp);
    assert_eq!(second.host_id(), id);
    keys.store("host-cert-v1", &[1, 2, 3]).unwrap();
    let repaired = HostIdentity::load_or_create(&keys, 3).expect("repair");
    assert_eq!(repaired.fp(), fp);
    assert_ne!(repaired.cert_der(), &[1, 2, 3]);
    assert_ne!(repaired.cert_der(), cert.as_slice());
}

#[test]
fn certificate_without_key_is_an_error() {
    let keys = Keys::default();
    let first = HostIdentity::load_or_create(&keys, 1).expect("identity");
    keys.delete("host-key-v1").unwrap();
    let error = HostIdentity::load_or_create(&keys, 2).expect_err("missing key");
    assert!(error.to_string().contains("key"));
    drop(first);
}
