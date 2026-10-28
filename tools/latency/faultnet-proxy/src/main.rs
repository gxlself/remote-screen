//! Unprivileged loopback delay proxy used by `tools/latency/run.sh`.
//!
//! Exposes the faultnet proxy on a caller-selected loopback port so the nightly
//! job stays inside its allocated port range. The two rules add 2 ms +/- 1 ms
//! (and the wifi-good 100 Mbit/s cap) independently in each direction.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use host_fake::faultnet::{Dir, FaultProxy, UdpRule};

fn wifi_good_rule() -> UdpRule {
    UdpRule {
        delay_ms: 2,
        jitter_ms: 1,
        rate_kbps: Some(100_000),
        ..UdpRule::default()
    }
}

fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let upstream: SocketAddr = arg_value(&args, "--upstream")
        .context("usage: rs-latency-faultnet --upstream HOST:PORT --listen HOST:PORT")?
        .parse()
        .context("--upstream must be an IP:port")?;
    let listen: SocketAddr = arg_value(&args, "--listen")
        .context("usage: rs-latency-faultnet --upstream HOST:PORT --listen HOST:PORT")?
        .parse()
        .context("--listen must be an IP:port")?;
    let seed = arg_value(&args, "--seed")
        .unwrap_or("33400")
        .parse()
        .context("--seed must be a u64")?;

    let proxy = FaultProxy::start_on(listen, upstream, None, seed)
        .await
        .with_context(|| format!("start faultnet proxy on {listen}"))?;
    proxy.set_udp(Dir::ToHost, wifi_good_rule());
    proxy.set_udp(Dir::ToClient, wifi_good_rule());
    println!("{{\"addr\":\"{}\"}}", proxy.addr());

    tokio::signal::ctrl_c()
        .await
        .context("install Ctrl-C handler")?;
    proxy.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_good_matches_the_profile() {
        let rule = wifi_good_rule();
        assert_eq!(rule.delay_ms, 2);
        assert_eq!(rule.jitter_ms, 1);
        assert_eq!(rule.rate_kbps, Some(100_000));
        assert_eq!((rule.loss_pct, rule.blackhole), (0.0, false));
    }

    #[test]
    fn option_values_do_not_read_a_following_flag() {
        let args = ["bin", "--upstream", "--tcp"].map(str::to_owned);
        assert_eq!(arg_value(&args, "--tcp"), None);
        assert_eq!(arg_value(&args, "--upstream"), Some("--tcp"));
    }
}
