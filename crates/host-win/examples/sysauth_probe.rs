//! Manual Windows account lookup and stdin-only password verification probe.

#[cfg(not(windows))]
fn main() {
    eprintln!("sysauth_probe only runs on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    windows_probe::run();
}

#[cfg(windows)]
mod windows_probe {
    use std::{io, time::Instant};

    use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
    use host_win::sysauth::WinAuthenticator;
    use serde_json::json;
    use windows::Win32::{
        Foundation::{GetLastError, HANDLE},
        Storage::FileSystem::{GetFileType, ReadFile, FILE_TYPE_CHAR},
        System::Console::{
            GetConsoleMode, GetStdHandle, ReadConsoleW, SetConsoleMode, CONSOLE_MODE,
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, STD_INPUT_HANDLE,
        },
    };
    use zeroize::{Zeroize, Zeroizing};

    struct Args {
        account: String,
        verify: bool,
    }

    fn parse_args() -> Result<Args, &'static str> {
        let mut account = None;
        let mut verify = false;
        let mut values = std::env::args().skip(1);
        while let Some(value) = values.next() {
            match value.as_str() {
                "--account" => account = Some(values.next().ok_or("--account requires a name")?),
                "--verify" => verify = true,
                "--help" | "-h" => {
                    println!("usage: sysauth_probe --account <name> [--verify]");
                    std::process::exit(0);
                }
                _ => return Err("unknown argument; use --help for usage"),
            }
        }
        let account = account.ok_or("--account <name> is required")?;
        Ok(Args { account, verify })
    }

    struct EchoGuard {
        handle: HANDLE,
        original: CONSOLE_MODE,
    }

    impl EchoGuard {
        fn disable(handle: HANDLE) -> io::Result<Self> {
            let mut original = CONSOLE_MODE::default();
            // SAFETY: handle is a borrowed stdin console handle; original is
            // a writable mode out parameter. No handle ownership is transferred.
            unsafe { GetConsoleMode(handle, &mut original) }
                .map_err(|_| io::Error::other("cannot query stdin console mode"))?;
            let guard = Self { handle, original };
            // SAFETY: this is the same live console handle. Only echo and line
            // input flags are changed, and the guard restores the original mode.
            unsafe { SetConsoleMode(handle, (original | ENABLE_LINE_INPUT) & !ENABLE_ECHO_INPUT) }
                .map_err(|_| io::Error::other("cannot disable stdin echo"))?;
            Ok(guard)
        }
    }

    impl Drop for EchoGuard {
        fn drop(&mut self) {
            // SAFETY: stdin is borrowed for the probe's lifetime; restore the
            // exact console mode captured before reading any password data.
            unsafe {
                let _ = SetConsoleMode(self.handle, self.original);
            }
        }
    }

    fn push_secret<T: Zeroize + Copy>(buffer: &mut Zeroizing<Vec<T>>, value: T) {
        if buffer.len() == buffer.capacity() {
            let capacity = buffer.capacity().saturating_mul(2).max(64);
            let mut larger = Zeroizing::new(Vec::with_capacity(capacity));
            larger.extend_from_slice(buffer);
            // Replacing the wrapper wipes the old allocation before freeing it.
            *buffer = larger;
        }
        buffer.push(value);
    }

    fn read_console(handle: HANDLE) -> io::Result<Zeroizing<Vec<u8>>> {
        let _echo = EchoGuard::disable(handle)?;
        let mut line = Zeroizing::new(Vec::new());
        let mut unit = Zeroizing::new([0_u16; 1]);
        loop {
            let mut read = 0;
            // SAFETY: the one-unit Zeroizing buffer and count are writable.
            // Line mode handles console editing while echo remains disabled.
            unsafe { ReadConsoleW(handle, unit.as_mut_ptr().cast(), 1, &mut read, None) }
                .map_err(|_| io::Error::other("cannot read stdin console"))?;
            if read == 0 || unit[0] == u16::from(b'\n') {
                break;
            }
            push_secret(&mut line, unit[0]);
            unit.zeroize();
        }
        if line.last() == Some(&u16::from(b'\r')) {
            line.pop();
        }
        // A BMP code unit needs at most three UTF-8 bytes; a surrogate pair
        // uses four bytes for two units, so this capacity avoids reallocation.
        let mut bytes = Zeroizing::new(Vec::with_capacity(line.len() * 3));
        let mut encoded = Zeroizing::new([0_u8; 4]);
        for character in char::decode_utf16(line.iter().copied()) {
            let character = character
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid stdin UTF-16"))?;
            bytes.extend_from_slice(character.encode_utf8(&mut *encoded).as_bytes());
            encoded.zeroize();
        }
        // line and unit are wiped as this function returns; bytes is owned by
        // the caller only until verify completes.
        Ok(bytes)
    }

    fn read_pipe(handle: HANDLE) -> io::Result<Zeroizing<Vec<u8>>> {
        let mut line = Zeroizing::new(Vec::new());
        let mut byte = Zeroizing::new([0_u8; 1]);
        loop {
            let mut read = 0;
            // SAFETY: a one-byte read stops at the first newline without a
            // hidden stdio buffer retaining password bytes or the next line.
            let result = unsafe { ReadFile(handle, Some(&mut *byte), Some(&mut read), None) };
            if result.is_err() {
                // SAFETY: read the error immediately after the failed ReadFile.
                if unsafe { GetLastError().0 } == 109 {
                    break; // ERROR_BROKEN_PIPE is EOF for redirected stdin.
                }
                return Err(io::Error::other("cannot read stdin"));
            }
            if read == 0 || byte[0] == b'\n' {
                break;
            }
            push_secret(&mut line, byte[0]);
            byte.zeroize();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(line)
    }

    fn read_password() -> io::Result<Zeroizing<Vec<u8>>> {
        // SAFETY: obtaining the process's borrowed stdin handle has no pointer
        // preconditions. The process retains ownership, so do not CloseHandle it.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }
            .map_err(|_| io::Error::other("stdin is unavailable"))?;
        if handle.is_invalid() {
            return Err(io::Error::other("stdin is unavailable"));
        }
        // SAFETY: handle is a live borrowed stdin handle. Character devices are
        // required to support echo control before any secret is read.
        if unsafe { GetFileType(handle) } == FILE_TYPE_CHAR {
            read_console(handle)
        } else {
            read_pipe(handle)
        }
    }

    fn verify_status(result: Result<(), SysAuthError>) -> &'static str {
        match result {
            Ok(()) => "ok",
            Err(SysAuthError::WrongPassword) => "wrong_password",
            Err(SysAuthError::Unavailable(_)) => "unavailable",
        }
    }

    pub fn run() {
        let args = match parse_args() {
            Ok(args) => args,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        };
        let authenticator = WinAuthenticator;
        let account = match authenticator.lookup(&args.account) {
            Ok(Some(account)) => {
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
                Some(account)
            }
            result => {
                println!(
                    "{}",
                    json!({
                        "found": false,
                        "short_name": null,
                        "full_name": null,
                        "is_admin": false,
                        "is_console_user": false,
                        "error": result.err().map(|error| error.to_string()),
                    })
                );
                None
            }
        };

        if args.verify {
            let input = read_password();
            let started = Instant::now();
            let result = match input {
                Ok(password) => {
                    let result = account.as_ref().map_or_else(
                        || Err(SysAuthError::Unavailable("account not found".to_owned())),
                        |account: &SystemAccount| {
                            authenticator.verify(&account.short_name, &password)
                        },
                    );
                    drop(password); // Zeroize before printing or returning.
                    result
                }
                Err(_) => Err(SysAuthError::Unavailable("stdin is unavailable".to_owned())),
            };
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
