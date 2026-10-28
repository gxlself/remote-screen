//! Run as SYSTEM to exercise service supervision without an SCM registration.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use host_core::config::{ConfigStore, HostConfig};
    use host_core::platform::Platform;
    use host_win::paths::WinPaths;
    use host_win::proxy::PipePlatform;
    use host_win::supervisor::SupervisorConfig;
    use host_win::svc_local::{read_host_info, WinServiceLocal};
    use host_win::svc_session::start_supervisor;
    use host_win::wts;

    let invalid =
        || {
            io::Error::new(io::ErrorKind::InvalidInput,
        "usage: svc_session_probe --exe <absolute rs-host-session.exe path> [--seconds <n>]")
        };
    let mut args = std::env::args_os().skip(1);
    let mut exe = None;
    let mut seconds = 30_u64;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--exe") if exe.is_none() => {
                exe = Some(PathBuf::from(args.next().ok_or_else(invalid)?))
            }
            Some("--seconds") => {
                seconds = args
                    .next()
                    .and_then(|value| value.to_str()?.parse().ok())
                    .ok_or_else(invalid)?;
            }
            _ => return Err(invalid().into()),
        }
    }
    let exe = exe.filter(|exe| exe.is_absolute()).ok_or_else(invalid)?;
    println!("{:?}", read_host_info());
    let program_data = std::env::var("ProgramData").ok();
    let paths = WinPaths::from_program_data(program_data.as_deref());
    let p = Arc::new(PipePlatform::new(WinServiceLocal::new(&paths)));
    let config = ConfigStore::new(paths.config_file.clone(), HostConfig::default());
    let handle = start_supervisor(Arc::clone(&p), exe, SupervisorConfig::default(), config)?;
    let mut ticks = tokio::time::interval(Duration::from_secs(1));
    for t in 0..seconds {
        ticks.tick().await;
        println!(
            "t={t} console={:?} attached={} desktop={:?} displays={}",
            wts::active_console_session(),
            p.is_attached(),
            p.desktop(),
            Platform::displays(&*p).len()
        );
    }
    // The first interval tick is immediate; stop at the requested elapsed time.
    if seconds > 0 {
        ticks.tick().await;
    }
    let stats = handle
        .shutdown()
        .ok_or_else(|| io::Error::other("session supervisor thread panicked"))?;
    println!("{stats:?}");
    Ok(())
}
