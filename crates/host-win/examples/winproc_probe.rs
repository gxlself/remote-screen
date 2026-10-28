//! Windows manual probe. Run --pipe-echo and --spawn as SYSTEM. --desktop can
//! run unelevated, but observing Winlogon requires SYSTEM.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use host_win::desktop::input_desktop_name;
    use host_win::pipe::{connect_client, PipeServer};
    use host_win::spawn::spawn_in_session;
    use host_win::winproc_model::{verify_client, SESSION_PIPE_SDDL};
    use host_win::wts::active_console_session;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--desktop") if args.len() == 1 => {
            for _ in 0..60 {
                println!("input desktop: {:?}", input_desktop_name());
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        Some("--console") if args.len() == 1 => {
            println!("active console session: {:?}", active_console_session());
        }
        Some("--pipe-echo") if args.len() == 1 => {
            const NAME: &str = r"\\.\pipe\RemoteHost.probe";
            let mut server = PipeServer::new(NAME, SESSION_PIPE_SDDL)?;
            let serving = async {
                let (stream, pid) = server.accept().await?;
                println!("client pid: {pid}; own pid: {}; equal: {}", std::process::id(), verify_client(Some(std::process::id()), pid));
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await?;
                stream.get_mut().write_all(line.as_bytes()).await?;
                // Keep the server handle alive until the client consumes its echo.
                line.clear();
                stream.read_line(&mut line).await?;
                if line != "ack\n" {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "missing echo acknowledgement"));
                }
                Ok::<(), io::Error>(())
            };
            let connecting = async {
                let mut client = connect_client(NAME, Duration::from_secs(5)).await?;
                client.write_all(b"RemoteHost probe\n").await?;
                let mut client = BufReader::new(client);
                let mut echo = String::new();
                client.read_line(&mut echo).await?;
                if echo != "RemoteHost probe\n" {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "pipe echo mismatch"));
                }
                println!("echo: {}", echo.trim_end());
                client.get_mut().write_all(b"ack\n").await?;
                Ok(())
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(serving, connecting)
            }).await??;
        }
        Some("--spawn") if args.len() >= 3 => {
            let session = args[1].parse()?;
            let child_args: Vec<&str> = args[3..].iter().map(String::as_str).collect();
            let child = spawn_in_session(session, Path::new(&args[2]), &child_args)?;
            println!("spawned pid: {}", child.pid);
            println!("exit code: {}", child.wait().await);
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: winproc_probe --desktop | --pipe-echo | --spawn <session> <absolute-exe> [args...] | --console").into()),
    }
    Ok(())
}
