//! Pure account parsing, matching and password conversion for Windows authentication.

use zeroize::Zeroizing;

/// The syntactic form supplied by a caller when selecting a Windows account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountInput {
    Local { user: String },
    Domain { domain: String, user: String },
    Upn { upn: String },
}

pub(crate) fn names_equal(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

const FORBIDDEN_USERNAME_CHARS: &[char] = &[
    '/', '"', '[', ']', ':', ';', '|', '=', ',', '+', '*', '?', '<', '>', '\\',
];

fn valid_name_part(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            !character.is_control() && !FORBIDDEN_USERNAME_CHARS.contains(&character)
        })
}

/// Parse a Windows account spelling into its canonical kind.
pub fn parse_account(input: &str, computer_name: &str) -> Option<AccountInput> {
    let input = input.trim();
    if input.is_empty() || input.chars().count() > 256 {
        return None;
    }

    if let Some((domain, user)) = input.split_once('\\') {
        if !valid_name_part(domain) || !valid_name_part(user) {
            return None;
        }
        if domain == "." || names_equal(domain, computer_name) {
            return Some(AccountInput::Local {
                user: user.to_owned(),
            });
        }
        return Some(AccountInput::Domain {
            domain: domain.to_owned(),
            user: user.to_owned(),
        });
    }

    if input.contains('@') {
        let (user, domain) = input.split_once('@')?;
        if !valid_name_part(user) || !valid_name_part(domain) {
            return None;
        }
        return Some(AccountInput::Upn {
            upn: input.to_owned(),
        });
    }

    valid_name_part(input).then(|| AccountInput::Local {
        user: input.to_owned(),
    })
}

/// Return the spelling accepted by `LogonUserW` and used in account records.
pub fn canonical_short_name(account: &AccountInput) -> String {
    match account {
        AccountInput::Local { user } => user.clone(),
        AccountInput::Domain { domain, user } => format!(r"{domain}\{user}"),
        AccountInput::Upn { upn } => upn.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogonOutcome {
    WrongPassword,
    TryInteractive,
    Unavailable,
}

/// Map Win32 logon errors without exposing account state to the peer.
/// Restriction, expiry, disabled-account and lockout errors count as rejected
/// passwords for host-core's failure counter too.
pub fn map_logon_error(code: u32, interactive_attempt: bool) -> LogonOutcome {
    match code {
        1326 | 1327 | 1330 | 1331 | 1907 | 1909 => LogonOutcome::WrongPassword,
        1385 if !interactive_attempt => LogonOutcome::TryInteractive,
        _ => LogonOutcome::Unavailable,
    }
}

pub const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

pub fn is_admin(group_sids: &[String]) -> bool {
    group_sids.iter().any(|sid| sid == ADMINISTRATORS_SID)
}

/// Match an account's full name. Ambiguous names are deliberately rejected.
pub fn pick_by_full_name<'a>(candidates: &'a [(String, String)], query: &str) -> Option<&'a str> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let mut matches = candidates
        .iter()
        .filter(|(_, full_name)| names_equal(full_name.trim(), query));
    let candidate = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(candidate.0.as_str())
    }
}

/// Convert a UTF-8 password to the NUL-terminated UTF-16 expected by Win32.
/// Invalid UTF-8 and embedded NULs are rejected before any system call.
pub fn password_utf16(password: &[u8]) -> Option<Zeroizing<Vec<u16>>> {
    let password = std::str::from_utf8(password).ok()?;
    if password.contains('\0') {
        return None;
    }
    // Reserve before writing the secret: reallocating a populated Vec could
    // leave an unzeroized copy in the old allocation.
    let mut utf16 = Zeroizing::new(Vec::with_capacity(password.encode_utf16().count() + 1));
    utf16.extend(password.encode_utf16());
    utf16.push(0);
    Some(utf16)
}
