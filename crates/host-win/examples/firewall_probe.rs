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

    use host_core::listen::ListenPolicy;
    use host_win::firewall::{read_rules, SyncReport, WinFirewall};
    use host_win::nics;

    const USAGE: &str = "Windows firewall probe (run mutations as administrator):
  --status [--nic <name> ...] [--source <CIDR> ...] [--port 7400] [--exe <path>]
  --list
  --sync --nic <name> ... --source <CIDR> ... [--port 7400] [--exe <path>]
  --ensure-tailnet [--port 7400] [--exe <path>]
  --clear
  --it-note [--nic <name> ...] [--source <CIDR> ...] [--port 7400] [--exe <path>]
  --help
Use adapter FriendlyName values (for example, 以太网 or WLAN).
Omitting --exe uses this probe's executable path.
For intranet status, repeat the --nic, --source, --port and --exe used by --sync.
Omitting --nic with --sync disables intranet rules; --clear also removes them.";

    pub fn run() -> Result<(), Box<dyn Error>> {
        let mut args = std::env::args().skip(1).peekable();
        if args.peek().is_none() {
            println!("{USAGE}");
            return Ok(());
        }
        let mut action = None;
        let mut exe = None;
        let mut policy = ListenPolicy {
            tailnet: true,
            intranet_nics: Vec::new(),
            allowed_cidrs: Vec::new(),
            lan_cidrs: Vec::new(),
            port: 7400,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--status" | "--list" | "--sync" | "--ensure-tailnet" | "--clear" | "--it-note" => {
                    if action.replace(arg).is_some() {
                        return Err("choose exactly one action".into());
                    }
                }
                "--nic" => policy
                    .intranet_nics
                    .push(args.next().ok_or("--nic needs a name")?),
                "--source" => policy
                    .allowed_cidrs
                    .push(args.next().ok_or("--source needs a CIDR")?.parse()?),
                "--port" => {
                    policy.port = args.next().ok_or("--port needs a port number")?.parse()?;
                    if policy.port == 0 {
                        return Err("--port must be between 1 and 65535".into());
                    }
                }
                "--exe" => exe = Some(args.next().ok_or("--exe needs a path")?),
                "--help" | "-h" => {
                    println!("{USAGE}");
                    return Ok(());
                }
                _ => return Err(format!("unknown argument: {arg}\n{USAGE}").into()),
            }
        }
        let action = action.ok_or("missing action (use --help for usage)")?;
        let firewall = match exe {
            Some(exe) => WinFirewall::new(Some(exe), policy.port),
            None => WinFirewall::for_current_exe(policy.port),
        };
        match action.as_str() {
            "--status" => {
                let facts = firewall.facts();
                println!(
                    "profiles={:?}, policy_locked={}, rules={}, error={:?}",
                    facts.profiles,
                    facts.policy_locked,
                    facts.existing.len(),
                    facts.error,
                );
                println!(
                    "{}",
                    serde_json::to_string_pretty(&firewall.status(&policy, &nics::nics()))?
                );
            }
            "--list" => {
                for rule in read_rules()? {
                    println!(
                        "name={:?}, protocol={}, ports={:?}, local={:?}, remote={:?}, profiles={}, program={:?}, enabled={}, group={:?}",
                        rule.name, rule.protocol, rule.local_ports, rule.local_addresses,
                        rule.remote_addresses, rule.profiles, rule.application, rule.enabled, rule.group,
                    );
                }
            }
            "--sync" => print_report(firewall.sync_intranet(&policy, &nics::nics()))?,
            "--ensure-tailnet" => print_report(firewall.ensure_tailnet())?,
            "--clear" => print_report(firewall.remove_intranet())?,
            "--it-note" => println!("{}", firewall.it_note(&policy, &nics::nics())),
            _ => return Err("unsupported action".into()),
        }
        Ok(())
    }

    fn print_report(report: SyncReport) -> Result<(), Box<dyn Error>> {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "removed": report.removed,
                "added": report.added,
                "error": report.error,
                "policy_locked": report.policy_locked,
            }))?
        );
        if report.error.is_some() {
            return Err("firewall synchronization failed; see the report above".into());
        }
        Ok(())
    }
}
