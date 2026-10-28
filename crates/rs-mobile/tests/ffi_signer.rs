mod support;

use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};
use rs_mobile::rscore::{
    ffi_signer::rs_register_signer,
    signer::{clear_global_signer, global_signer},
};
use rsp_transport::identity::{CertKind, ExternalSigner, LocalIdentity};
use std::{ffi::c_void, ptr, slice, sync::Arc};

extern "C" fn sign(
    ctx: *mut c_void,
    msg: *const u8,
    len: usize,
    out: *mut u8,
    out_len: *mut usize,
) -> i32 {
    // SAFETY: The test keeps TestSigner alive; FfiSigner supplies readable msg
    // and writable output/capacity pointers for this invocation.
    unsafe {
        let signer = &*ctx.cast::<support::TestSigner>();
        let Ok(signature) = signer.sign(slice::from_raw_parts(msg, len)) else {
            return 1;
        };
        if signature.len() > *out_len {
            return 1;
        }
        ptr::copy_nonoverlapping(signature.as_ptr(), out, signature.len());
        *out_len = signature.len();
    }
    0
}
extern "C" fn failed(_: *mut c_void, _: *const u8, _: usize, _: *mut u8, _: *mut usize) -> i32 {
    1
}
extern "C" fn empty(_: *mut c_void, _: *const u8, _: usize, _: *mut u8, len: *mut usize) -> i32 {
    // SAFETY: FfiSigner provides this writable output length.
    unsafe {
        *len = 0;
    }
    0
}
extern "C" fn oversized(
    _: *mut c_void,
    _: *const u8,
    _: usize,
    _: *mut u8,
    len: *mut usize,
) -> i32 {
    // SAFETY: FfiSigner provides this writable output length. Deliberately report
    // an invalid size without writing beyond the actual output buffer.
    unsafe {
        *len = 200;
    }
    0
}

#[test]
fn native_signer_validates_registration_signatures_and_failures() {
    clear_global_signer();
    let signer = support::TestSigner::new();
    let ctx = Arc::as_ptr(&signer).cast_mut().cast::<c_void>();
    let spki = signer.spki_der();
    let mut registered_spki = spki.clone();
    // SAFETY: TestSigner and all callback code live through all uses below.
    unsafe {
        rs_register_signer(
            ctx,
            Some(sign),
            registered_spki.as_ptr(),
            registered_spki.len(),
        );
    }
    registered_spki.fill(0);
    let native = global_signer().unwrap();
    assert_eq!(native.spki_der(), spki);
    let signature = native.sign(b"x").unwrap();
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &spki[26..])
        .verify(b"x", &signature)
        .unwrap();
    let certificate = LocalIdentity::generate_external(CertKind::App, native.clone()).unwrap();
    assert!(!certificate.cert_der.is_empty());
    assert!(signer.sign_count() >= 2);
    let mut wrong = spki.clone();
    wrong[25] = 0x02;
    // SAFETY: All supplied buffers are valid; these registrations must be ignored.
    unsafe {
        rs_register_signer(ctx, Some(sign), ptr::null(), 0);
        rs_register_signer(ctx, Some(sign), spki.as_ptr(), 90);
        rs_register_signer(ctx, Some(sign), wrong.as_ptr(), wrong.len());
        rs_register_signer(ctx, None, spki.as_ptr(), spki.len());
    }
    assert!(Arc::ptr_eq(&native, &global_signer().unwrap()));
    for cb in [failed as _, empty as _, oversized as _] {
        // SAFETY: Error callbacks do not use ctx; SPKI is readable.
        unsafe {
            rs_register_signer(ptr::null_mut(), Some(cb), spki.as_ptr(), spki.len());
        }
        assert!(global_signer().unwrap().sign(b"x").is_err());
    }
    clear_global_signer();
}
