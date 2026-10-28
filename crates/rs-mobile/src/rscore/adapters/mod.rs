//! Built-in VNC / RDP adapters (M3 V4, ).

pub mod api;
pub mod audio;
pub mod cursor;
pub mod driver;
pub mod dto;
pub mod ffi_raw;
pub mod ffi_secrets;
pub mod rawframe;
pub mod rdp;
pub mod route;
pub mod screen_guess;
pub mod secrets;
pub mod session;
pub mod table;
pub mod vnc;

/// First adapter session id (rs_mobile.h RS_ADAPTER_ID_BASE). RSP session ids
/// count up from 1 and never reach it.
pub const ADAPTER_ID_BASE: u64 = 1 << 62;

pub fn is_adapter_id(id: u64) -> bool {
    id >= ADAPTER_ID_BASE
}
