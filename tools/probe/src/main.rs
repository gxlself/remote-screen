//! rs-probe: headless RSP client for bring-up and latency numbers.
//!
//! Connects to rs-host, samples for `--seconds` after the first frame, then prints frame
//! rate, bitrate, losses and capture→receive latency percentiles. `--dump`
//! writes the raw HEVC Annex-B stream so it can be checked with ffprobe.
//! `--json` additionally writes one machine-readable metrics line for CI.

use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rsp_client::{
    ClientEvent, ClientHandle, ClientOptions, Session, SessionEvent, SessionHandle, SessionParams,
    SessionStats, StartParams,
};
use rsp_core::control::Codec;
use rsp_transport::identity::{CertKind, KeySource, LocalIdentity};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::time::Instant;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Parser, Debug)]
#[command(about = "RSP/1 probe client")]
struct Args {
    /// Host address, e.g. 100.64.0.6:7400
    #[arg(long, default_value = "100.64.0.6:7400")]
    addr: SocketAddr,
    /// Sample for this many seconds after the first complete frame. Wait up to
    /// 15 seconds after opening the session for that frame. Ignored by --probe.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    #[arg(long, default_value_t = 1920)]
    max_width: u32,
    #[arg(long, default_value_t = 60)]
    fps: u16,
    #[arg(long, default_value_t = 12_000)]
    bitrate_kbps: u32,
    /// Write the received HEVC Annex-B stream here.
    #[arg(long)]
    dump: Option<PathBuf>,
    /// Write the final metrics as one JSON line to this path, or '-' for stdout.
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,
    /// Host SHA-256 pin (hex): SPKI with --identity, certificate DER otherwise.
    #[arg(long)]
    pin: Option<String>,
    /// Move the remote cursor to "x,y" (0..1) after 2 s, then back. No clicks.
    #[arg(long)]
    move_to: Option<String>,
    /// Directory containing a software device identity. It is generated on
    /// first use and printed for the strict pre-P2 pairing workflow.
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Connect to a development host that accepts clients without pairing.
    #[arg(long)]
    dev_allow_unpaired: bool,
    /// Read the minimal ServerHello and exit without starting a stream.
    #[arg(long, requires = "identity", conflicts_with = "pair_code")]
    probe: bool,
    /// Complete SPAKE2 pairing, then connect with the same identity.
    #[arg(long, requires = "identity")]
    pair_code: Option<String>,
}

struct SamplingWindow {
    duration: Duration,
    deadline: Instant,
    first_frame: Option<Instant>,
}

impl SamplingWindow {
    fn new(now: Instant, duration: Duration) -> Self {
        Self {
            duration,
            deadline: now + FIRST_FRAME_TIMEOUT,
            first_frame: None,
        }
    }

    fn on_frame(&mut self, now: Instant) {
        if self.first_frame.is_none() {
            self.first_frame = Some(now);
            self.deadline = now + self.duration;
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.first_frame
            .map(|start| now.saturating_duration_since(start))
            .unwrap_or_default()
    }
}

fn load_or_create_identity(dir: &PathBuf) -> Result<LocalIdentity> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create identity directory {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let cert_path = dir.join("cert.der");
    let key_path = dir.join("key.der");
    if cert_path.exists() && key_path.exists() {
        return Ok(LocalIdentity {
            cert_der: CertificateDer::from(std::fs::read(cert_path)?),
            key: KeySource::Software(PrivatePkcs8KeyDer::from(std::fs::read(key_path)?)),
        });
    }
    if cert_path.exists() || key_path.exists() {
        bail!("identity directory is incomplete; refusing to replace its key");
    }
    let identity = LocalIdentity::generate_software(CertKind::App)
        .context("generate software device identity")?;
    if let KeySource::Software(key) = &identity.key {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(key_path)?;
        file.write_all(key.secret_pkcs8_der())?;
        file.sync_all()?;
    }
    std::fs::write(cert_path, identity.cert_der.as_ref())?;
    Ok(identity)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LatencySummary {
    p50_us: i64,
    p95_us: i64,
    p99_us: i64,
    max_us: i64,
}

fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    let last = sorted.len().checked_sub(1)?;
    debug_assert!((0.0..=1.0).contains(&p));
    let idx = (last as f64 * p).round() as usize;
    sorted.get(idx).copied()
}

fn latency_summary(sorted: &[i64]) -> Option<LatencySummary> {
    Some(LatencySummary {
        p50_us: percentile(sorted, 0.50)?,
        p95_us: percentile(sorted, 0.95)?,
        p99_us: percentile(sorted, 0.99)?,
        max_us: *sorted.last()?,
    })
}

fn json_string(value: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

struct JsonReport<'a> {
    seconds: u64,
    frames: u64,
    latency: LatencySummary,
    fps: f64,
    bitrate_kbps: f64,
    codec: &'a str,
    addr: SocketAddr,
    git_rev: Option<&'a str>,
}

fn render_json(report: &JsonReport<'_>) -> String {
    format!(
        concat!(
            "{{\"schema\":1,\"seconds\":{},\"frames\":{},\"capture_to_receive_ms\":{{",
            "\"p50\":{:.3},\"p95\":{:.3},\"p99\":{:.3},\"max\":{:.3}}},",
            "\"fps\":{:.3},\"bitrate_kbps\":{:.3},\"codec\":{},\"addr\":{},\"git_rev\":{}}}"
        ),
        report.seconds,
        report.frames,
        report.latency.p50_us as f64 / 1e3,
        report.latency.p95_us as f64 / 1e3,
        report.latency.p99_us as f64 / 1e3,
        report.latency.max_us as f64 / 1e3,
        report.fps,
        report.bitrate_kbps,
        json_string(report.codec),
        json_string(&report.addr.to_string()),
        report
            .git_rev
            .map_or_else(|| "null".to_owned(), json_string),
    )
}

fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Hevc => "hevc",
        Codec::H264 => "h264",
        Codec::Unspecified => "unspecified",
    }
}

fn write_json(path: &PathBuf, report: &JsonReport<'_>) -> Result<()> {
    let line = render_json(report);
    if path.as_os_str() == "-" {
        println!("{line}");
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create JSON directory {}", parent.display()))?;
    }
    std::fs::write(path, line + "\n")
        .with_context(|| format!("write JSON report {}", path.display()))
}

fn git_revision() -> Result<Option<String>> {
    std::env::var_os("RS_GIT_REV")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("RS_GIT_REV is not valid UTF-8"))
        })
        .transpose()
}

async fn pair(
    addr: SocketAddr,
    identity: Arc<LocalIdentity>,
    host_fp: [u8; 32],
    code: &str,
) -> Result<()> {
    let mut params = SessionParams::single_shared(addr, host_fp, identity);
    params.intent = rsp_core::pb::Intent::Pair;
    params.device_name = "rs-probe".into();
    params.platform = rsp_core::pb::Platform::Linux;
    params.pairing_code = Some(code.to_owned());
    let (session, mut events) = Session::open(params).await?;
    let result = tokio::time::timeout(Duration::from_secs(40), async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::PairResult(result) => {
                    match rsp_core::pb::pair_result::Status::try_from(result.status) {
                        Ok(rsp_core::pb::pair_result::Status::WaitingLocal) => {
                            println!("waiting for local pairing approval");
                        }
                        Ok(rsp_core::pb::pair_result::Status::Ok) => {
                            println!(
                                "paired with {}; Host SPKI {}",
                                result.host_id,
                                rsp_client::fingerprint_hex(&host_fp)
                            );
                            return Ok(());
                        }
                        status => bail!("pairing failed: {status:?}"),
                    }
                }
                SessionEvent::Ended { reason, .. } => {
                    bail!("pairing connection closed: {reason}")
                }
                _ => {}
            }
        }
        bail!("pairing connection closed")
    })
    .await
    .context("pairing timed out")?;
    session.close();
    result
}

enum ProbeClient {
    Legacy(ClientHandle),
    Session(SessionHandle),
}

impl ProbeClient {
    fn send_cursor(&self, x: f32, y: f32) {
        match self {
            Self::Legacy(client) => {
                client.send_input(rsp_core::input::InputEvent::MouseMove { x, y })
            }
            Self::Session(session) => {
                let _ = session.send_input(rsp_core::hid::InputMsg::MouseMove { x, y });
            }
        }
    }

    fn close(&self) {
        match self {
            Self::Legacy(client) => client.close(),
            Self::Session(session) => session.close(),
        }
    }

    fn print_stats(&self) {
        match self {
            Self::Legacy(client) => {
                let stats = client.stats();
                println!(
                    "datagrams        {} received, path MTU-ish {} B, lost packets {}",
                    stats.udp_rx.datagrams, stats.path.current_mtu, stats.path.lost_packets
                );
            }
            Self::Session(session) => {
                let stats: SessionStats = session.stats();
                println!(
                    "session frames   {} received, {} lost, {} recovered",
                    stats.frames_received, stats.frames_lost, stats.frames_recovered
                );
            }
        }
    }
}

async fn discover_host_fingerprint(addr: SocketAddr, identity: &LocalIdentity) -> Result<[u8; 32]> {
    let tls = rsp_transport::tls::client_config(identity, rsp_transport::tls::ServerPin::Any)?;
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let config = quinn::ClientConfig::new(Arc::new(crypto));
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse()?
    } else if addr.ip().is_loopback() {
        "127.0.0.1:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);
    let conn = endpoint.connect(addr, rsp_core::SERVER_NAME)?.await?;
    let certs = conn
        .peer_identity()
        .and_then(|peer| peer.downcast::<Vec<CertificateDer<'static>>>().ok())
        .context("missing Host TLS identity")?;
    let fingerprint =
        rsp_transport::tls::peer_fingerprint(&certs).context("invalid Host certificate")?;
    conn.close(0u32.into(), b"fingerprint discovery");
    endpoint.wait_idle().await;
    Ok(fingerprint)
}

async fn open_identity_session(
    opts: &ClientOptions,
    identity: Arc<LocalIdentity>,
    host_fp: [u8; 32],
) -> Result<(
    ProbeClient,
    tokio::sync::mpsc::UnboundedReceiver<ClientEvent>,
)> {
    let mut params = SessionParams::single_shared(opts.addr, host_fp, identity);
    params.device_name = opts.client_name.clone();
    params.start = opts.start.clone();
    params.intent = rsp_core::pb::Intent::Control;
    params.legacy_quic_input = true;
    // This bring-up media/input path requires QUIC, just like the legacy probe.
    // A faster TCP handshake under load cannot provide the requested stream.
    params.dial_options.tcp_after = params.dial_options.timeout;
    let (session, mut events) = Session::open(params).await?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let mapped = match event {
                SessionEvent::ServerHello(hello) => {
                    let display = hello
                        .displays
                        .iter()
                        .find(|display| display.primary)
                        .or_else(|| hello.displays.first());
                    let (width, height) = display
                        .map(|display| (display.width_px, display.height_px))
                        .unwrap_or_default();
                    Some(ClientEvent::Connected {
                        host_name: hello.host_name,
                        display_width: width,
                        display_height: height,
                    })
                }
                SessionEvent::StreamConfig(config) => Some(ClientEvent::StreamConfig {
                    width: config.width,
                    height: config.height,
                    fps: config.fps.min(u32::from(u16::MAX)) as u16,
                    cursor_x: config.cursor_x,
                    cursor_y: config.cursor_y,
                }),
                SessionEvent::Frame(frame) => Some(ClientEvent::Frame(frame)),
                SessionEvent::ClockSync { rtt_us, offset_us } => {
                    Some(ClientEvent::ClockSync { rtt_us, offset_us })
                }
                SessionEvent::Error { code, message } => Some(ClientEvent::HostError {
                    code: code.code(),
                    message,
                }),
                SessionEvent::Ended { reason, code, .. } => {
                    if let Some(code) = code {
                        let _ = tx.send(ClientEvent::HostError {
                            code: code.code(),
                            message: reason.clone(),
                        });
                    }
                    let _ = tx.send(ClientEvent::Closed { reason });
                    break;
                }
                _ => None,
            };
            if let Some(event) = mapped {
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    });
    Ok((ProbeClient::Session(session), rx))
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let identity = args
        .identity
        .as_ref()
        .map(load_or_create_identity)
        .transpose()?
        .map(Arc::new);
    if let (Some(dir), Some(identity)) = (&args.identity, &identity) {
        println!(
            "device identity {} (certificate saved in {})",
            rsp_client::fingerprint_hex(&identity.fingerprint()),
            dir.display()
        );
        if args.dev_allow_unpaired {
            println!("development mode requested; host must be started with --dev-allow-unpaired");
        }
    }

    let pin = match &args.pin {
        Some(hex) => {
            Some(rsp_client::parse_fingerprint(hex).context("--pin must be 64 hex chars")?)
        }
        None => None,
    };
    let move_to = match &args.move_to {
        Some(s) => {
            let (x, y) = s.split_once(',').context("--move-to expects x,y")?;
            Some((x.trim().parse::<f32>()?, y.trim().parse::<f32>()?))
        }
        None => None,
    };

    let opts = ClientOptions {
        addr: args.addr,
        client_name: "rs-probe".into(),
        start: StartParams {
            codec: Codec::Hevc,
            max_width: args.max_width,
            fps: args.fps,
            bitrate_kbps: args.bitrate_kbps,
        },
        pin_sha256: pin,
    };
    let t0 = Instant::now();
    let identity_host_fp = if let Some(identity) = &identity {
        let host_fp = match pin {
            Some(pin) => pin,
            None => discover_host_fingerprint(args.addr, identity).await?,
        };
        if args.probe {
            let params = SessionParams::single_shared(args.addr, host_fp, Arc::clone(identity));
            let result = Session::probe(params).await?;
            println!(
                "probe complete: {:?}; peer {:?}",
                result.server_hello, result.peer
            );
            return Ok(());
        }
        if let Some(code) = &args.pair_code {
            pair(args.addr, Arc::clone(identity), host_fp, code).await?;
        }
        Some(host_fp)
    } else {
        None
    };
    let (client, mut events) = match (identity, identity_host_fp) {
        (Some(identity), Some(host_fp)) => open_identity_session(&opts, identity, host_fp).await?,
        _ => {
            let (client, events) = rsp_client::connect(opts.clone()).await?;
            (ProbeClient::Legacy(client), events)
        }
    };
    println!(
        "connected in {:.1} ms, host cert sha256 {}",
        t0.elapsed().as_secs_f64() * 1e3,
        identity_host_fp
            .or_else(|| match &client {
                ProbeClient::Legacy(client) => client.server_fingerprint(),
                ProbeClient::Session(_) => None,
            })
            .map(|f| rsp_client::fingerprint_hex(&f))
            .unwrap_or_default()
    );

    let mut dump = match &args.dump {
        Some(p) => Some(File::create(p).with_context(|| format!("create {}", p.display()))?),
        None => None,
    };

    // Session::open returns before authorization and stream setup finish.
    // Keep startup bounded without charging it to the sampling window.
    let mut sampling = SamplingWindow::new(Instant::now(), Duration::from_secs(args.seconds));
    let mut first_frame: Option<Duration> = None;
    let mut frames = 0u64;
    let mut keyframes = 0u64;
    let mut bytes = 0u64;
    let mut max_frame = 0usize;
    let mut last_id: Option<u32> = None;
    let mut gaps = 0u64;
    let mut latency_us: Vec<i64> = Vec::new();
    let mut best_rtt_us = u64::MAX;
    let mut cursor_start = (0.5f32, 0.5f32);
    let mut stream_started: Option<Instant> = None;
    let mut moved = false;
    let mut restored = false;

    loop {
        let ev = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(sampling.deadline) => break,
            ev = events.recv() => ev,
        };
        let Some(ev) = ev else { break };
        match ev {
            ClientEvent::Connected {
                host_name,
                display_width,
                display_height,
            } => {
                println!("host '{host_name}', display {display_width}x{display_height}");
            }
            ClientEvent::StreamConfig {
                width,
                height,
                fps,
                cursor_x,
                cursor_y,
            } => {
                println!("stream {width}x{height}@{fps}, cursor at ({cursor_x:.3}, {cursor_y:.3})");
                cursor_start = (cursor_x, cursor_y);
                stream_started = Some(Instant::now());
            }
            ClientEvent::ClockSync { rtt_us, .. } => best_rtt_us = best_rtt_us.min(rtt_us),
            ClientEvent::HostError { code, message } => bail!("host error {code}: {message}"),
            ClientEvent::Closed { reason } => {
                println!("closed: {reason}");
                break;
            }
            ClientEvent::Frame(rf) => {
                let f = &rf.frame;
                sampling.on_frame(Instant::now());
                first_frame.get_or_insert_with(|| t0.elapsed());
                if let Some(prev) = last_id {
                    if f.frame_id != prev.wrapping_add(1) {
                        gaps += 1;
                    }
                }
                last_id = Some(f.frame_id);
                frames += 1;
                keyframes += f.keyframe as u64;
                bytes += f.data.len() as u64;
                max_frame = max_frame.max(f.data.len());
                // Skip the first second: the encoder and clock sync are still settling.
                if let (Some(cap), Some(start)) = (rf.capture_local_us, stream_started) {
                    if start.elapsed() > Duration::from_secs(1) {
                        latency_us.push(rf.received_us as i64 - cap);
                    }
                }
                if let Some(d) = dump.as_mut() {
                    d.write_all(&f.data)?;
                }
            }
        }

        if let (Some((x, y)), Some(start)) = (move_to, stream_started) {
            if !moved && start.elapsed() > Duration::from_secs(2) {
                println!("moving cursor to ({x:.3}, {y:.3})");
                client.send_cursor(x, y);
                moved = true;
            } else if moved && !restored && start.elapsed() > Duration::from_secs(3) {
                println!(
                    "restoring cursor to ({:.3}, {:.3})",
                    cursor_start.0, cursor_start.1
                );
                client.send_cursor(cursor_start.0, cursor_start.1);
                restored = true;
            }
        }
    }

    let elapsed = sampling.elapsed(Instant::now()).as_secs_f64().max(1e-3);
    client.close();
    tokio::time::sleep(Duration::from_millis(100)).await;

    latency_us.sort_unstable();
    println!("--- summary ---");
    if let Some(ff) = first_frame {
        println!(
            "first frame      {:.1} ms after connect start",
            ff.as_secs_f64() * 1e3
        );
    }
    println!(
        "frames           {frames} ({:.1} fps), keyframes {keyframes}, id gaps {gaps}",
        frames as f64 / elapsed
    );
    println!(
        "bitrate          {:.2} Mbps, max frame {} KB",
        bytes as f64 * 8.0 / elapsed / 1e6,
        max_frame / 1024
    );
    client.print_stats();
    if best_rtt_us != u64::MAX {
        println!("ping rtt (min)   {:.2} ms", best_rtt_us as f64 / 1e3);
    }
    let latency = latency_summary(&latency_us);
    if let Some(latency) = latency {
        println!(
            "capture→receive  p50 {:.1} ms  p95 {:.1} ms  p99 {:.1} ms  max {:.1} ms  (n={})",
            latency.p50_us as f64 / 1e3,
            latency.p95_us as f64 / 1e3,
            latency.p99_us as f64 / 1e3,
            latency.max_us as f64 / 1e3,
            latency_us.len()
        );
    }
    if frames == 0 {
        bail!("no frames received (first-frame wait limited to 15 seconds)");
    }
    if let Some(path) = &args.json {
        let latency = latency
            .context("no capture-to-receive samples; refusing to write an empty --json report")?;
        let git_rev = git_revision()?;
        let report = JsonReport {
            seconds: args.seconds,
            frames,
            latency,
            fps: frames as f64 / elapsed,
            bitrate_kbps: bytes as f64 * 8.0 / elapsed / 1e3,
            codec: codec_name(opts.start.codec),
            addr: args.addr,
            git_rev: git_rev.as_deref(),
        };
        write_json(path, &report)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_first_frame_gets_the_full_sampling_window() {
        let opened = Instant::now();
        let duration = Duration::from_secs(3);
        let mut sampling = SamplingWindow::new(opened, duration);
        let first_frame = opened + Duration::from_millis(2607);

        sampling.on_frame(first_frame);

        assert_eq!(sampling.deadline, first_frame + duration);
        assert_eq!(sampling.elapsed(first_frame), Duration::ZERO);
        assert_eq!(sampling.elapsed(sampling.deadline), duration);
    }

    #[test]
    fn later_frames_do_not_extend_the_sampling_window() {
        let opened = Instant::now();
        let mut sampling = SamplingWindow::new(opened, Duration::from_secs(3));
        sampling.on_frame(opened + Duration::from_secs(2));
        let deadline = sampling.deadline;

        sampling.on_frame(opened + Duration::from_secs(4));

        assert_eq!(sampling.deadline, deadline);
        assert_eq!(
            sampling.elapsed(opened + Duration::from_secs(4)),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn missing_first_frame_has_a_bounded_wait() {
        let opened = Instant::now();
        let sampling = SamplingWindow::new(opened, Duration::from_secs(3));

        assert_eq!(sampling.deadline, opened + FIRST_FRAME_TIMEOUT);
        assert_eq!(sampling.elapsed(sampling.deadline), Duration::ZERO);
    }

    #[test]
    fn percentiles_use_rounded_indexes() {
        let samples = [1, 2, 3, 4, 5];

        assert_eq!(percentile(&samples, 0.50), Some(3));
        assert_eq!(percentile(&samples, 0.95), Some(5));
        assert_eq!(percentile(&samples, 0.99), Some(5));
        assert_eq!(percentile(&[], 0.95), None);
    }

    #[test]
    fn latency_summary_rejects_an_empty_sample() {
        assert_eq!(latency_summary(&[]), None);
        assert_eq!(
            latency_summary(&[100, 200, 300]),
            Some(LatencySummary {
                p50_us: 200,
                p95_us: 300,
                p99_us: 300,
                max_us: 300,
            })
        );
    }

    #[test]
    fn json_report_has_the_ci_schema_and_escaped_strings() {
        let report = JsonReport {
            seconds: 20,
            frames: 1200,
            latency: LatencySummary {
                p50_us: 2_500,
                p95_us: 6_000,
                p99_us: 8_000,
                max_us: 9_500,
            },
            fps: 59.75,
            bitrate_kbps: 4_321.5,
            codec: "hevc",
            addr: "127.0.0.1:33401".parse().unwrap(),
            git_rev: Some("abc\\\"def"),
        };

        assert_eq!(
            render_json(&report),
            concat!(
                "{\"schema\":1,\"seconds\":20,\"frames\":1200,\"capture_to_receive_ms\":{",
                "\"p50\":2.500,\"p95\":6.000,\"p99\":8.000,\"max\":9.500},",
                "\"fps\":59.750,\"bitrate_kbps\":4321.500,\"codec\":\"hevc\",",
                "\"addr\":\"127.0.0.1:33401\",\"git_rev\":\"abc\\\\\\\"def\"}"
            )
        );
    }

    #[test]
    fn missing_git_revision_is_json_null() {
        let report = JsonReport {
            seconds: 1,
            frames: 60,
            latency: LatencySummary {
                p50_us: 1,
                p95_us: 2,
                p99_us: 3,
                max_us: 3,
            },
            fps: 60.0,
            bitrate_kbps: 100.0,
            codec: "h264",
            addr: "127.0.0.1:1".parse().unwrap(),
            git_rev: None,
        };

        assert!(render_json(&report).ends_with("\"git_rev\":null}"));
    }
}
