//! Registration of native device signers; no persistent private keys live here.

use std::sync::{Arc, Mutex};

use ring::digest;
use rsp_transport::identity::{spki_fingerprint, ExternalSigner};

static SIGNER: Mutex<Option<Arc<dyn ExternalSigner>>> = Mutex::new(None);

pub fn register_global_signer(s: Arc<dyn ExternalSigner>) {
    let previous = SIGNER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .replace(s);
    drop(previous);
}

pub fn global_signer() -> Option<Arc<dyn ExternalSigner>> {
    SIGNER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

pub fn clear_global_signer() {
    let previous = SIGNER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    drop(previous);
}

pub fn spki_matches(signer: &dyn ExternalSigner, cert_der: &[u8]) -> bool {
    spki_fingerprint(cert_der).is_ok_and(|fingerprint| {
        digest::digest(&digest::SHA256, &signer.spki_der()).as_ref() == fingerprint
    })
}
