//! Console-user directory policy, using Windows strings on every target.

use std::time::{Duration, Instant};

pub const RECEIVED_DIR: &str = "received";
pub const CACHE_TTL: Duration = Duration::from_secs(10);

pub fn fallback_dir(data_dir: &str) -> String {
    format!(r"{}\{RECEIVED_DIR}", data_dir.trim_end_matches('\\'))
}

pub fn strip_verbatim(path: &str) -> String {
    match path.strip_prefix(r"\\?\") {
        Some(rest) => match rest.get(..4) {
            Some(prefix) if prefix.eq_ignore_ascii_case(r"UNC\") => {
                format!(r"\\{}", &rest[4..])
            }
            _ if rest.as_bytes().get(1..3) == Some(b":\\")
                && rest.as_bytes()[0].is_ascii_alphabetic() =>
            {
                rest.to_owned()
            }
            _ => path.to_owned(),
        },
        None => path.to_owned(),
    }
}

/// Compare whole components, without consulting the host operating system.
pub fn is_under(path: &str, root: &str) -> bool {
    let mut path = components(path);
    let mut root = components(root).peekable();
    root.peek().is_some()
        && root.all(|prefix| {
            path.next()
                .is_some_and(|part| part.eq_ignore_ascii_case(prefix))
        })
}

fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|part| !part.is_empty())
}

pub fn forbidden_roots(env: &dyn Fn(&str) -> Option<String>, data_dir: &str) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let candidates = [
        env("SystemRoot").unwrap_or_else(|| r"C:\Windows".to_owned()),
        env("ProgramFiles").unwrap_or_default(),
        env("ProgramFiles(x86)").unwrap_or_default(),
        env("ProgramW6432").unwrap_or_default(),
        data_dir.to_owned(),
    ];
    for candidate in candidates {
        if !candidate.is_empty()
            && !roots.iter().any(|root| {
                root.trim_end_matches('\\')
                    .eq_ignore_ascii_case(candidate.trim_end_matches('\\'))
            })
        {
            roots.push(candidate);
        }
    }
    roots
}

pub fn is_forbidden(canonical: &str, roots: &[String]) -> bool {
    canonical.starts_with(r"\\") || roots.iter().any(|root| is_under(canonical, root))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    NoConsole,
    NoUser,
    NoFolder,
    Forbidden,
    NotWritable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub console: Option<u32>,
    pub has_user: bool,
    /// Canonical, without a verbatim prefix; UNC is retained only for rejection.
    pub downloads: Option<String>,
    pub profile: Option<String>,
    pub downloads_writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    User {
        downloads: String,
        home: Option<String>,
    },
    Fallback {
        reason: Fallback,
        home: Option<String>,
    },
}

pub fn choose(probe: &Probe, forbidden: &[String]) -> Choice {
    if probe.console.is_none() {
        return Choice::Fallback {
            reason: Fallback::NoConsole,
            home: None,
        };
    }
    if !probe.has_user {
        return Choice::Fallback {
            reason: Fallback::NoUser,
            home: None,
        };
    }
    let home = probe
        .profile
        .as_ref()
        .filter(|profile| !is_forbidden(profile, forbidden))
        .cloned();
    let reason = match &probe.downloads {
        None => Fallback::NoFolder,
        Some(downloads) if is_forbidden(downloads, forbidden) => Fallback::Forbidden,
        Some(_) if !probe.downloads_writable => Fallback::NotWritable,
        Some(downloads) => {
            return Choice::User {
                downloads: downloads.clone(),
                home,
            };
        }
    };
    Choice::Fallback { reason, home }
}

pub fn cache_fresh(
    cached_session: u32,
    cached_at: Instant,
    session: u32,
    now: Instant,
    ttl: Duration,
) -> bool {
    cached_session == session
        && now
            .checked_duration_since(cached_at)
            .is_some_and(|age| age < ttl)
}
