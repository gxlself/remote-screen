//! Manual Windows service probe. Tray and user-token modes require SYSTEM.
//! --deploy prints registry metadata only; it never opens the pair file.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Write};
    use std::path::Path;

    use host_win::deploy::read_deploy;
    use host_win::scm::{run_service, ScmError};
    use host_win::svc_os_model::{deploy_fingerprint, parse_deploy, ScmEvent};
    use host_win::tray::{ensure_tray, running_in_session, session_has_user, spawn_as_user};
    use host_win::wts::active_console_session;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
    let console = || {
        active_console_session()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no console session"))
    };
    match args.first().map(String::as_str) {
        Some("--scm") => {
            let mut out = None;
            let mut exit_code = 0_u32;
            let mut options = args[1..].iter();
            while let Some(option) = options.next() {
                match option.as_str() {
                    "--out" => out = Some(options.next().ok_or_else(|| invalid("--out requires a path"))?.clone()),
                    "--exit-code" => exit_code = options.next().ok_or_else(|| invalid("--exit-code requires a number"))?.parse()?,
                    _ => return Err(invalid("unknown SCM option").into()),
                }
            }
            let out = out.ok_or_else(|| invalid("--scm requires --out <file>"))?;
            let result = run_service("RsSvcProbe", Box::new(move |status, mut events| {
                status.running();
                let mut write_events = || -> io::Result<()> {
                    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&out)?;
                    while let Some(event) = events.blocking_recv() {
                        writeln!(file, "{event:?}")?;
                        if matches!(event, ScmEvent::Stop | ScmEvent::Shutdown) {
                            writeln!(file, "stop")?;
                            file.flush()?;
                            return Ok(());
                        }
                        file.flush()?;
                    }
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "SCM event channel closed"))
                };
                match write_events() {
                    Ok(()) => exit_code,
                    Err(error) => { eprintln!("SCM probe output failed: {error}"); 1 }
                }
            }));
            match result {
                Err(ScmError::NotUnderScm) => {
                    eprintln!("not started by the service control manager");
                    std::process::exit(2);
                }
                result => result?,
            }
        }
        Some("--tray") if args.len() == 2 => println!("{:?}", ensure_tray(Path::new(&args[1]))?),
        Some("--as-user") if args.len() >= 2 => {
            let child_args: Vec<_> = args[2..].iter().map(String::as_str).collect();
            println!("pid: {}", spawn_as_user(console()?, Path::new(&args[1]), &child_args)?);
        }
        Some("--running") if args.len() == 2 => println!("{}", running_in_session(console()?, &args[1])?),
        Some("--deploy") if args.len() == 1 => {
            let values = read_deploy()?;
            println!("{values:?}");
            if let Some(values) = values {
                println!("parsed: {:?}", parse_deploy(&values));
                println!("fingerprint: {}", deploy_fingerprint(&values));
            }
        }
        Some("--console") if args.len() == 1 => {
            let session = active_console_session();
            println!("console: {session:?}, has_user={}", session.is_some_and(session_has_user));
        }
        _ => return Err(invalid("usage: svc_os_probe --scm --out <file> [--exit-code <n>] | --tray <exe> | --as-user <exe> [args...] | --running <image> | --deploy | --console").into()),
    }
    Ok(())
}
