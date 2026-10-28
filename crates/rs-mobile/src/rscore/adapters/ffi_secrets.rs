//! rs_register_secret_store C ABI (rs_mobile.h, ).

use std::{
    ffi::{c_char, c_void},
    sync::Arc,
};

use tracing::{info, warn};

use super::secrets::{set_registered, CallbackStore};
use crate::rscore::ffi_session::guard;

pub type RsSecretGetCb = extern "C" fn(*mut c_void, *const c_char, *mut u8, *mut usize) -> i32;
pub type RsSecretPutCb = extern "C" fn(*mut c_void, *const c_char, *const u8, usize) -> i32;
pub type RsSecretDelCb = extern "C" fn(*mut c_void, *const c_char) -> i32;

/// # Safety
/// `ctx` and the callbacks must stay valid for the process lifetime or until
/// the next registration and all calls using the old registration return.
/// Callbacks must support concurrent calls on arbitrary threads, must not wait
/// synchronously for the main thread or unwind, and must respect buffer lengths.
#[no_mangle]
pub unsafe extern "C" fn rs_register_secret_store(
    ctx: *mut c_void,
    get: Option<RsSecretGetCb>,
    put: Option<RsSecretPutCb>,
    del: Option<RsSecretDelCb>,
) {
    guard(|| {
        let (Some(get), Some(put), Some(del)) = (get, put, del) else {
            warn!("ignored invalid native secret store registration");
            return;
        };
        set_registered(Some(Arc::new(CallbackStore { ctx, get, put, del })));
        info!("native secret store registered");
    });
}
