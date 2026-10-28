//! Windows system-account authentication backed by NetAPI, WTS and LogonUserW.

use std::{ffi::c_void, mem::size_of, ptr, slice};

use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE, HLOCAL},
        NetworkManagement::NetManagement::{
            NERR_Success, NERR_UserNotFound, NetApiBufferFree, NetQueryDisplayInformation,
            NetUserGetInfo, NetUserGetLocalGroups, LG_INCLUDE_INDIRECT, LOCALGROUP_USERS_INFO_0,
            MAX_PREFERRED_LENGTH, NET_DISPLAY_USER, USER_INFO_23, USER_INFO_24,
        },
        Security::{
            Authentication::Identity::{
                NameDisplay, NameSamCompatible, NameUserPrincipal, TranslateNameW,
                EXTENDED_NAME_FORMAT,
            },
            Authorization::ConvertSidToStringSidW,
            LogonUserW, LookupAccountNameW, LOGON32_LOGON, LOGON32_LOGON_INTERACTIVE,
            LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT, PSID, SID_NAME_USE,
        },
        System::{
            RemoteDesktop::{
                WTSDomainName, WTSFreeMemory, WTSGetActiveConsoleSessionId,
                WTSQuerySessionInformationW, WTSUserName, WTS_INFO_CLASS,
            },
            SystemInformation::{ComputerNameNetBIOS, GetComputerNameExW},
        },
    },
};

use crate::account::{
    canonical_short_name, is_admin, map_logon_error, names_equal, parse_account, password_utf16,
    pick_by_full_name, AccountInput, LogonOutcome,
};

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_MORE_DATA: u32 = 234;
const MAX_LOCAL_USERS: usize = 10_000;

/// Stateless Windows account authenticator. Call its blocking methods from
/// `spawn_blocking` in the SYSTEM service; domain lookups can take seconds.
pub struct WinAuthenticator;

struct NetBuffer(*mut c_void);

impl Drop for NetBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper uniquely owns a buffer allocated by NetAPI.
            unsafe { NetApiBufferFree(Some(self.0)) };
        }
    }
}

struct WtsBuffer(PWSTR);

impl Drop for WtsBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: WTSQuerySessionInformationW allocated this owned buffer.
            unsafe { WTSFreeMemory(self.0.as_ptr().cast()) };
        }
    }
}

struct LocalBuffer(PWSTR);

impl Drop for LocalBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: ConvertSidToStringSidW allocated this buffer with LocalAlloc.
            unsafe { LocalFree(Some(HLOCAL(self.0.as_ptr().cast()))) };
        }
    }
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: this wrapper uniquely owns a token returned by LogonUserW.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct ConsoleIdentity {
    domain: String,
    user: String,
}

impl ConsoleIdentity {
    fn sam_name(&self) -> String {
        format!(r"{}\{}", self.domain, self.user)
    }

    fn is_local(&self, computer: &str) -> bool {
        self.domain == "." || names_equal(&self.domain, computer)
    }
}

fn unavailable(api: &str, code: u32) -> SysAuthError {
    // Include only the API name and code, never caller-supplied account text.
    SysAuthError::Unavailable(format!("{api} error {code}"))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_string(value: &[u16]) -> Option<String> {
    let end = value.iter().position(|unit| *unit == 0)?;
    String::from_utf16(&value[..end]).ok()
}

/// # Safety
/// A non-null value must point to a live, NUL-terminated system string.
unsafe fn native_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the caller keeps the API allocation containing the string alive.
    unsafe { value.to_string().ok() }
}

fn computer_name() -> Result<String, SysAuthError> {
    // NetBIOS names fit in 16 UTF-16 code units including the terminator.
    let mut buffer = [0_u16; 256];
    let mut length = buffer.len() as u32;
    // SAFETY: buffer is writable for the supplied capacity; length is an out value.
    let result = unsafe {
        GetComputerNameExW(
            ComputerNameNetBIOS,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    };
    if result.is_err() {
        // SAFETY: GetLastError reads this thread's error after the failed call.
        return Err(unavailable("GetComputerNameExW", unsafe {
            GetLastError().0
        }));
    }
    wide_string(&buffer)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| SysAuthError::Unavailable("invalid computer name".into()))
}

fn wts_value(session: u32, class: WTS_INFO_CLASS) -> Option<String> {
    let mut buffer = WtsBuffer(PWSTR::null());
    let mut bytes = 0_u32;
    // SAFETY: WTS writes an owned buffer and byte count into valid out pointers.
    unsafe { WTSQuerySessionInformationW(None, session, class, &mut buffer.0, &mut bytes) }.ok()?;
    if buffer.0.is_null() || bytes < 2 || !bytes.is_multiple_of(2) {
        return None;
    }
    // SAFETY: WTS returned `bytes` readable bytes aligned for UTF-16; the
    // WtsBuffer remains alive until after the string is copied.
    let value = unsafe { slice::from_raw_parts(buffer.0.as_ptr(), bytes as usize / 2) };
    wide_string(value)
}

fn console_identity() -> Option<ConsoleIdentity> {
    // SAFETY: this API has no pointer parameters or preconditions.
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    if session == u32::MAX {
        return None;
    }
    let user = wts_value(session, WTSUserName)?;
    let domain = wts_value(session, WTSDomainName)?;
    if user.is_empty() {
        return None;
    }
    Some(ConsoleIdentity {
        domain: if domain.is_empty() {
            ".".to_owned()
        } else {
            domain
        },
        user,
    })
}

fn translated_name(sam_name: &str, format: EXTENDED_NAME_FORMAT) -> Option<String> {
    let source = wide(sam_name);
    let mut size = 0_u32;
    // SAFETY: the source is NUL-terminated; a null output requests its size.
    unsafe {
        TranslateNameW(
            PCWSTR(source.as_ptr()),
            NameSamCompatible,
            format,
            None,
            &mut size,
        )
    };
    if size == 0 || size > 32_768 {
        return None;
    }
    let mut output = vec![0_u16; size as usize];
    // SAFETY: output is writable for the capacity returned by the sizing call.
    let success = unsafe {
        TranslateNameW(
            PCWSTR(source.as_ptr()),
            NameSamCompatible,
            format,
            Some(PWSTR(output.as_mut_ptr())),
            &mut size,
        )
    };
    if success {
        wide_string(&output).filter(|name| !name.is_empty())
    } else {
        None
    }
}

fn microsoft_console_upn(console: &ConsoleIdentity, computer: &str) -> Option<String> {
    if names_equal(&console.domain, "MicrosoftAccount") && console.user.contains('@') {
        return Some(console.user.clone());
    }
    if !console.is_local(computer) {
        return None;
    }
    // WTS often reports a Microsoft account by its local SAM alias. Level 24
    // identifies the internet principal attached to that same local account.
    let user = wide(&console.user);
    let mut raw = ptr::null_mut();
    // SAFETY: user is NUL-terminated and raw is a valid NetAPI out pointer.
    let status = unsafe { NetUserGetInfo(PCWSTR::null(), PCWSTR(user.as_ptr()), 24, &mut raw) };
    let buffer = NetBuffer(raw.cast());
    if status != NERR_Success || buffer.0.is_null() {
        return None;
    }
    // SAFETY: a successful level-24 query returns one live USER_INFO_24.
    let info = unsafe { &*buffer.0.cast::<USER_INFO_24>() };
    if !info.usri24_internet_identity.as_bool() {
        return None;
    }
    // SAFETY: the NetBuffer still owns the level-24 string fields.
    unsafe { native_string(info.usri24_internet_principal_name) }.filter(|name| !name.is_empty())
}

fn console_upn(console: &ConsoleIdentity, computer: &str) -> Option<String> {
    translated_name(&console.sam_name(), NameUserPrincipal)
        .or_else(|| microsoft_console_upn(console, computer))
}

fn account_sid(account: &str) -> Option<String> {
    let name = wide(account);
    let mut sid_size = 0;
    let mut domain_size = 0;
    let mut use_type = SID_NAME_USE::default();
    // SAFETY: null output buffers request their sizes for a valid wide name.
    let sized = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            None,
            &mut sid_size,
            None,
            &mut domain_size,
            &mut use_type,
        )
    };
    // SAFETY: this reads the error from the sizing call without changing it.
    if sized.is_ok() || unsafe { GetLastError().0 } != ERROR_INSUFFICIENT_BUFFER || sid_size == 0 {
        return None;
    }
    // SID structures contain DWORD fields, so retain at least DWORD alignment.
    let mut sid = vec![0_u32; (sid_size as usize).div_ceil(size_of::<u32>())];
    let mut domain = vec![0_u16; domain_size as usize];
    let sid_pointer = PSID(sid.as_mut_ptr().cast());
    // SAFETY: SID and domain buffers have the sizes returned by the first call.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            Some(sid_pointer),
            &mut sid_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut use_type,
        )
    }
    .ok()?;
    let mut text = LocalBuffer(PWSTR::null());
    // SAFETY: sid_pointer references a SID just written by LookupAccountNameW;
    // text uniquely owns the LocalAlloc string returned by this call.
    unsafe { ConvertSidToStringSidW(sid_pointer, &mut text.0) }.ok()?;
    // SAFETY: text owns the NUL-terminated SID string throughout this copy.
    unsafe { native_string(text.0) }
}

fn user_is_admin(account: &str) -> Result<bool, SysAuthError> {
    let user = wide(account);
    let mut raw = ptr::null_mut();
    let mut entries = 0;
    let mut total = 0;
    // SAFETY: user is NUL-terminated and every out pointer is live and writable.
    let status = unsafe {
        NetUserGetLocalGroups(
            PCWSTR::null(),
            PCWSTR(user.as_ptr()),
            0,
            LG_INCLUDE_INDIRECT,
            &mut raw,
            MAX_PREFERRED_LENGTH,
            &mut entries,
            &mut total,
        )
    };
    let buffer = NetBuffer(raw.cast());
    if status != NERR_Success {
        return Err(unavailable("NetUserGetLocalGroups", status));
    }
    if entries == 0 {
        return Ok(false);
    }
    if buffer.0.is_null() {
        return Err(SysAuthError::Unavailable(
            "missing local groups buffer".into(),
        ));
    }
    // SAFETY: a successful level-0 query returns `entries` group records, all
    // retained by the NetBuffer while their strings are read.
    let groups = unsafe {
        slice::from_raw_parts(buffer.0.cast::<LOCALGROUP_USERS_INFO_0>(), entries as usize)
    };
    let mut group_sids = Vec::new();
    for group in groups {
        // SAFETY: the containing NetBuffer owns the NUL-terminated group name.
        if let Some(name) = unsafe { native_string(group.lgrui0_name) } {
            if let Some(sid) = account_sid(&name) {
                group_sids.push(sid);
            }
        }
    }
    Ok(is_admin(&group_sids))
}

struct LocalUser {
    short_name: String,
    full_name: String,
}

fn user_info(user: &str) -> Result<Option<LocalUser>, SysAuthError> {
    let name = wide(user);
    let mut raw = ptr::null_mut();
    // SAFETY: name is NUL-terminated and raw is a valid out pointer.
    let status = unsafe { NetUserGetInfo(PCWSTR::null(), PCWSTR(name.as_ptr()), 23, &mut raw) };
    let buffer = NetBuffer(raw.cast());
    if status == NERR_UserNotFound {
        return Ok(None);
    }
    if status != NERR_Success {
        return Err(unavailable("NetUserGetInfo", status));
    }
    if buffer.0.is_null() {
        return Err(SysAuthError::Unavailable("missing user info buffer".into()));
    }
    // SAFETY: a successful level-23 query returns one live USER_INFO_23 whose
    // string fields stay valid until NetBuffer is dropped.
    let (short_name, full_name) = unsafe {
        let info = &*buffer.0.cast::<USER_INFO_23>();
        (
            native_string(info.usri23_name),
            native_string(info.usri23_full_name),
        )
    };
    let short_name = short_name
        .filter(|name| !name.is_empty())
        .ok_or_else(|| SysAuthError::Unavailable("user record has no short name".into()))?;
    let full_name = full_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| short_name.clone());
    Ok(Some(LocalUser {
        short_name,
        full_name,
    }))
}

fn find_full_name(query: &str) -> Result<Option<String>, SysAuthError> {
    let mut index = 0;
    let mut candidates = Vec::new();
    loop {
        let mut raw = ptr::null_mut();
        let mut count = 0;
        let requested = 1_000.min(MAX_LOCAL_USERS - candidates.len()) as u32;
        // SAFETY: level 1 requests NET_DISPLAY_USER records in a NetAPI-owned
        // page; count and raw are valid out pointers.
        let status = unsafe {
            NetQueryDisplayInformation(
                PCWSTR::null(),
                1,
                index,
                requested,
                MAX_PREFERRED_LENGTH,
                &mut count,
                &mut raw,
            )
        };
        let buffer = NetBuffer(raw);
        if status != NERR_Success && status != ERROR_MORE_DATA {
            return Err(unavailable("NetQueryDisplayInformation", status));
        }
        if count > requested || (count != 0 && buffer.0.is_null()) {
            return Err(SysAuthError::Unavailable(
                "invalid user display buffer".into(),
            ));
        }
        if count == 0 {
            if status == NERR_Success {
                break;
            }
            return Err(SysAuthError::Unavailable(
                "user enumeration made no progress".into(),
            ));
        }
        // SAFETY: NetAPI returned count valid level-1 records; buffer owns both
        // the records and the strings copied in this loop.
        let page =
            unsafe { slice::from_raw_parts(buffer.0.cast::<NET_DISPLAY_USER>(), count as usize) };
        for entry in page {
            // SAFETY: the page buffer owns these NUL-terminated string fields.
            let (name, full_name) = unsafe {
                (
                    native_string(entry.usri1_name),
                    native_string(entry.usri1_full_name),
                )
            };
            let name =
                name.ok_or_else(|| SysAuthError::Unavailable("invalid display user name".into()))?;
            candidates.push((name, full_name.unwrap_or_default()));
        }
        if status == NERR_Success {
            break;
        }
        if candidates.len() >= MAX_LOCAL_USERS {
            // An incomplete enumeration cannot establish that a full name is
            // unique, so never authorize from a truncated list.
            return Err(SysAuthError::Unavailable(
                "local user enumeration limit reached".into(),
            ));
        }
        let next = page[page.len() - 1].usri1_next_index;
        if next <= index {
            return Err(SysAuthError::Unavailable(
                "user enumeration made no progress".into(),
            ));
        }
        index = next;
    }
    Ok(pick_by_full_name(&candidates, query).map(str::to_owned))
}

fn local_account(
    user: &str,
    computer: &str,
    console: Option<&ConsoleIdentity>,
) -> Result<Option<SystemAccount>, SysAuthError> {
    let info = match user_info(user)? {
        Some(info) => info,
        None => {
            let Some(name) = find_full_name(user)? else {
                return Ok(None);
            };
            let Some(info) = user_info(&name)? else {
                return Ok(None);
            };
            info
        }
    };
    let short_name = canonical_short_name(&AccountInput::Local {
        user: info.short_name,
    });
    let is_console_user = console.is_some_and(|console| {
        console.is_local(computer) && names_equal(&short_name, &console.user)
    });
    Ok(Some(SystemAccount {
        is_admin: user_is_admin(&short_name)?,
        short_name,
        full_name: info.full_name,
        is_console_user,
    }))
}

fn console_account(
    input: &AccountInput,
    computer: &str,
    console: Option<&ConsoleIdentity>,
) -> Option<SystemAccount> {
    let console = console?;
    let matches = match input {
        AccountInput::Domain { domain, user } => {
            (names_equal(domain, &console.domain) && names_equal(user, &console.user))
                || (names_equal(domain, "MicrosoftAccount")
                    && microsoft_console_upn(console, computer)
                        .is_some_and(|upn| names_equal(user, &upn)))
        }
        AccountInput::Upn { upn } => {
            console_upn(console, computer).is_some_and(|console_upn| names_equal(upn, &console_upn))
        }
        AccountInput::Local { .. } => false,
    };
    if !matches {
        return None;
    }
    let short_name = canonical_short_name(input);
    let sam_name = console.sam_name();
    let full_name = translated_name(&sam_name, NameDisplay)
        .or_else(|| {
            console
                .is_local(computer)
                .then(|| {
                    user_info(&console.user)
                        .ok()
                        .flatten()
                        .map(|info| info.full_name)
                })
                .flatten()
        })
        .unwrap_or_else(|| short_name.clone());
    Some(SystemAccount {
        is_admin: user_is_admin(&sam_name).unwrap_or(false),
        short_name,
        full_name,
        is_console_user: true,
    })
}

fn logon(
    user: &[u16],
    domain: Option<&[u16]>,
    password: &[u16],
    kind: LOGON32_LOGON,
) -> Result<(), u32> {
    let mut token = Token(HANDLE::default());
    let domain = domain.map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
    // SAFETY: all strings are NUL-terminated and live through the call. The
    // password is borrowed from Zeroizing storage; Token owns any output handle.
    unsafe {
        if LogonUserW(
            PCWSTR(user.as_ptr()),
            domain,
            PCWSTR(password.as_ptr()),
            kind,
            LOGON32_PROVIDER_DEFAULT,
            &mut token.0,
        )
        .is_ok()
        {
            Ok(())
        } else {
            // Capture the error before Token::drop can call CloseHandle.
            Err(GetLastError().0)
        }
    }
}

impl SystemAuthenticator for WinAuthenticator {
    /// May block for seconds; invoke from `spawn_blocking` in the SYSTEM service.
    fn lookup(&self, account: &str) -> Result<Option<SystemAccount>, SysAuthError> {
        let computer = computer_name()?;
        let Some(input) = parse_account(account, &computer) else {
            return Ok(None);
        };
        let console = console_identity();
        match &input {
            AccountInput::Local { user } => local_account(user, &computer, console.as_ref()),
            AccountInput::Domain { .. } | AccountInput::Upn { .. } => {
                Ok(console_account(&input, &computer, console.as_ref()))
            }
        }
    }

    /// Must run in the SYSTEM service, from `spawn_blocking`: LogonUserW may
    /// block for seconds when a domain controller is unreachable. Failed logons
    /// also count toward Windows' own account-lockout policy.
    fn verify(&self, short_name: &str, password: &[u8]) -> Result<(), SysAuthError> {
        let password = password_utf16(password).ok_or(SysAuthError::WrongPassword)?;
        let computer = computer_name()?;
        let input = parse_account(short_name, &computer).ok_or(SysAuthError::WrongPassword)?;
        let (user, domain) = match &input {
            AccountInput::Local { user } => (wide(user), Some(wide("."))),
            AccountInput::Domain { domain, user } => (wide(user), Some(wide(domain))),
            AccountInput::Upn { upn } => (wide(upn), None),
        };
        let Err(code) = logon(&user, domain.as_deref(), &password, LOGON32_LOGON_NETWORK) else {
            return Ok(());
        };
        match map_logon_error(code, false) {
            LogonOutcome::WrongPassword => Err(SysAuthError::WrongPassword),
            LogonOutcome::Unavailable => Err(unavailable("LogonUserW", code)),
            LogonOutcome::TryInteractive => {
                let Err(code) = logon(
                    &user,
                    domain.as_deref(),
                    &password,
                    LOGON32_LOGON_INTERACTIVE,
                ) else {
                    return Ok(());
                };
                match map_logon_error(code, true) {
                    LogonOutcome::WrongPassword => Err(SysAuthError::WrongPassword),
                    LogonOutcome::Unavailable | LogonOutcome::TryInteractive => {
                        Err(unavailable("LogonUserW", code))
                    }
                }
            }
        }
    }
}
