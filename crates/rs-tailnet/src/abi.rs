//! C ABI v1. Function pointers follow `tailnet/include/rs_tailnet.h` exactly.
//!
//! Implementations must honor its buffer bounds and transfer ownership of every
//! successful dial's fresh descriptor. The code behind a table must remain loaded
//! for the lifetime of the process. Public fields also allow test function tables.

use std::{ffi::c_char, io};

pub const ABI_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Code {
    Ok = 0,
    Invalid = -1,
    State = -2,
    NotRunning = -3,
    Timeout = -4,
    Refused = -5,
    Unreachable = -6,
    Io = -7,
    Closed = -8,
    Unknown = i32::MIN,
}

impl Code {
    pub fn from_raw(raw: i64) -> Self {
        match raw {
            0 => Self::Ok,
            -1 => Self::Invalid,
            -2 => Self::State,
            -3 => Self::NotRunning,
            -4 => Self::Timeout,
            -5 => Self::Refused,
            -6 => Self::Unreachable,
            -7 => Self::Io,
            -8 => Self::Closed,
            _ => Self::Unknown,
        }
    }
}

pub fn code_to_io_kind(code: Code) -> io::ErrorKind {
    match code {
        Code::Invalid => io::ErrorKind::InvalidInput,
        Code::NotRunning => io::ErrorKind::NotConnected,
        Code::Timeout => io::ErrorKind::TimedOut,
        Code::Refused => io::ErrorKind::ConnectionRefused,
        Code::Unreachable => io::ErrorKind::HostUnreachable,
        Code::Closed => io::ErrorKind::BrokenPipe,
        Code::Ok | Code::State | Code::Io | Code::Unknown => io::ErrorKind::Other,
    }
}

pub const SYMBOLS: [&str; 10] = [
    "rs_ts_abi_version",
    "rs_ts_start",
    "rs_ts_stop",
    "rs_ts_login",
    "rs_ts_logout",
    "rs_ts_rebind",
    "rs_ts_status",
    "rs_ts_next_event",
    "rs_ts_dial_udp",
    "rs_ts_dial_tcp",
];

#[derive(Debug)]
pub struct Api {
    pub abi_version: unsafe extern "C" fn() -> i32,
    pub start: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32,
    pub stop: unsafe extern "C" fn() -> i32,
    pub login: unsafe extern "C" fn(*mut c_char, usize) -> i32,
    pub logout: unsafe extern "C" fn(*mut c_char, usize) -> i32,
    pub rebind: unsafe extern "C" fn() -> i32,
    pub status: unsafe extern "C" fn(*mut c_char, usize) -> i64,
    pub next_event: unsafe extern "C" fn(*mut c_char, usize, i32) -> i64,
    pub dial_udp: unsafe extern "C" fn(*const c_char, u16, i32, *mut c_char, usize) -> i32,
    pub dial_tcp: unsafe extern "C" fn(*const c_char, u16, i32, *mut c_char, usize) -> i32,
}
