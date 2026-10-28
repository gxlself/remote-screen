#![cfg_attr(not(windows), allow(dead_code))]

/// Mirrors `UI_PIPE_NAME` in `crates/host-win/src/pipe_proto.rs:25`.
pub(crate) const DEFAULT_PIPE: &str = r"\\.\pipe\RemoteHost.ui";
pub(crate) const PIPE_ENV: &str = "RS_AGENT_PIPE";
/// Console-user rights in `UI_PIPE_SDDL`, `crates/host-win/src/winproc_model.rs:8`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SERVER_GRANTED: u32 = 0x0012_018b;
/// Data read/write, attributes read/write, READ_CONTROL and SYNCHRONIZE only.
pub(crate) const CLIENT_ACCESS: u32 = 0x0012_0183;
pub(crate) const SID_LOCAL_SYSTEM: &str = "S-1-5-18";
pub(crate) const SID_ADMINISTRATORS: &str = "S-1-5-32-544";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reject {
    Session(u32),
    Owner(String),
    Query { step: &'static str, code: u32 },
}

impl std::fmt::Display for Reject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(session) => write!(formatter, "server session {session} is not 0"),
            Self::Owner(sid) => {
                write!(
                    formatter,
                    "server owner {sid} is not SYSTEM or Administrators"
                )
            }
            Self::Query { step, code } => write!(formatter, "{step} failed: {code}"),
        }
    }
}

pub(crate) fn check_server(
    session: Result<u32, Reject>,
    owner_sid: Result<String, Reject>,
) -> Result<(), Reject> {
    let session = session?;
    if session != 0 {
        return Err(Reject::Session(session));
    }
    let owner = owner_sid?;
    if !owner.eq_ignore_ascii_case(SID_LOCAL_SYSTEM)
        && !owner.eq_ignore_ascii_case(SID_ADMINISTRATORS)
    {
        return Err(Reject::Owner(owner));
    }
    Ok(())
}

pub(crate) fn valid_pipe_name(name: &str) -> bool {
    const PREFIX: &str = r"\\.\pipe\";
    name.len() > PREFIX.len()
        && name
            .get(..PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
        && name.chars().count() <= 256
        && !name.contains(['\0', '/'])
}

pub(crate) fn pipe_name(env: Option<&str>) -> String {
    env.filter(|name| valid_pipe_name(name))
        .unwrap_or(DEFAULT_PIPE)
        .to_owned()
}

#[derive(Default)]
pub(crate) struct RejectLog {
    last: Option<Reject>,
    // Count modulo 60, so an arbitrarily long outage cannot overflow.
    consecutive: u8,
}

impl RejectLog {
    pub(crate) fn should_warn(&mut self, reason: &Reject) -> bool {
        let same_reason = match (&self.last, reason) {
            (Some(Reject::Session(_)), Reject::Session(_))
            | (Some(Reject::Query { .. }), Reject::Query { .. }) => true,
            (Some(Reject::Owner(previous)), Reject::Owner(current)) => previous == current,
            _ => false,
        };
        if !same_reason {
            self.last = Some(reason.clone());
            self.consecutive = 1;
            return true;
        }
        self.consecutive = (self.consecutive + 1) % 60;
        self.consecutive == 0
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_system_and_administrators_in_session_zero() {
        for sid in [
            SID_LOCAL_SYSTEM,
            SID_ADMINISTRATORS,
            "s-1-5-18",
            "s-1-5-32-544",
        ] {
            assert_eq!(check_server(Ok(0), Ok(sid.into())), Ok(()));
        }
    }

    #[test]
    fn rejects_untrusted_owners() {
        for sid in ["S-1-5-21-1-2-3-1001", "S-1-5-32-545", "S-1-5-19", ""] {
            assert_eq!(
                check_server(Ok(0), Ok(sid.into())),
                Err(Reject::Owner(sid.into()))
            );
        }
    }

    #[test]
    fn session_rejection_precedes_owner_check() {
        let owner_query = Reject::Query {
            step: "GetSecurityInfo",
            code: 5,
        };
        for owner in [Ok(SID_LOCAL_SYSTEM.into()), Err(owner_query)] {
            assert_eq!(check_server(Ok(1), owner), Err(Reject::Session(1)));
        }
    }

    #[test]
    fn session_query_failure_precedes_owner_query_failure() {
        let session_query = Reject::Query {
            step: "GetNamedPipeServerSessionId",
            code: 6,
        };
        let owner_query = Reject::Query {
            step: "GetSecurityInfo",
            code: 5,
        };
        assert_eq!(
            check_server(Err(session_query.clone()), Err(owner_query)),
            Err(session_query)
        );
    }

    #[test]
    fn preserves_owner_query_failure() {
        let query = Reject::Query {
            step: "ConvertSidToStringSidW",
            code: 1337,
        };
        assert_eq!(check_server(Ok(0), Err(query.clone())), Err(query));
    }

    #[test]
    fn uses_default_or_valid_local_pipe_override() {
        assert_eq!(pipe_name(None), DEFAULT_PIPE);
        for name in [DEFAULT_PIPE, r"\\.\pipe\x", r"\\.\PIPE\x"] {
            assert!(valid_pipe_name(name));
            assert_eq!(pipe_name(Some(name)), name);
        }
    }

    #[test]
    fn rejects_invalid_pipe_names() {
        for name in [
            r"C:\x",
            "",
            r"\\.\pipe\",
            "\\\\.\\pipe\\x\0suffix",
            r"\\.\pipe\x/y",
            r"\\hs.example.com\pipe\x",
            "ééééé",
        ] {
            assert!(!valid_pipe_name(name), "{name:?}");
            assert_eq!(pipe_name(Some(name)), DEFAULT_PIPE);
        }
    }

    #[test]
    fn pipe_name_limit_counts_characters() {
        for character in ['x', '管'] {
            let name = format!(r"\\.\pipe\{}", character.to_string().repeat(247));
            assert_eq!(name.chars().count(), 256);
            assert_eq!(pipe_name(Some(&name)), name);
            let too_long = format!("{name}{character}");
            assert_eq!(too_long.chars().count(), 257);
            assert_eq!(pipe_name(Some(&too_long)), DEFAULT_PIPE);
        }
    }

    #[test]
    fn warns_on_first_and_every_sixtieth_consecutive_rejection() {
        let mut log = RejectLog::default();
        let reason = Reject::Session(1);
        for attempt in 1..=181 {
            assert_eq!(
                log.should_warn(&reason),
                attempt == 1 || attempt % 60 == 0,
                "attempt {attempt}"
            );
        }
    }

    #[test]
    fn warns_when_variant_or_owner_changes_and_after_reset() {
        let mut log = RejectLog::default();
        for reason in [
            Reject::Session(1),
            Reject::Owner("S-1-5-19".into()),
            Reject::Owner("S-1-5-32-545".into()),
            Reject::Query {
                step: "GetSecurityInfo",
                code: 5,
            },
        ] {
            assert!(log.should_warn(&reason));
            assert!(!log.should_warn(&reason));
            log.reset();
            assert!(log.should_warn(&reason));
        }
    }

    #[test]
    fn session_numbers_and_query_details_share_a_reason_kind() {
        let mut log = RejectLog::default();
        assert!(log.should_warn(&Reject::Session(1)));
        assert!(!log.should_warn(&Reject::Session(2)));
        assert!(log.should_warn(&Reject::Query {
            step: "GetSecurityInfo",
            code: 5,
        }));
        assert!(!log.should_warn(&Reject::Query {
            step: "GetNamedPipeServerSessionId",
            code: 6,
        }));
    }

    #[test]
    fn displays_short_rejection_reasons() {
        assert_eq!(Reject::Session(1).to_string(), "server session 1 is not 0");
        assert_eq!(
            Reject::Owner("S-1-5-19".into()).to_string(),
            "server owner S-1-5-19 is not SYSTEM or Administrators"
        );
        assert_eq!(
            Reject::Query {
                step: "GetSecurityInfo",
                code: 5,
            }
            .to_string(),
            "GetSecurityInfo failed: 5"
        );
    }

    #[test]
    fn requests_only_granted_minimum_rights() {
        // Mirrors `UI_PIPE_SDDL`, `crates/host-win/src/winproc_model.rs:8`.
        const UI_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;0x12018b;;;S-1-2-1)";
        let mask = UI_PIPE_SDDL.split("0x").nth(1).unwrap();
        let mask = mask.split(';').next().unwrap();
        assert_eq!(u32::from_str_radix(mask, 16).unwrap(), SERVER_GRANTED);
        assert_eq!(CLIENT_ACCESS & !SERVER_GRANTED, 0);
        for forbidden in [
            0x4000_0000, // GENERIC_WRITE
            0x1000_0000, // GENERIC_ALL
            0x4,         // FILE_CREATE_PIPE_INSTANCE
            0x4_0000,    // WRITE_DAC
            0x8_0000,    // WRITE_OWNER
        ] {
            assert_eq!(CLIENT_ACCESS & forbidden, 0);
        }
        assert_ne!(CLIENT_ACCESS & 0x2_0000, 0); // READ_CONTROL
    }
}
