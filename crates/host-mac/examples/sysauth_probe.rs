//! Manual probe for macOS OpenDirectory account lookup and password checking.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("sysauth_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::{self, BufRead};
    use std::time::Instant;

    use host_core::platform::Platform;
    use host_core::sysauth::SysAuthError;
    use host_mac::MacPlatform;
    use serde_json::json;

    struct Args {
        account: String,
        verify: bool,
    }

    fn parse_args() -> Args {
        let mut account = None;
        let mut verify = false;
        let mut values = std::env::args().skip(1);
        while let Some(value) = values.next() {
            match value.as_str() {
                "--account" => account = values.next(),
                "--verify" => verify = true,
                "--help" | "-h" => {
                    println!("usage: sysauth_probe --account <name> [--verify]");
                    std::process::exit(0);
                }
                unknown => {
                    eprintln!("unknown argument: {unknown}");
                    std::process::exit(2);
                }
            }
        }
        let Some(account) = account else {
            eprintln!("--account <name> is required");
            std::process::exit(2);
        };
        Args { account, verify }
    }

    fn print_lookup(account: &host_core::sysauth::SystemAccount) {
        println!(
            "{}",
            json!({
                "found": true,
                "short_name": account.short_name,
                "full_name": account.full_name,
                "is_admin": account.is_admin,
                "is_console_user": account.is_console_user,
            })
        );
    }

    fn print_missing() {
        println!(
            "{}",
            json!({
                "found": false,
                "short_name": null,
                "full_name": null,
                "is_admin": false,
                "is_console_user": false,
            })
        );
    }

    fn read_password() -> io::Result<Vec<u8>> {
        let mut input = Vec::new();
        io::stdin().lock().read_until(b'\n', &mut input)?;
        while matches!(input.last(), Some(b'\n' | b'\r')) {
            input.pop();
        }
        Ok(input)
    }

    fn verify_status(result: Result<(), SysAuthError>) -> &'static str {
        match result {
            Ok(()) => "ok",
            Err(SysAuthError::WrongPassword) => "wrong_password",
            Err(SysAuthError::Unavailable(_)) => "unavailable",
        }
    }

    pub fn main() {
        let args = parse_args();
        let platform = MacPlatform::new();
        let Some(authenticator) = platform.system_authenticator() else {
            eprintln!("system authenticator unavailable");
            std::process::exit(1);
        };
        let account = match authenticator.lookup(&args.account) {
            Ok(Some(account)) => {
                print_lookup(&account);
                Some(account)
            }
            Ok(None) => {
                print_missing();
                None
            }
            Err(error) => {
                println!(
                    "{}",
                    json!({
                        "found": false,
                        "short_name": null,
                        "full_name": null,
                        "is_admin": false,
                        "is_console_user": false,
                        "error": error.to_string(),
                    })
                );
                None
            }
        };

        if args.verify {
            let started = Instant::now();
            let mut password = read_password().unwrap_or_default();
            let result = account.as_ref().map_or(
                Err(SysAuthError::Unavailable("account not found".to_owned())),
                |account| authenticator.verify(&account.short_name, &password),
            );
            password.fill(0);
            println!(
                "{}",
                json!({
                    "verify": verify_status(result),
                    "ms": started.elapsed().as_secs_f64() * 1000.0,
                })
            );
        }
    }
}
