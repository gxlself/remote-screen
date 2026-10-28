//! Platform-independent rules for the Windows service/session process boundary.

/// Only SYSTEM: session processes run with the service's SYSTEM token.
pub const SESSION_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)";
/// SYSTEM has full access. CONSOLE LOGON (not RDP) has data read/write access:
/// FILE_GENERIC_READ | FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES = 0x12018b.
/// Excludes FILE_CREATE_PIPE_INSTANCE (0x4), preventing pipe instance squatting.
pub const UI_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;0x12018b;;;S-1-2-1)";

/// Quotes one argument using the Windows backslash/double-quote rules.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        let count = if ch == '"' { slashes * 2 + 1 } else { slashes };
        quoted.extend(std::iter::repeat_n('\\', count));
        quoted.push(ch);
        slashes = 0;
    }
    quoted.extend(std::iter::repeat_n('\\', slashes * 2));
    quoted.push('"');
    quoted
}

pub fn command_line(exe: &str, args: &[&str]) -> String {
    let mut line = quote_arg(exe);
    for arg in args {
        line.push(' ');
        line.push_str(&quote_arg(arg));
    }
    line
}

/// Fail closed until the supervisor has an expected child PID.
pub fn verify_client(expected_pid: Option<u32>, actual_pid: u32) -> bool {
    expected_pid == Some(actual_pid)
}

/// Converts WTS's no-console sentinel to an absent session.
pub fn console_session_id(raw: u32) -> Option<u32> {
    (raw != u32::MAX).then_some(raw)
}

/// A UI connection is accepted only from the currently attached console.
pub fn verify_console_client(active_session: Option<u32>, client_session: u32) -> bool {
    active_session.is_some_and(|active| active != u32::MAX && active == client_session)
}

/// Reject embedded NULs instead of silently truncating strings passed to Win32.
pub fn nul_terminated_utf16(value: &str) -> Option<Vec<u16>> {
    if value.contains('\0') {
        return None;
    }
    Some(value.encode_utf16().chain(std::iter::once(0)).collect())
}

/// Decodes a complete, NUL-terminated UOI_NAME result without lossy conversion.
pub fn desktop_name_from_utf16(value: &[u16]) -> Option<String> {
    let end = value.iter().position(|unit| *unit == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf16(&value[..end]).ok()
}

#[derive(Debug, Default)]
pub struct DesktopTracker {
    last: Option<String>,
}

impl DesktopTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// An inaccessible desktop leaves the last successful observation intact.
    pub fn observe(&mut self, name: Option<&str>) -> Option<String> {
        let name = name?;
        if self
            .last
            .as_deref()
            .is_some_and(|last| last.eq_ignore_ascii_case(name))
        {
            return None;
        }
        self.last = Some(name.to_owned());
        self.last.clone()
    }
}
