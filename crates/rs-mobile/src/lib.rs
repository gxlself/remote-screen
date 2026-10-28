//! Remote Screen mobile core with UniFFI and native media/terminal bridges.

pub mod rscore;
pub mod uniffi_api;

uniffi::setup_scaffolding!();

/// Local Unix wall clock in microseconds, shared by native media callbacks.
#[no_mangle]
pub extern "C" fn rs_now_us() -> u64 {
    rsp_core::clock::unix_micros()
}
