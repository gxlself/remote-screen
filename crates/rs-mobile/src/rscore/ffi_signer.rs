//! Process-wide native P-256 signer. Calls may arrive on any thread, including
//! spawn_blocking; the native callback must not synchronously wait for main.

use rsp_transport::identity::{ExternalSigner, SignError};
use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    slice,
    sync::Arc,
};
use tracing::error;

use super::{ffi_session::guard, signer::register_global_signer};

pub type RsSignCb = extern "C" fn(*mut c_void, *const u8, usize, *mut u8, *mut usize) -> i32;

const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

struct FfiSigner {
    ctx: *mut c_void,
    sign: RsSignCb,
    spki: Vec<u8>,
}

// SAFETY: Registration requires the native ctx/callback to remain valid and
// support arbitrary, potentially concurrent threads for the signer's lifetime.
unsafe impl Send for FfiSigner {}
// SAFETY: Same native thread-safety contract; public key bytes are immutable.
unsafe impl Sync for FfiSigner {}

impl ExternalSigner for FfiSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        catch_unwind(AssertUnwindSafe(|| {
            let mut signature = [0u8; 128];
            let mut len = signature.len();
            let status = (self.sign)(
                self.ctx,
                msg.as_ptr(),
                msg.len(),
                signature.as_mut_ptr(),
                &mut len,
            );
            if status != 0 || len == 0 || len > signature.len() {
                return Err(SignError(
                    "native signer failed or returned an invalid length".into(),
                ));
            }
            Ok(signature[..len].to_vec())
        }))
        .unwrap_or_else(|_| {
            error!("panic contained in native signer adapter");
            Err(SignError("native signer adapter panicked".into()))
        })
    }
    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

/// Register a native ECDSA-P256-SHA256 message signer and copy its SPKI.
/// Invalid registration preserves the existing signer.
/// # Safety
/// Non-NULL spki_der must point to spki_len readable bytes during this call.
/// ctx/callback must remain valid for the process lifetime, or until replacement
/// and all in-flight uses of the old signer complete. Calls may be concurrent
/// on any thread (including spawn_blocking); do not wait for main or unwind.
/// The callback receives the original message, a 128-byte signature buffer and
/// its capacity in *sig_len. Write a DER signature, set the actual length, and
/// return 0 for success (nonzero for failure). Never write beyond the capacity.
#[no_mangle]
pub unsafe extern "C" fn rs_register_signer(
    ctx: *mut c_void,
    sign: Option<RsSignCb>,
    spki_der: *const u8,
    spki_len: usize,
) {
    guard(|| {
        if spki_der.is_null() || spki_len != 91 || sign.is_none() {
            error!("ignored invalid native P-256 signer registration");
            return;
        }
        // SAFETY: Non-NULL, fixed length, and readable per the caller contract.
        let spki = unsafe { slice::from_raw_parts(spki_der, spki_len) };
        if !spki.starts_with(&P256_SPKI_PREFIX) || spki[26] != 0x04 {
            error!("ignored native signer with invalid P-256 SPKI prefix");
            return;
        }
        if let Some(sign) = sign {
            register_global_signer(Arc::new(FfiSigner {
                ctx,
                sign,
                spki: spki.to_vec(),
            }));
        }
    });
}
