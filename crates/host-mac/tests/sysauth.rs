#![cfg(target_os = "macos")]

use std::process::Command;

use host_core::platform::Platform;
use host_core::sysauth::{SysAuthError, SystemAuthenticator};
use host_mac::MacPlatform;

fn command_output(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("run account lookup command");
    assert!(output.status.success(), "{program} failed");
    String::from_utf8(output.stdout)
        .expect("command output is UTF-8")
        .trim()
        .to_owned()
}

fn require_od() -> bool {
    std::env::var_os("RS_REQUIRE_OD").is_some_and(|value| value == "1")
}

fn lookup_or_skip(
    auth: &dyn SystemAuthenticator,
    account: &str,
) -> Option<host_core::sysauth::SystemAccount> {
    match auth.lookup(account) {
        Ok(Some(account)) => Some(account),
        Ok(None) => panic!("expected account {account:?} to be found"),
        Err(SysAuthError::Unavailable(reason)) => {
            if require_od() {
                panic!("OpenDirectory unavailable with RS_REQUIRE_OD=1: {reason}");
            }
            eprintln!("skipping OpenDirectory test: {reason}");
            None
        }
        Err(SysAuthError::WrongPassword) => panic!("lookup cannot return WrongPassword"),
    }
}

#[test]
fn lookup_current_user_and_console_state() {
    let whoami = command_output("whoami", &[]);
    let console_user = command_output("stat", &["-f%Su", "/dev/console"]);
    let platform = MacPlatform::new();
    let Some(auth) = platform.system_authenticator() else {
        panic!("MacPlatform must provide a system authenticator");
    };
    let Some(account) = lookup_or_skip(auth.as_ref(), &whoami) else {
        return;
    };
    assert_eq!(account.short_name, whoami);
    assert_eq!(account.is_console_user, console_user == whoami);
}

#[test]
fn lookup_by_full_name() {
    let whoami = command_output("whoami", &[]);
    let full_name = command_output("id", &["-F", &whoami]);
    let platform = MacPlatform::new();
    let Some(auth) = platform.system_authenticator() else {
        panic!("MacPlatform must provide a system authenticator");
    };
    let Some(account) = lookup_or_skip(auth.as_ref(), &whoami) else {
        return;
    };
    let Some(by_full_name) = lookup_or_skip(auth.as_ref(), &full_name) else {
        return;
    };
    assert_eq!(by_full_name.short_name, account.short_name);
    assert_eq!(by_full_name.full_name, account.full_name);
}

#[test]
fn lookup_missing_and_rejects_embedded_nul() {
    let platform = MacPlatform::new();
    let Some(auth) = platform.system_authenticator() else {
        panic!("MacPlatform must provide a system authenticator");
    };
    assert_eq!(auth.lookup("a\0b"), Ok(None));
    match auth.lookup("rs-no-such-user-7f3a") {
        Ok(None) => {}
        Ok(Some(account)) => panic!("unexpected account: {account:?}"),
        Err(SysAuthError::Unavailable(reason)) if !require_od() => {
            eprintln!("skipping missing-account OpenDirectory test: {reason}");
        }
        Err(error) => panic!("missing-account lookup failed: {error}"),
    }
}

#[test]
fn verify_wrong_password_and_invalid_utf8() {
    let whoami = command_output("whoami", &[]);
    let platform = MacPlatform::new();
    let Some(auth) = platform.system_authenticator() else {
        panic!("MacPlatform must provide a system authenticator");
    };
    assert_eq!(
        auth.verify(&whoami, &[0xff, 0xfe]),
        Err(SysAuthError::WrongPassword)
    );
    let Some(account) = lookup_or_skip(auth.as_ref(), &whoami) else {
        return;
    };
    match auth.verify(&account.short_name, b"definitely-wrong-rs-test") {
        Err(SysAuthError::WrongPassword) => {}
        Err(SysAuthError::Unavailable(reason)) if !require_od() => {
            eprintln!("skipping password verification assertion: {reason}");
        }
        result => panic!("expected WrongPassword, got {result:?}"),
    }
}

#[test]
fn platform_exposes_system_authenticator() {
    assert!(MacPlatform::new().system_authenticator().is_some());
}
