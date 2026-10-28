#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::run()
}

#[cfg(windows)]
mod windows_probe {
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;

    use host_core::platform::Nic;
    use host_core::whois::{
        ChainedWhoIs, CliSource, LocalApiSource, WhoIs, WhoIsError, WhoIsSource,
    };
    use host_win::addr_watch::watch_addresses;
    use host_win::addr_watch_model::{follow_nics, DEBOUNCE, MAX_DELAY, RESYNC};
    use host_win::nics;
    use tokio::sync::mpsc;

    struct Options {
        seconds: u64,
        whois: Option<SocketAddr>,
        self_login: bool,
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let mut options = Options {
            seconds: 120,
            whois: None,
            self_login: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--seconds" => {
                    options.seconds = args.next().ok_or("--seconds needs a number")?.parse()?;
                }
                "--whois" => {
                    options.whois = Some(args.next().ok_or("--whois needs <ip:port>")?.parse()?);
                }
                "--self-login" => options.self_login = true,
                _ => return Err(format!("unknown argument: {arg}").into()),
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(probe(options))
    }

    async fn probe(options: Options) -> Result<(), Box<dyn Error>> {
        if options.whois.is_some() || options.self_login {
            let local = LocalApiSource::default();
            let cli = CliSource::default();
            println!("LocalAPI endpoint: {}", local.socket_path().display());
            println!("CLI executable: {}", cli.executable().display());
            if options.self_login {
                print_login("LocalAPI", local.self_login().await);
                print_login("CLI", cli.self_login().await);
            }
            if let Some(peer) = options.whois {
                print_whois("LocalAPI", local.whois(peer).await);
                print_whois("CLI", cli.whois(peer).await);
                let chained = ChainedWhoIs::new(LocalApiSource::default(), CliSource::default());
                print_whois("Chained", chained.whois(peer).await);
            }
            return Ok(());
        }

        print_nics(nics::nics());
        let (tx, rx) = mpsc::channel(8);
        let _watch = watch_addresses(tx)?;
        let _ = tokio::time::timeout(
            Duration::from_secs(options.seconds),
            follow_nics(rx, nics::nics, print_nics, DEBOUNCE, MAX_DELAY, RESYNC),
        )
        .await;
        Ok(())
    }

    fn print_nics(nics: Vec<Nic>) {
        println!("NICs:");
        for nic in nics {
            println!(
                "  {}: addresses={:?}, tailnet={}",
                nic.name, nic.addresses, nic.tailnet
            );
        }
    }

    fn print_login(source: &str, result: Result<String, WhoIsError>) {
        match result {
            Ok(login) => println!("{source}: login={login}"),
            Err(error) => println!("{source}: error={error}"),
        }
    }

    fn print_whois(source: &str, result: Result<WhoIs, WhoIsError>) {
        match result {
            Ok(whois) => println!("{source}: login={}, node={}", whois.login, whois.node_name),
            Err(error) => println!("{source}: error={error}"),
        }
    }
}
