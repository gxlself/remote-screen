//! macOS 系统账户密码校验（OpenDirectory）。见 06「Mac 平台层」。

use std::ffi::{c_char, c_int, CString};

use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use zeroize::Zeroizing;

const OUTPUT_LEN: usize = 256;

extern "C" {
    fn rs_sysauth_lookup(
        account: *const c_char,
        short_name: *mut c_char,
        short_len: usize,
        full_name: *mut c_char,
        full_len: usize,
        is_admin: *mut c_int,
        is_console: *mut c_int,
        msg: *mut c_char,
        msg_len: usize,
    ) -> i32;
    fn rs_sysauth_verify(
        short_name: *const c_char,
        password: *const u8,
        password_len: usize,
        msg: *mut c_char,
        msg_len: usize,
    ) -> i32;
}

/// Stateless macOS account authenticator backed by the OpenDirectory search node.
pub struct MacAuthenticator;

fn output_string(buffer: &[u8]) -> String {
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

fn unavailable(buffer: &[u8]) -> SysAuthError {
    let message = output_string(buffer);
    let message = if message.is_empty() {
        "OpenDirectory error".to_owned()
    } else {
        message
    };
    SysAuthError::Unavailable(message)
}

impl SystemAuthenticator for MacAuthenticator {
    fn lookup(&self, account: &str) -> Result<Option<SystemAccount>, SysAuthError> {
        let Ok(account) = CString::new(account) else {
            return Ok(None);
        };
        let mut short_name = [0_u8; OUTPUT_LEN];
        let mut full_name = [0_u8; OUTPUT_LEN];
        let mut message = [0_u8; OUTPUT_LEN];
        let (mut is_admin, mut is_console) = (0_i32, 0_i32);
        // SAFETY: `account` is a NUL-terminated CString; every output pointer
        // points to a live, writable buffer for the duration of this call.
        let result = unsafe {
            rs_sysauth_lookup(
                account.as_ptr(),
                short_name.as_mut_ptr().cast(),
                short_name.len(),
                full_name.as_mut_ptr().cast(),
                full_name.len(),
                &mut is_admin,
                &mut is_console,
                message.as_mut_ptr().cast(),
                message.len(),
            )
        };
        match result {
            1 => Ok(Some(SystemAccount {
                short_name: output_string(&short_name),
                full_name: output_string(&full_name),
                is_admin: is_admin != 0,
                is_console_user: is_console != 0,
            })),
            0 => Ok(None),
            -1 => Err(unavailable(&message)),
            other => Err(SysAuthError::Unavailable(format!(
                "OpenDirectory returned unexpected status {other}"
            ))),
        }
    }

    fn verify(&self, short_name: &str, password: &[u8]) -> Result<(), SysAuthError> {
        let Ok(short_name) = CString::new(short_name) else {
            return Err(SysAuthError::WrongPassword);
        };
        // Invalid UTF-8 is a rejected password and must not reach OpenDirectory.
        if std::str::from_utf8(password).is_err() {
            return Err(SysAuthError::WrongPassword);
        }
        let password = Zeroizing::new(password.to_vec());
        let mut message = [0_u8; OUTPUT_LEN];
        // SAFETY: `short_name` is NUL-terminated; the Zeroizing buffer and
        // message buffer remain valid for the duration of the native call.
        let result = unsafe {
            rs_sysauth_verify(
                short_name.as_ptr(),
                password.as_ptr(),
                password.len(),
                message.as_mut_ptr().cast(),
                message.len(),
            )
        };
        match result {
            0 => Ok(()),
            1 => Err(SysAuthError::WrongPassword),
            -1 => Err(unavailable(&message)),
            other => Err(SysAuthError::Unavailable(format!(
                "OpenDirectory returned unexpected status {other}"
            ))),
        }
    }
}
